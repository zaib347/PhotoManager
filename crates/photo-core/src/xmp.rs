//! XMP metadata: ratings, keywords, labels, titles and descriptions written by
//! Lightroom, darktable, digiKam, Windows and cameras, in `.xmp` sidecars or embedded in
//! JPEGs. Photo files are never written.
//!
//! Reading: a scan reads each sidecar when it's new or changed (`xmp_seen` holds the
//! mtimes already read), and the embedded XMP of new and changed JPEGs. What's read only
//! fills blanks: a rating or description set here wins, and keywords (and colour labels)
//! are added as tags, never removed.
//!
//! Writing (off by default; see [`Settings`]): triggers queue assets whose rating,
//! favorite, description or tags change in `xmp_dirty`, and [`write_pending`] writes them
//! to each file's sidecar. An existing sidecar keeps everything else in it: our values
//! go in their own `rdf:Description`, and the same properties are removed elsewhere.
//! Sidecars are `IMG_1.xmp` (Lightroom's naming), or `IMG_1.JPG.xmp` when another file
//! in the folder shares the name `IMG_1` or that sidecar already exists.

use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::{NsReader, Writer};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::{Error, Result, library};

const NS_XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/";
const NS_DC: &[u8] = b"http://purl.org/dc/elements/1.1/";
const NS_RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The label written for favorites when [`FavoriteAs::Label`] is chosen, and read back
/// as a favorite.
pub const FAVORITE_LABEL: &str = "Favorite";

/// Sidecars bigger than this aren't XMP worth reading.
const MAX_SIDECAR: u64 = 4 << 20;

/// What an XMP packet says about a photo.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Data {
    /// -1 rejected, 0 unrated, 1 to 5 stars.
    pub rating: Option<i8>,
    pub label: Option<String>,
    /// Keywords.
    pub subjects: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

impl Data {
    pub fn is_empty(&self) -> bool {
        *self == Data::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Prop {
    Rating,
    Label,
    Subject,
    Title,
    Description,
}

fn prop_of(ns: &ResolveResult, local: &[u8]) -> Option<Prop> {
    let ResolveResult::Bound(Namespace(ns)) = ns else {
        return None;
    };
    match (*ns, local) {
        (NS_XMP, b"Rating") => Some(Prop::Rating),
        (NS_XMP, b"Label") => Some(Prop::Label),
        (NS_DC, b"subject") => Some(Prop::Subject),
        (NS_DC, b"title") => Some(Prop::Title),
        (NS_DC, b"description") => Some(Prop::Description),
        _ => None,
    }
}

fn is_rdf(ns: &ResolveResult, local: &[u8], name: &[u8]) -> bool {
    matches!(ns, ResolveResult::Bound(Namespace(n)) if *n == NS_RDF) && local == name
}

fn xml_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("Unreadable XMP: {e}"))
}

impl Data {
    fn set(&mut self, prop: Prop, value: &str, default_lang: bool) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let text = || Some(value.to_owned());
        match prop {
            Prop::Rating => {
                self.rating = value
                    .parse::<f32>()
                    .ok()
                    .map(|r| r.round().clamp(-1.0, 5.0) as i8)
            }
            Prop::Label => self.label = text(),
            Prop::Subject => {
                if !self.subjects.iter().any(|s| s.eq_ignore_ascii_case(value)) {
                    self.subjects.push(value.to_owned());
                }
            }
            // Of several languages, the default one wins, else the first.
            Prop::Title if self.title.is_none() || default_lang => self.title = text(),
            Prop::Description if self.description.is_none() || default_lang => {
                self.description = text()
            }
            Prop::Title | Prop::Description => {}
        }
    }
}

/// Reads the properties we use from an XMP packet. Unknown content is ignored.
pub fn parse(xml: &str) -> Result<Data> {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut data = Data::default();
    // Per open element: the property it's inside, and whether it's an `rdf:li`.
    let mut stack: Vec<(Option<Prop>, bool, bool)> = Vec::new();
    let mut text = String::new();
    let mut seen_li = false;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(xml_error)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let local = e.local_name();
                let inside = stack.last().and_then(|s| s.0);
                let own = prop_of(&ns, local.as_ref());
                let is_li = is_rdf(&ns, local.as_ref(), b"li");
                let is_description = is_rdf(&ns, local.as_ref(), b"Description");
                drop(ns);
                if is_description {
                    // Simple properties as attributes: xmp:Rating="3".
                    for attr in e.attributes().flatten() {
                        let (ans, alocal) = reader.resolve_attribute(attr.key);
                        if let Some(p) = prop_of(&ans, alocal.as_ref()) {
                            let value = attr.unescape_value().map_err(xml_error)?;
                            data.set(p, &value, true);
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    let prop = inside.or(own);
                    let default_lang = is_li
                        && e.attributes().flatten().any(|a| {
                            a.key.as_ref() == b"xml:lang" && a.value.as_ref() == b"x-default"
                        });
                    if inside.is_none() && own.is_some() {
                        seen_li = false;
                    }
                    if is_li {
                        seen_li = true;
                    }
                    stack.push((prop, is_li, default_lang));
                    text.clear();
                }
            }
            Event::Text(t) => {
                if stack.last().is_some_and(|s| s.0.is_some()) {
                    text.push_str(&t.unescape().map_err(xml_error)?);
                }
            }
            Event::CData(t) => {
                if stack.last().is_some_and(|s| s.0.is_some()) {
                    text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::End(_) => {
                let Some((prop, is_li, default_lang)) = stack.pop() else {
                    continue;
                };
                let parent = stack.last().and_then(|s| s.0);
                if let Some(p) = prop {
                    if is_li {
                        data.set(p, &text, default_lang);
                    } else if parent.is_none() && !seen_li {
                        // The property element itself, holding plain text.
                        data.set(p, &text, true);
                    }
                }
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(data)
}

/// What we write to a sidecar.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Values {
    pub rating: Option<i8>,
    /// `None` leaves any existing label alone; `Some(None)` removes it.
    pub label: Option<Option<String>>,
    pub subjects: Vec<String>,
    pub description: Option<String>,
}

const NEW_SIDECAR: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"PhotoManager\">
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
 </rdf:RDF>
</x:xmpmeta>
";

fn esc(s: &str) -> Cow<'_, str> {
    quick_xml::escape::escape(s)
}

/// Our `rdf:Description`, declaring its own namespaces so it fits in any packet.
fn description_xml(v: &Values) -> String {
    let mut body = String::new();
    if let Some(r) = v.rating {
        body.push_str(&format!("   <xmp:Rating>{r}</xmp:Rating>\n"));
    }
    if let Some(Some(label)) = &v.label {
        body.push_str(&format!("   <xmp:Label>{}</xmp:Label>\n", esc(label)));
    }
    if !v.subjects.is_empty() {
        body.push_str("   <dc:subject>\n    <rdf:Bag>\n");
        for s in &v.subjects {
            body.push_str(&format!("     <rdf:li>{}</rdf:li>\n", esc(s)));
        }
        body.push_str("    </rdf:Bag>\n   </dc:subject>\n");
    }
    if let Some(d) = &v.description {
        body.push_str(&format!(
            "   <dc:description>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    </rdf:Alt>\n   </dc:description>\n",
            esc(d)
        ));
    }
    if body.is_empty() {
        return String::new();
    }
    format!(
        "  <rdf:Description rdf:about=\"\" xmlns:rdf=\"{}\" xmlns:xmp=\"{}\" xmlns:dc=\"{}\">\n{body}  </rdf:Description>\n ",
        String::from_utf8_lossy(NS_RDF),
        String::from_utf8_lossy(NS_XMP),
        String::from_utf8_lossy(NS_DC),
    )
}

/// `existing` (or a new packet) with `values` written in: the properties we manage are
/// removed wherever they are and written again in one `rdf:Description` of our own.
/// Everything else is kept as it was.
pub fn merge(existing: Option<&str>, values: &Values) -> Result<String> {
    let manages = |p: Prop| match p {
        Prop::Rating | Prop::Subject | Prop::Description => true,
        Prop::Label => values.label.is_some(),
        Prop::Title => false,
    };
    let emit = |event: Event| -> Result<Vec<u8>> {
        let mut w = Writer::new(Vec::new());
        w.write_event(event).map_err(xml_error)?;
        Ok(w.into_inner())
    };
    let mut reader = NsReader::from_str(existing.unwrap_or(NEW_SIDECAR));
    let mut out: Vec<u8> = Vec::new();
    // Whitespace not yet written: dropped with an element removed right after it.
    let mut pending_ws: Vec<u8> = Vec::new();
    // A top-level `rdf:Description` being copied: the whitespace before it, its
    // content so far, and whether anything but namespaces and `rdf:about` is left.
    let mut block: Option<(Vec<u8>, Vec<u8>, bool)> = None;
    let mut depth = 0usize;
    // Depth of the elements directly inside `rdf:RDF`.
    let mut rdf_inner: Option<usize> = None;
    // While Some(d), events are dropped until the element opened at depth d closes.
    let mut skipping: Option<usize> = None;
    let mut wrote = false;
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(xml_error)?;
        let local = match &event {
            Event::Start(e) | Event::Empty(e) => Some(e.local_name().as_ref().to_vec()),
            Event::End(e) => Some(e.local_name().as_ref().to_vec()),
            _ => None,
        };
        let local = local.as_deref().unwrap_or_default();
        let prop = prop_of(&ns, local);
        let description = is_rdf(&ns, local, b"Description");
        let rdf = is_rdf(&ns, local, b"RDF");
        drop(ns);

        if let Some(d) = skipping {
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    depth -= 1;
                    if depth == d {
                        skipping = None;
                    }
                }
                Event::Eof => return Err(xml_error("unexpected end")),
                _ => {}
            }
            continue;
        }
        if let Event::Text(t) = &event
            && t.iter().all(u8::is_ascii_whitespace)
        {
            pending_ws.extend_from_slice(t);
            continue;
        }
        let top_level = rdf_inner == Some(depth);
        let in_block = block.is_some() && rdf_inner.map(|d| d + 1) == Some(depth);
        match event {
            // A managed property directly inside a top-level description: removed.
            Event::Start(_) | Event::Empty(_) if in_block && prop.is_some_and(manages) => {
                pending_ws.clear();
                if matches!(event, Event::Start(_)) {
                    skipping = Some(depth);
                    depth += 1;
                }
            }
            Event::Start(ref e) | Event::Empty(ref e) if description && top_level => {
                let mut kept =
                    BytesStart::new(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                let mut has_content = false;
                for attr in e.attributes().flatten() {
                    let (ans, alocal) = reader.resolve_attribute(attr.key);
                    if prop_of(&ans, alocal.as_ref()).is_some_and(manages) {
                        continue;
                    }
                    let key = attr.key.as_ref();
                    let about = is_rdf(&ans, alocal.as_ref(), b"about");
                    if !(about || key == b"xmlns" || key.starts_with(b"xmlns:")) {
                        has_content = true;
                    }
                    kept.push_attribute(attr);
                }
                let ws = std::mem::take(&mut pending_ws);
                if matches!(event, Event::Start(_)) {
                    block = Some((ws, emit(Event::Start(kept))?, has_content));
                    depth += 1;
                } else if has_content {
                    out.extend(ws);
                    out.extend(emit(Event::Empty(kept))?);
                }
            }
            Event::End(_) if block.is_some() && rdf_inner == Some(depth - 1) => {
                // The end of a top-level description: kept only if anything is left.
                depth -= 1;
                let (ws, mut content, has_content) = block.take().expect("checked");
                if has_content {
                    content.extend(std::mem::take(&mut pending_ws));
                    content.extend(emit(event)?);
                    out.extend(ws);
                    out.extend(content);
                } else {
                    pending_ws.clear();
                }
            }
            Event::Eof => break,
            other => {
                if in_block
                    && matches!(other, Event::Start(_) | Event::Empty(_))
                    && let Some((_, _, has_content)) = &mut block
                {
                    *has_content = true;
                }
                let start = matches!(other, Event::Start(_));
                let end = matches!(other, Event::End(_));
                let target = match &mut block {
                    Some((_, content, _)) => content,
                    None => &mut out,
                };
                target.append(&mut pending_ws);
                if rdf && end && !wrote {
                    target.extend(description_xml(values).as_bytes());
                    wrote = true;
                }
                target.extend(emit(other)?);
                if start {
                    if rdf {
                        rdf_inner = Some(depth + 1);
                    }
                    depth += 1;
                } else if end {
                    depth = depth.saturating_sub(1);
                }
            }
        }
    }
    out.extend(pending_ws);
    if !wrote {
        return Err(xml_error("no rdf:RDF element"));
    }
    String::from_utf8(out).map_err(xml_error)
}

/// The XMP packet embedded in a JPEG (an APP1 segment), if it has one. Reads only the
/// segments before the image data.
pub fn embedded(path: &Path) -> Option<String> {
    const HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    let mut file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut two = [0u8; 2];
    file.read_exact(&mut two).ok()?;
    if two != [0xFF, 0xD8] {
        return None;
    }
    loop {
        file.read_exact(&mut two).ok()?;
        if two[0] != 0xFF {
            return None;
        }
        let marker = two[1];
        // Start of scan, or end of image: no more metadata.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        if marker == 0xFF || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        file.read_exact(&mut two).ok()?;
        let len = u16::from_be_bytes(two) as usize;
        if len < 2 {
            return None;
        }
        if marker == 0xE1 && len - 2 > HEADER.len() {
            let mut segment = vec![0u8; len - 2];
            file.read_exact(&mut segment).ok()?;
            if let Some(packet) = segment.strip_prefix(HEADER) {
                return Some(String::from_utf8_lossy(packet).into_owned());
            }
        } else {
            file.seek(SeekFrom::Current(len as i64 - 2)).ok()?;
        }
    }
}

// ---------------------------------------------------------------- settings

/// How a favorite is written, since XMP has no favorite of its own.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum FavoriteAs {
    /// Five stars, unless the photo has a rating of its own.
    #[default]
    Rating,
    /// The label "Favorite".
    Label,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Write ratings, tags, favorites and descriptions to sidecars.
    pub write: bool,
    pub favorite_as: FavoriteAs,
}

fn setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
        .query_row(params![key], |r| r.get(0))
        .optional()?)
}

pub fn settings(conn: &Connection) -> Result<Settings> {
    Ok(Settings {
        write: setting(conn, "xmp_write")?.as_deref() == Some("1"),
        favorite_as: match setting(conn, "xmp_favorite")?.as_deref() {
            Some("label") => FavoriteAs::Label,
            _ => FavoriteAs::Rating,
        },
    })
}

/// Saves the settings. Turning writing on (or changing how favorites are written)
/// queues every photo that has something to write; turning it off empties the queue.
pub fn set_settings(conn: &mut Connection, s: Settings) -> Result<()> {
    let before = settings(conn)?;
    let tx = conn.transaction()?;
    let put = |key: &str, value: &str| {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
    };
    put("xmp_write", if s.write { "1" } else { "0" })?;
    put(
        "xmp_favorite",
        match s.favorite_as {
            FavoriteAs::Rating => "rating",
            FavoriteAs::Label => "label",
        },
    )?;
    if !s.write {
        tx.execute("DELETE FROM xmp_dirty", [])?;
    } else if !before.write || before.favorite_as != s.favorite_as {
        tx.execute_batch(
            "INSERT OR IGNORE INTO xmp_dirty (asset_id)
                 SELECT asset_id FROM user_meta
                 WHERE rating IS NOT NULL OR favorite = 1 OR note IS NOT NULL
                 UNION SELECT asset_id FROM asset_tags;",
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Sets (or with `None` clears) a photo's description.
pub fn set_description(conn: &Connection, asset_id: i64, text: Option<&str>) -> Result<()> {
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    conn.execute(
        "INSERT INTO user_meta (asset_id, note) VALUES (?1, ?2)
         ON CONFLICT (asset_id) DO UPDATE SET note = excluded.note",
        params![asset_id, text],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- reading into the library

/// Fills in what `asset_id` doesn't have yet from `data`: rating, description (or title),
/// and favorite (our own label). Keywords and other labels are added as tags. Doesn't
/// queue the asset for writing because of this.
pub fn apply(conn: &Connection, asset_id: i64, data: &Data) -> Result<()> {
    let queued: bool = conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM xmp_dirty WHERE asset_id = ?1)")?
        .query_row(params![asset_id], |r| r.get(0))?;
    if let Some(r) = data.rating.filter(|r| (1..=5).contains(r)) {
        conn.prepare_cached(
            "INSERT INTO user_meta (asset_id, rating) VALUES (?1, ?2)
             ON CONFLICT (asset_id) DO UPDATE SET rating = COALESCE(rating, excluded.rating)",
        )?
        .execute(params![asset_id, r])?;
    }
    if let Some(text) = data.description.as_ref().or(data.title.as_ref()) {
        conn.prepare_cached(
            "INSERT INTO user_meta (asset_id, note) VALUES (?1, ?2)
             ON CONFLICT (asset_id) DO UPDATE SET note = COALESCE(note, excluded.note)",
        )?
        .execute(params![asset_id, text])?;
    }
    let mut tags: Vec<&str> = data.subjects.iter().map(String::as_str).collect();
    match data.label.as_deref() {
        Some(l) if l.eq_ignore_ascii_case(FAVORITE_LABEL) => {
            conn.prepare_cached(
                "INSERT INTO user_meta (asset_id, favorite) VALUES (?1, 1)
                 ON CONFLICT (asset_id) DO UPDATE SET favorite = 1",
            )?
            .execute(params![asset_id])?;
        }
        Some(l) => tags.push(l),
        None => {}
    }
    for name in tags {
        let name: String = name.trim().chars().take(100).collect();
        if name.is_empty() {
            continue;
        }
        conn.prepare_cached("INSERT OR IGNORE INTO tags (name) VALUES (?1)")?
            .execute(params![name])?;
        conn.prepare_cached(
            "INSERT OR IGNORE INTO asset_tags (asset_id, tag_id)
             SELECT ?1, id FROM tags WHERE name = ?2 COLLATE NOCASE",
        )?
        .execute(params![asset_id, name])?;
    }
    if !queued {
        conn.prepare_cached("DELETE FROM xmp_dirty WHERE asset_id = ?1")?
            .execute(params![asset_id])?;
    }
    Ok(())
}

/// The library files a sidecar belongs to, as (file id, asset id): `IMG_1.JPG.xmp`
/// belongs to `IMG_1.JPG`, and `IMG_1.xmp` to every `IMG_1.*` in its folder.
fn sidecar_owners(conn: &Connection, root_id: i64, rel: &str) -> Result<Vec<(i64, i64)>> {
    let Some(base) = rel
        .len()
        .checked_sub(4)
        .filter(|&n| rel.is_char_boundary(n) && rel[n..].eq_ignore_ascii_case(".xmp"))
        .map(|n| &rel[..n])
    else {
        return Ok(Vec::new());
    };
    let exact: Option<(i64, i64)> = conn
        .prepare_cached(
            "SELECT id, asset_id FROM files
             WHERE root_id = ?1 AND rel_path = ?2 AND asset_id IS NOT NULL",
        )?
        .query_row(params![root_id, base], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    if let Some(owner) = exact {
        return Ok(vec![owner]);
    }
    // `base.` up to `base/` covers every `base.<ext>`; '/' follows '.' in ASCII.
    let mut stmt = conn.prepare_cached(
        "SELECT id, asset_id, rel_path FROM files
         WHERE root_id = ?1 AND rel_path > ?2 AND rel_path < ?3 AND asset_id IS NOT NULL",
    )?;
    let rows = stmt.query_map(
        params![root_id, format!("{base}."), format!("{base}/")],
        |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?)),
    )?;
    let mut owners = Vec::new();
    for row in rows {
        let (id, asset, path) = row?;
        let ext = &path[base.len() + 1..];
        if !ext.is_empty() && !ext.contains(['/', '.']) {
            owners.push((id, asset));
        }
    }
    Ok(owners)
}

fn read_sidecar(path: &Path) -> Option<std::io::Result<String>> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_SIDECAR => None,
        Ok(_) => Some(std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned())),
        Err(e) => Some(Err(e)),
    }
}

/// Reads the sidecars a scan found under `root` (relative path, mtime) that are new or
/// changed since they were last read, into the photos they belong to. Returns how many
/// were read.
pub fn import_sidecars(
    library: &Db,
    root_id: i64,
    root: &Path,
    found: &[(String, i64)],
) -> Result<usize> {
    let changed: Vec<&(String, i64)> = {
        let conn = library.read();
        let mut seen =
            conn.prepare("SELECT mtime FROM xmp_seen WHERE root_id = ?1 AND rel_path = ?2")?;
        let mut changed = Vec::new();
        for item in found {
            let mtime: Option<i64> = seen
                .query_row(params![root_id, item.0], |r| r.get(0))
                .optional()?;
            if mtime != Some(item.1) {
                changed.push(item);
            }
        }
        changed
    };
    let mut read = 0;
    for chunk in changed.chunks(256) {
        // Parse outside the write lock.
        let parsed: Vec<(&String, i64, Option<Data>)> = chunk
            .iter()
            .filter_map(|(rel, mtime)| {
                let text = read_sidecar(&root.join(rel))?.ok()?;
                // A sidecar that doesn't parse is remembered, so it isn't retried until
                // it changes.
                Some((rel, *mtime, parse(&text).ok()))
            })
            .collect();
        let mut conn = library.write();
        let tx = conn.transaction()?;
        for (rel, mtime, data) in &parsed {
            if let Some(data) = data.as_ref().filter(|d| !d.is_empty()) {
                for (_, asset_id) in sidecar_owners(&tx, root_id, rel)? {
                    apply(&tx, asset_id, data)?;
                }
                read += 1;
            }
            mark_seen(&tx, root_id, rel, *mtime)?;
        }
        tx.commit()?;
    }
    Ok(read)
}

fn mark_seen(conn: &Connection, root_id: i64, rel: &str, mtime: i64) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO xmp_seen (root_id, rel_path, mtime) VALUES (?1, ?2, ?3)
         ON CONFLICT (root_id, rel_path) DO UPDATE SET mtime = excluded.mtime",
    )?
    .execute(params![root_id, rel, mtime])?;
    Ok(())
}

// ---------------------------------------------------------------- writing sidecars

fn mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as i64,
    )
}

/// Where the sidecar of library file `rel` (in root `root_id`) lives or will live.
fn sidecar_rel(conn: &Connection, root: &Path, root_id: i64, rel: &str) -> Result<String> {
    let full = format!("{rel}.xmp");
    if root.join(&full).is_file() {
        return Ok(full);
    }
    let (folder, name) = rel.rsplit_once('/').map_or(("", rel), |(f, n)| (f, n));
    let Some((stem, _)) = name.rsplit_once('.') else {
        return Ok(full);
    };
    let short = if folder.is_empty() {
        format!("{stem}.xmp")
    } else {
        format!("{folder}/{stem}.xmp")
    };
    if root.join(&short).is_file() {
        return Ok(short);
    }
    // A RAW+JPEG pair (or any two files named IMG_1.*) would share `IMG_1.xmp`.
    let owners = sidecar_owners(conn, root_id, &short)?;
    Ok(if owners.len() > 1 { full } else { short })
}

/// The values to write for `asset_id`, from the library.
fn values_of(
    conn: &Connection,
    asset_id: i64,
    s: Settings,
    existing: Option<&Data>,
) -> Result<Values> {
    let (rating, favorite, note): (Option<i8>, bool, Option<String>) = conn
        .prepare_cached("SELECT rating, favorite, note FROM user_meta WHERE asset_id = ?1")?
        .query_row(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
        .unwrap_or_default();
    let label = existing.and_then(|d| d.label.clone());
    let favorite_label = |l: &String| l.eq_ignore_ascii_case(FAVORITE_LABEL);
    let mut subjects: Vec<String> = crate::collections::tags_of(conn, asset_id)?
        .into_iter()
        .map(|t| t.name)
        .collect();
    // A colour label read in as a tag stays a label rather than becoming a keyword.
    if let Some(l) = &label {
        subjects.retain(|t| !t.eq_ignore_ascii_case(l));
    }
    let (rating, label) = match s.favorite_as {
        FavoriteAs::Rating => (
            rating.or((favorite).then_some(5)).or(
                // Keep a reject or "unrated" we never read in.
                existing.and_then(|d| d.rating).filter(|r| *r <= 0),
            ),
            None,
        ),
        FavoriteAs::Label => (
            rating.or(existing.and_then(|d| d.rating).filter(|r| *r <= 0)),
            Some(if favorite {
                Some(FAVORITE_LABEL.to_owned())
            } else {
                label.filter(|l| !favorite_label(l))
            }),
        ),
    };
    Ok(Values {
        rating,
        label,
        subjects,
        description: note,
    })
}

#[derive(Serialize, Debug, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    pub written: usize,
    pub failed: usize,
}

/// Writes the sidecars of queued assets, up to `limit` assets. Files on drives that
/// aren't connected wait in the queue. A sidecar changed by another app since it was
/// last read is read in first, so its new values aren't lost.
pub fn write_pending(library: &Db, limit: usize) -> Result<WriteReport> {
    let mut report = WriteReport::default();
    let (s, queued) = {
        let conn = library.read();
        let s = settings(&conn)?;
        if !s.write {
            return Ok(report);
        }
        let mut stmt =
            conn.prepare("SELECT rowid, asset_id FROM xmp_dirty ORDER BY rowid LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        (s, rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    for (rowid, asset_id) in queued {
        let files: Vec<(i64, i64, String, Option<PathBuf>)> = {
            let conn = library.read();
            let mut stmt = conn.prepare_cached(
                "SELECT f.id, f.root_id, f.rel_path FROM files f
                 WHERE f.asset_id = ?1 AND f.missing_since IS NULL",
            )?;
            let rows =
                stmt.query_map(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let mut files = Vec::new();
            for row in rows {
                let (id, root_id, rel): (i64, i64, String) = row?;
                let root = library::root_path(&conn, root_id)?;
                files.push((id, root_id, rel, root));
            }
            files
        };
        if files.iter().any(|f| f.3.is_none()) {
            // Wait for the drive; try again later.
            continue;
        }
        let mut ok = true;
        for (_, root_id, rel, root) in &files {
            let root = root.as_ref().expect("checked");
            let side_rel = sidecar_rel(&library.read(), root, *root_id, rel)?;
            let path = root.join(&side_rel);
            let existing_text = match read_sidecar(&path) {
                None => {
                    ok = false;
                    continue;
                }
                Some(Ok(text)) => Some(text),
                Some(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
                Some(Err(_)) => {
                    ok = false;
                    continue;
                }
            };
            let existing = match existing_text.as_deref().map(parse) {
                Some(Ok(d)) => Some(d),
                // Not ours to overwrite.
                Some(Err(_)) => {
                    ok = false;
                    continue;
                }
                None => None,
            };
            let mtime = mtime_ms(&path);
            if let (Some(d), Some(m)) = (&existing, mtime) {
                let conn = library.write();
                let seen: Option<i64> = conn
                    .prepare_cached(
                        "SELECT mtime FROM xmp_seen WHERE root_id = ?1 AND rel_path = ?2",
                    )?
                    .query_row(params![root_id, side_rel], |r| r.get(0))
                    .optional()?;
                if seen != Some(m) {
                    apply(&conn, asset_id, d)?;
                }
            }
            let values = values_of(&library.read(), asset_id, s, existing.as_ref())?;
            let Ok(xml) = merge(existing_text.as_deref(), &values) else {
                ok = false;
                continue;
            };
            if existing_text.as_deref() == Some(xml.as_str()) {
                continue;
            }
            let tmp = path.with_extension("xmp.tmp");
            if std::fs::write(&tmp, xml.as_bytes())
                .and_then(|_| std::fs::rename(&tmp, &path))
                .is_err()
            {
                let _ = std::fs::remove_file(&tmp);
                ok = false;
                continue;
            }
            if let Some(m) = mtime_ms(&path) {
                mark_seen(&library.write(), *root_id, &side_rel, m)?;
            }
        }
        if ok {
            report.written += 1;
        } else {
            report.failed += 1;
        }
        // Done (or failed for good): leave the queue, unless it changed meanwhile.
        library.write().execute(
            "DELETE FROM xmp_dirty WHERE rowid = ?1 AND asset_id = ?2",
            params![rowid, asset_id],
        )?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="4"
   xmp:Label="Red"
   crs:Exposure2012="+0.50">
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="fr">Plage</rdf:li>
     <rdf:li xml:lang="x-default">Beach &amp; sun</rdf:li>
    </rdf:Alt>
   </dc:title>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>beach</rdf:li>
     <rdf:li>Family</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn reads_attributes_elements_and_languages() {
        let d = parse(LIGHTROOM).unwrap();
        assert_eq!(d.rating, Some(4));
        assert_eq!(d.label.as_deref(), Some("Red"));
        assert_eq!(d.subjects, vec!["beach", "Family"]);
        assert_eq!(d.title.as_deref(), Some("Beach & sun"));
        assert_eq!(d.description, None);

        // darktable / exiftool style: properties as elements, other prefixes.
        let other = r#"<?xml version="1.0"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <r:Description xmlns:a="http://ns.adobe.com/xap/1.0/" xmlns:d="http://purl.org/dc/elements/1.1/">
            <a:Rating>-1</a:Rating>
            <d:description><r:Alt><r:li xml:lang="x-default">A note</r:li></r:Alt></d:description>
            </r:Description></r:RDF></x:xmpmeta>"#;
        let d = parse(other).unwrap();
        assert_eq!(
            (d.rating, d.description.as_deref()),
            (Some(-1), Some("A note"))
        );
    }

    #[test]
    fn writing_keeps_everything_else() {
        let values = Values {
            rating: Some(2),
            label: None,
            subjects: vec!["sea".into(), "Tom & Ann".into()],
            description: Some("Day <one>".into()),
        };
        let merged = merge(Some(LIGHTROOM), &values).unwrap();
        assert!(merged.contains("crs:Exposure2012=\"+0.50\""), "{merged}");
        assert!(
            merged.contains("xmp:Label=\"Red\""),
            "label not managed: {merged}"
        );
        assert!(!merged.contains("xmp:Rating=\"4\""));
        let d = parse(&merged).unwrap();
        assert_eq!(d.rating, Some(2));
        assert_eq!(d.label.as_deref(), Some("Red"));
        assert_eq!(d.subjects, vec!["sea", "Tom & Ann"]);
        assert_eq!(d.title.as_deref(), Some("Beach & sun"));
        assert_eq!(d.description.as_deref(), Some("Day <one>"));
        // Writing the same again changes nothing.
        assert_eq!(merge(Some(&merged), &values).unwrap(), merged);

        let fresh = merge(
            None,
            &Values {
                label: Some(Some(FAVORITE_LABEL.into())),
                ..Default::default()
            },
        )
        .unwrap();
        let d = parse(&fresh).unwrap();
        assert_eq!((d.rating, d.label.as_deref()), (None, Some(FAVORITE_LABEL)));
        // Removing the label.
        let cleared = merge(
            Some(&fresh),
            &Values {
                label: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(parse(&cleared).unwrap().is_empty(), "{cleared}");
    }

    #[test]
    fn finds_xmp_embedded_in_a_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jpg");
        let packet = LIGHTROOM.as_bytes();
        let mut bytes = vec![0xFF, 0xD8];
        // An EXIF segment first, then the XMP one, then the image data.
        let exif = b"Exif\0\0junk";
        bytes.extend([0xFF, 0xE1]);
        bytes.extend(((exif.len() + 2) as u16).to_be_bytes());
        bytes.extend(exif);
        let header = b"http://ns.adobe.com/xap/1.0/\0";
        bytes.extend([0xFF, 0xE1]);
        bytes.extend(((header.len() + packet.len() + 2) as u16).to_be_bytes());
        bytes.extend(header);
        bytes.extend(packet);
        bytes.extend([0xFF, 0xDA, 0, 2, 1, 2, 3]);
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(embedded(&path).as_deref(), Some(LIGHTROOM));

        std::fs::write(&path, [0xFF, 0xD8, 0xFF, 0xDA, 0, 2]).unwrap();
        assert_eq!(embedded(&path), None);
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Db,
        photos: PathBuf,
        root: i64,
    }

    /// Library files (path, asset id), with their files on disk.
    fn fixture(files: &[(&str, i64)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            2,
            crate::schema::migrate_library,
        )
        .unwrap();
        let root = library::add_root(&db.write(), &photos).unwrap();
        {
            let conn = db.write();
            for (rel, id) in files {
                std::fs::write(photos.join(rel), b"x").unwrap();
                conn.execute(
                    "INSERT OR IGNORE INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
                     VALUES (?1, 0, 0, 0, 0, 0)",
                    params![id],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO files (root_id, rel_path, size, mtime, asset_id) VALUES (?1, ?2, 1, 0, ?3)",
                    params![root, rel, id],
                )
                .unwrap();
            }
        }
        Fixture {
            _dir: dir,
            db,
            photos,
            root,
        }
    }

    fn found(f: &Fixture, names: &[&str]) -> Vec<(String, i64)> {
        names
            .iter()
            .map(|n| (n.to_string(), mtime_ms(&f.photos.join(n)).unwrap()))
            .collect()
    }

    type Meta = (Option<i64>, bool, Option<String>, Vec<String>);

    fn meta(f: &Fixture, id: i64) -> Meta {
        let conn = f.db.read();
        let (rating, favorite, note) = conn
            .query_row(
                "SELECT rating, favorite, note FROM user_meta WHERE asset_id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap()
            .unwrap_or_default();
        let tags = crate::collections::tags_of(&conn, id)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        (rating, favorite, note, tags)
    }

    fn queued(f: &Fixture) -> i64 {
        f.db.read()
            .query_row("SELECT COUNT(*) FROM xmp_dirty", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn sidecars_fill_blanks_once_and_are_written_back() {
        let f = fixture(&[("IMG_1.JPG", 1), ("IMG_2.CR2", 2), ("IMG_2.JPG", 3)]);
        std::fs::write(f.photos.join("IMG_1.xmp"), LIGHTROOM).unwrap();
        std::fs::write(
            f.photos.join("IMG_2.CR2.xmp"),
            LIGHTROOM.replace("beach", "raw only"),
        )
        .unwrap();
        // A rating set here wins over the sidecar's.
        crate::assets::set_rating(&mut f.db.write(), &[1], Some(2)).unwrap();

        let all = found(&f, &["IMG_1.xmp", "IMG_2.CR2.xmp"]);
        assert_eq!(import_sidecars(&f.db, f.root, &f.photos, &all).unwrap(), 2);
        let (rating, _, note, tags) = meta(&f, 1);
        assert_eq!((rating, note.as_deref()), (Some(2), Some("Beach & sun")));
        assert_eq!(tags, vec!["beach", "Family", "Red"]);
        assert!(meta(&f, 2).3.contains(&"raw only".to_string()));
        assert!(meta(&f, 3).3.is_empty(), "IMG_2.CR2.xmp is the RAW's alone");
        // Unchanged sidecars aren't read again.
        assert_eq!(import_sidecars(&f.db, f.root, &f.photos, &all).unwrap(), 0);
        assert_eq!(queued(&f), 0);

        // Turn writing on: everything with data is queued and written.
        set_settings(
            &mut f.db.write(),
            Settings {
                write: true,
                favorite_as: FavoriteAs::Label,
            },
        )
        .unwrap();
        crate::assets::set_favorite(&mut f.db.write(), &[3], true).unwrap();
        crate::collections::add_tag(&mut f.db.write(), &[3], "jpeg twin").unwrap();
        let report = write_pending(&f.db, 100).unwrap();
        assert_eq!(
            report,
            WriteReport {
                written: 3,
                failed: 0
            }
        );

        let one = std::fs::read_to_string(f.photos.join("IMG_1.xmp")).unwrap();
        assert!(one.contains("crs:Exposure2012"), "kept: {one}");
        let d = parse(&one).unwrap();
        assert_eq!((d.rating, d.label.as_deref()), (Some(2), Some("Red")));
        assert_eq!(
            d.subjects,
            vec!["beach", "Family"],
            "the label stays a label"
        );
        // IMG_2.JPG shares its name with the RAW, so it gets IMG_2.JPG.xmp.
        let twin =
            parse(&std::fs::read_to_string(f.photos.join("IMG_2.JPG.xmp")).unwrap()).unwrap();
        assert_eq!(twin.label.as_deref(), Some(FAVORITE_LABEL));
        assert_eq!(twin.subjects, vec!["jpeg twin"]);
        assert!(!f.photos.join("IMG_2.xmp").exists());

        // What we wrote isn't read back as new.
        let all = found(&f, &["IMG_1.xmp", "IMG_2.CR2.xmp", "IMG_2.JPG.xmp"]);
        assert_eq!(import_sidecars(&f.db, f.root, &f.photos, &all).unwrap(), 0);
        assert_eq!(write_pending(&f.db, 100).unwrap(), WriteReport::default());

        // Turning writing off stops queueing.
        set_settings(&mut f.db.write(), Settings::default()).unwrap();
        crate::assets::set_rating(&mut f.db.write(), &[3], Some(5)).unwrap();
        assert_eq!(queued(&f), 0);
    }
}
