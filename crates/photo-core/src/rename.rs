//! Renaming photos from a pattern such as `{date} {place} {counter}`.
//!
//! Like the organizer it plans first (`preview` changes nothing), then renames through
//! `fileops`, so every rename is journaled and can be undone; favorites, tags and edits
//! stay with the photo because the library row follows the file. Files keep their
//! folder and extension.
//!
//! Files that belong together are renamed together, to the same new name with their
//! own extensions: a RAW+JPEG pair, a Live Photo's clip (same folder, same name), and
//! sidecars (`IMG_1.xmp`, `IMG_1.AAE`, `IMG_1.CR2.xmp`), which the library doesn't
//! index but are carried along in the same operation.
//!
//! Names never overwrite: a taken name gets " (2)". Renaming into a name another file
//! in the same batch is leaving is allowed, ordered so it is free by then, which lets a
//! pattern with `{counter}` be applied again after photos were added or removed.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Timelike};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::db::Db;
use crate::fileops::{self, Companion, Report};
use crate::{Error, Result, library};

/// Renames listed in a preview.
const SAMPLE: usize = 50;
/// Longest new name (without extension), in characters.
const MAX_STEM: usize = 150;
/// Sidecar extensions carried along with a photo.
const SIDECARS: &[&str] = &["xmp", "aae", "thm"];
/// Stands in for `{counter}` until numbers are given out.
const COUNTER: char = '\u{1}';

pub const TOKENS: &[&str] = &[
    "date", "time", "year", "month", "day", "camera", "place", "event", "trip", "counter",
    "original",
];

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rename {
    /// Paths relative to the library folder, with `/`.
    pub from: String,
    pub to: String,
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// Files that would be renamed, sidecars included.
    pub files: usize,
    /// Of those, sidecars (`.xmp`, `.aae`).
    pub sidecars: usize,
    /// Photos whose name already fits the pattern.
    pub in_place: usize,
    /// Names that needed " (2)" because the name was taken.
    pub numbered: usize,
    /// The first few renames.
    pub sample: Vec<Rename>,
}

/// What a name is made from.
#[derive(Debug, Default, Clone)]
struct Facts {
    /// Wall-clock capture time, ms.
    wall: i64,
    camera: Option<String>,
    place: Option<String>,
    event: Option<String>,
    trip: Option<String>,
}

/// Files in one folder sharing a name (stem): renamed as one.
#[derive(Debug)]
struct Shot {
    root_id: i64,
    folder: String,
    stem: String,
    facts: Facts,
    /// Library files: (file id, rel path).
    files: Vec<(i64, String)>,
    /// Sidecars on disk: rel paths.
    sidecars: Vec<String>,
}

fn split(rel: &str) -> (&str, &str) {
    rel.rsplit_once('/').unwrap_or(("", rel))
}

/// (stem, extension with its dot) of a file name.
fn stem_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// Checks a pattern's `{tokens}`.
pub fn validate(pattern: &str) -> Result<()> {
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| Error::Invalid("A { in the pattern has no closing }".into()))?;
        let token = &after[..close];
        if !TOKENS.contains(&token) {
            return Err(Error::Invalid(format!(
                "\"{{{token}}}\" isn't a name part; use {}",
                TOKENS
                    .iter()
                    .map(|t| format!("{{{t}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        rest = &after[close + 1..];
    }
    if pattern.trim().is_empty() {
        return Err(Error::Invalid("Type a pattern for the new names".into()));
    }
    Ok(())
}

/// Characters Windows (and so a USB drive) can't have in names become "-".
fn clean_part(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

/// Tidies a rendered name: an empty part leaves no doubled or dangling separators.
fn tidy(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for word in name.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let mut prev = out.clone();
    loop {
        let next = prev
            .replace(" - - ", " - ")
            .replace("--", "-")
            .replace("__", "_")
            .replace("- -", "-");
        if next == prev {
            break;
        }
        prev = next;
    }
    let trimmed = prev.trim_matches(|c: char| c == ' ' || c == '-' || c == '_' || c == '.');
    let mut stem: String = trimmed.chars().take(MAX_STEM).collect();
    // Windows reserves a few names whatever the extension.
    let upper = stem.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str())
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        stem.push('_');
    }
    stem
}

/// The name for a shot, with `COUNTER` where the number goes.
fn render(pattern: &str, facts: &Facts, original: &str) -> String {
    let date = DateTime::from_timestamp_millis(facts.wall).unwrap_or_default();
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&clean_part(&rest[..open]));
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        let value = match &after[..close] {
            "date" => format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day()),
            "time" => format!(
                "{:02}.{:02}.{:02}",
                date.hour(),
                date.minute(),
                date.second()
            ),
            "year" => format!("{:04}", date.year()),
            "month" => format!("{:02}", date.month()),
            "day" => format!("{:02}", date.day()),
            "camera" => facts.camera.clone().unwrap_or_default(),
            "place" => facts.place.clone().unwrap_or_default(),
            "event" => facts.event.clone().unwrap_or_default(),
            "trip" => facts.trip.clone().unwrap_or_default(),
            "original" => original.to_owned(),
            "counter" => {
                out.push(COUNTER);
                String::new()
            }
            _ => String::new(),
        };
        out.push_str(clean_part(&value).trim());
        rest = &after[close + 1..];
    }
    out.push_str(&clean_part(rest));
    tidy(&out)
}

/// "Canon EOS R6" rather than "Canon Canon EOS R6".
fn camera_name(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (_, None) => None,
        (Some(make), Some(model)) => {
            let first = make.split_whitespace().next().unwrap_or("").to_lowercase();
            if first.is_empty() || model.to_lowercase().starts_with(&first) {
                Some(model)
            } else {
                Some(format!(
                    "{} {model}",
                    make.split_whitespace().next().unwrap_or("")
                ))
            }
        }
        (None, Some(model)) => Some(model),
    }
}

/// The trip containing `local_day`: its title, else the town most of it was in.
fn trip_name(
    conn: &Connection,
    local_day: i64,
    cache: &mut HashMap<i64, Option<String>>,
) -> Result<Option<String>> {
    if let Some(name) = cache.get(&local_day) {
        return Ok(name.clone());
    }
    let name: Option<String> = conn
        .prepare_cached(
            "SELECT COALESCE(t.title, (SELECT p.name FROM trip_places tp
                                       JOIN places p ON p.id = tp.place_id
                                       WHERE tp.trip_id = t.id ORDER BY tp.count DESC LIMIT 1))
             FROM trips t WHERE t.status != 'rejected' AND ?1 BETWEEN t.first_day AND t.last_day
             LIMIT 1",
        )?
        .query_row(params![local_day], |r| r.get(0))
        .optional()?
        .flatten();
    cache.insert(local_day, name.clone());
    Ok(name)
}

/// Gathers the shots for `asset_ids` (and their Live Photo clips), with any other
/// library files and sidecars that share their names.
fn shots(conn: &Connection, asset_ids: &[i64]) -> Result<Vec<Shot>> {
    let mut by_key: HashMap<(i64, String, String), Shot> = HashMap::new();
    let mut trips = HashMap::new();
    let mut seen_files = HashSet::new();
    {
        let mut files = conn.prepare_cached(
            "SELECT f.id, f.root_id, f.rel_path, a.media_kind,
                    COALESCE(s.taken_at, a.taken_at) + 60000 * COALESCE(s.tz_offset_min, a.tz_offset_min),
                    COALESCE(s.local_day, a.local_day),
                    a.camera_make, a.camera_model, p.name, COALESCE(e.title, ep.name)
             FROM files f JOIN assets a ON a.id = f.asset_id
             LEFT JOIN assets s ON s.id = a.paired_with
             LEFT JOIN places p ON p.id = COALESCE(s.place_id, a.place_id)
             LEFT JOIN events e ON e.id = COALESCE(s.event_id, a.event_id) AND e.status != 'rejected'
             LEFT JOIN places ep ON ep.id = e.place_id
             WHERE f.missing_since IS NULL AND (a.id = ?1 OR a.paired_with = ?1)",
        )?;
        for &id in asset_ids {
            let rows: Vec<_> = files
                .query_map(params![id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, u8>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (file_id, root_id, rel, kind, wall, day, make, model, place, event) in rows {
                if !seen_files.insert(file_id) {
                    continue;
                }
                let (folder, name) = split(&rel);
                let stem = stem_ext(name).0.to_owned();
                let key = (root_id, folder.to_lowercase(), stem.to_lowercase());
                let facts = Facts {
                    wall,
                    camera: camera_name(make, model),
                    place,
                    event,
                    trip: trip_name(conn, day, &mut trips)?,
                };
                let shot = by_key.entry(key).or_insert_with(|| Shot {
                    root_id,
                    folder: folder.to_owned(),
                    stem: stem.clone(),
                    facts: facts.clone(),
                    files: Vec::new(),
                    sidecars: Vec::new(),
                });
                // A photo's facts win over its clip's.
                if kind != 1 && shot.files.is_empty() {
                    shot.facts = facts;
                }
                shot.files.push((file_id, rel));
            }
        }
    }

    // Other library files with the same name in the same folder (the RAW of a selected
    // JPEG): `stem.` up to `stem/` covers every extension.
    let mut siblings = conn.prepare_cached(
        "SELECT id, rel_path FROM files WHERE root_id = ?1 AND missing_since IS NULL
           AND rel_path >= ?2 AND rel_path < ?3",
    )?;
    let mut roots: HashMap<i64, Option<std::path::PathBuf>> = HashMap::new();
    let mut listings: HashMap<(i64, String), Vec<String>> = HashMap::new();
    for shot in by_key.values_mut() {
        let base = join(&shot.folder, &shot.stem);
        let rows: Vec<(i64, String)> = siblings
            .query_map(
                params![shot.root_id, format!("{base}."), format!("{base}/")],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
            .collect::<rusqlite::Result<_>>()?;
        for (id, rel) in rows {
            if stem_ext(split(&rel).1).0 == shot.stem && seen_files.insert(id) {
                shot.files.push((id, rel));
            }
        }
        // Sidecars, from the folder on disk.
        let root = match roots.get(&shot.root_id) {
            Some(r) => r.clone(),
            None => {
                let r = library::root_path(conn, shot.root_id)?;
                roots.insert(shot.root_id, r.clone());
                r
            }
        };
        let Some(root) = root else { continue };
        let names = listings
            .entry((shot.root_id, shot.folder.clone()))
            .or_insert_with(|| {
                std::fs::read_dir(root.join(&shot.folder))
                    .map(|entries| {
                        entries
                            .flatten()
                            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            });
        let member_names: Vec<String> = shot
            .files
            .iter()
            .map(|(_, rel)| split(rel).1.to_lowercase())
            .collect();
        for name in names.iter() {
            let (stem, ext) = stem_ext(name);
            let ext = ext.trim_start_matches('.').to_ascii_lowercase();
            if !SIDECARS.contains(&ext.as_str()) {
                continue;
            }
            // IMG_1.xmp, or IMG_1.CR2.xmp (named after one file).
            let belongs = stem.eq_ignore_ascii_case(&shot.stem)
                || member_names.contains(&stem.to_lowercase());
            if belongs {
                shot.sidecars.push(join(&shot.folder, name));
            }
        }
    }
    let mut shots: Vec<Shot> = by_key.into_values().collect();
    shots.sort_by(|a, b| {
        a.facts
            .wall
            .cmp(&b.facts.wall)
            .then_with(|| a.folder.cmp(&b.folder))
            .then_with(|| a.stem.cmp(&b.stem))
    });
    Ok(shots)
}

/// The new name of `rel` in a shot renamed to `stem`: same folder, own extension. A
/// sidecar named after a whole file name (`IMG_1.CR2.xmp`) keeps that shape.
fn renamed(rel: &str, shot: &Shot, stem: &str) -> String {
    let (folder, name) = split(rel);
    let rest = &name[shot.stem.len().min(name.len())..];
    // Name starts with the old stem (case-insensitive by construction); keep the rest.
    let rest = if name.to_lowercase().starts_with(&shot.stem.to_lowercase()) {
        rest
    } else {
        stem_ext(name).1
    };
    join(folder, &format!("{stem}{rest}"))
}

/// Works out the new name of every file. Returns the library renames (file id → new
/// path), the sidecars, and the preview.
#[allow(clippy::type_complexity)]
fn plan(
    conn: &Connection,
    asset_ids: &[i64],
    pattern: &str,
) -> Result<(Vec<(i64, String)>, Vec<Companion>, Preview)> {
    validate(pattern)?;
    let shots = shots(conn, asset_ids)?;

    // Stems before numbering, then numbers in capture order within each name.
    let rendered: Vec<String> = shots
        .iter()
        .map(|s| {
            let name = render(pattern, &s.facts, &s.stem);
            if name.replace(COUNTER, "").trim().is_empty() {
                s.stem.clone()
            } else {
                name
            }
        })
        .collect();
    let mut groups: HashMap<&str, usize> = HashMap::new();
    for name in &rendered {
        *groups.entry(name.as_str()).or_default() += 1;
    }
    let mut next: HashMap<&str, usize> = HashMap::new();
    let wanted: Vec<String> = rendered
        .iter()
        .map(|name| {
            if !name.contains(COUNTER) {
                return name.clone();
            }
            let n = next.entry(name.as_str()).or_insert(0);
            *n += 1;
            let width = groups[name.as_str()].to_string().len().max(3);
            tidy(&name.replace(COUNTER, &format!("{:0width$}", *n)))
        })
        .collect();

    // Every name in the folders involved is taken, except those of files this batch
    // renames (they'll be free by the time they're needed).
    let leaving: HashSet<(i64, String)> = shots
        .iter()
        .flat_map(|s| {
            s.files
                .iter()
                .map(|(_, r)| r.clone())
                .chain(s.sidecars.iter().cloned())
                .map(move |r| (s.root_id, r.to_lowercase()))
        })
        .collect();
    let mut taken: HashSet<(i64, String)> = HashSet::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT rel_path FROM files WHERE root_id = ?1 AND rel_path >= ?2 AND rel_path < ?3",
        )?;
        let folders: HashSet<(i64, String)> = shots
            .iter()
            .map(|s| (s.root_id, s.folder.clone()))
            .collect();
        for (root_id, folder) in folders {
            let (lo, hi) = if folder.is_empty() {
                (String::new(), "\u{10FFFF}".to_owned())
            } else {
                (format!("{folder}/"), format!("{folder}0"))
            };
            for rel in stmt.query_map(params![root_id, lo, hi], |r| r.get::<_, String>(0))? {
                let rel = rel?;
                // Only this folder, not its subfolders.
                if split(&rel).0 == folder {
                    taken.insert((root_id, rel.to_lowercase()));
                }
            }
            if let Some(root) = library::root_path(conn, root_id)?
                && let Ok(entries) = std::fs::read_dir(root.join(&folder))
            {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    taken.insert((root_id, join(&folder, &name).to_lowercase()));
                }
            }
        }
    }
    taken.retain(|k| !leaving.contains(k));

    let mut preview = Preview::default();
    // (root, from, to, file id or None for a sidecar), in planning order.
    let mut planned: Vec<(i64, String, String, Option<i64>)> = Vec::new();
    for (shot, stem) in shots.iter().zip(&wanted) {
        let targets = |stem: &str| -> Vec<(i64, String, String, Option<i64>)> {
            shot.files
                .iter()
                .map(|(id, rel)| {
                    (
                        shot.root_id,
                        rel.clone(),
                        renamed(rel, shot, stem),
                        Some(*id),
                    )
                })
                .chain(
                    shot.sidecars
                        .iter()
                        .map(|rel| (shot.root_id, rel.clone(), renamed(rel, shot, stem), None)),
                )
                .collect()
        };
        let mut chosen = targets(stem);
        if chosen.iter().all(|(_, from, to, _)| from == to) {
            preview.in_place += 1;
            continue;
        }
        let free = |list: &[(i64, String, String, Option<i64>)], taken: &HashSet<(i64, String)>| {
            list.iter().all(|(root, from, to, _)| {
                from.eq_ignore_ascii_case(to) || !taken.contains(&(*root, to.to_lowercase()))
            })
        };
        if !free(&chosen, &taken) {
            chosen = (2..)
                .map(|n| targets(&format!("{stem} ({n})")))
                .find(|c| free(c, &taken))
                .expect("some number is free");
            preview.numbered += 1;
        }
        for (root, from, to, id) in chosen {
            taken.insert((root, to.to_lowercase()));
            // A change of letter case only is left alone: the same name to Windows.
            if !from.eq_ignore_ascii_case(&to) {
                planned.push((root, from, to, id));
            }
        }
    }

    // Order so a file renamed into a name another file is leaving goes after it; in a
    // cycle (A→B, B→A) the move is impossible as planned, so it is left out.
    let by_from: HashMap<(i64, String), usize> = planned
        .iter()
        .enumerate()
        .map(|(i, (root, from, _, _))| ((*root, from.to_lowercase()), i))
        .collect();
    let mut order = Vec::with_capacity(planned.len());
    let mut state = vec![0u8; planned.len()]; // 0 new, 1 visiting, 2 placed, 3 dropped
    fn visit(
        i: usize,
        planned: &[(i64, String, String, Option<i64>)],
        by_from: &HashMap<(i64, String), usize>,
        state: &mut [u8],
        order: &mut Vec<usize>,
    ) -> bool {
        match state[i] {
            2 => return true,
            1 | 3 => return false,
            _ => {}
        }
        state[i] = 1;
        let (root, _, to, _) = &planned[i];
        if let Some(&j) = by_from.get(&(*root, to.to_lowercase()))
            && j != i
            && !visit(j, planned, by_from, state, order)
        {
            state[i] = 3;
            return false;
        }
        state[i] = 2;
        order.push(i);
        true
    }
    for i in 0..planned.len() {
        visit(i, &planned, &by_from, &mut state, &mut order);
    }

    let mut moves = Vec::new();
    let mut companions = Vec::new();
    for i in order {
        let (root_id, from, to, id) = &planned[i];
        if preview.sample.len() < SAMPLE {
            preview.sample.push(Rename {
                from: from.clone(),
                to: to.clone(),
            });
        }
        match id {
            Some(id) => moves.push((*id, to.clone())),
            None => companions.push(Companion {
                root_id: *root_id,
                from: from.clone(),
                to: to.clone(),
            }),
        }
    }
    preview.files = moves.len() + companions.len();
    preview.sidecars = companions.len();
    Ok((moves, companions, preview))
}

/// What renaming would do, without touching anything.
pub fn preview(conn: &Connection, asset_ids: &[i64], pattern: &str) -> Result<Preview> {
    Ok(plan(conn, asset_ids, pattern)?.2)
}

/// Renames the files (journaled, undoable). Plans again first, so it acts on the
/// library as it is now.
pub fn run(db: &Db, asset_ids: &[i64], pattern: &str) -> Result<Report> {
    let (moves, companions, _) = plan(&db.read(), asset_ids, pattern)?;
    if moves.is_empty() && companions.is_empty() {
        return Err(Error::Invalid(
            "These photos already have those names".into(),
        ));
    }
    fileops::relocate_with(db, &moves, &companions, "rename")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileops::Kind;

    const JUNE_14_2023_10_15: i64 = 1_686_737_700_000; // 10:15:00 UTC

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Db,
        photos: std::path::PathBuf,
    }

    /// Files (path, asset key); files with the same key share an asset. Each asset is
    /// taken a minute after the previous one.
    fn fixture(files: &[(&str, i64)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            2,
            crate::schema::migrate_library,
        )
        .unwrap();
        let root = library::add_root(&db.write(), &photos).unwrap();
        {
            let conn = db.write();
            for (rel, key) in files {
                let path = photos.join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, rel.as_bytes()).unwrap();
                let md = std::fs::metadata(&path).unwrap();
                let mtime = md
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;
                conn.execute(
                    "INSERT OR IGNORE INTO assets (id, media_kind, taken_at, taken_src, local_day,
                         camera_make, camera_model, created_at)
                     VALUES (?1, 0, ?2, 0, 0, 'Canon', 'Canon EOS R6', 0)",
                    params![key, JUNE_14_2023_10_15 + key * 60_000],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO files (root_id, rel_path, size, mtime, quick_hash, asset_id)
                     VALUES (?1, ?2, ?3, ?4, x'00', ?5)",
                    params![root, rel, md.len() as i64, mtime, key],
                )
                .unwrap();
            }
        }
        Fixture {
            _dir: dir,
            db,
            photos,
        }
    }

    fn names(f: &Fixture, folder: &str) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(f.photos.join(folder))
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().unwrap().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn patterns_render_and_tidy_up() {
        let facts = Facts {
            wall: JUNE_14_2023_10_15,
            camera: Some("EOS R6".into()),
            place: Some("Paris".into()),
            ..Default::default()
        };
        assert_eq!(
            render("{date} {time}", &facts, "IMG_1"),
            "2023-06-14 10.15.00"
        );
        assert_eq!(
            render("{date} {place}", &facts, "IMG_1"),
            "2023-06-14 Paris"
        );
        // No event: no dangling separator.
        assert_eq!(render("{date} - {event}", &facts, "x"), "2023-06-14");
        assert_eq!(
            render("{year}/{month} {original}", &facts, "IMG_1"),
            "2023-06 IMG_1"
        );
        assert_eq!(
            camera_name(Some("Canon".into()), Some("Canon EOS R6".into())).as_deref(),
            Some("Canon EOS R6")
        );
        assert_eq!(
            camera_name(Some("NIKON CORPORATION".into()), Some("Z 6".into())).as_deref(),
            Some("NIKON Z 6")
        );
        assert!(validate("{date} {nope}").is_err());
        assert!(validate("{date").is_err());
        assert_eq!(tidy("con"), "con_");
    }

    #[test]
    fn renames_pairs_and_sidecars_together_and_undoes() {
        let f = fixture(&[
            ("2023/IMG_0001.JPG", 1),
            ("2023/IMG_0001.CR2", 2),
            ("2023/IMG_0002.JPG", 3),
            ("2023/other.jpg", 9),
        ]);
        std::fs::write(f.photos.join("2023/IMG_0001.xmp"), b"x").unwrap();
        std::fs::write(f.photos.join("2023/IMG_0001.CR2.xmp"), b"x").unwrap();
        let before = names(&f, "2023");

        // Only the JPEG of the pair and the other photo are selected.
        let pattern = "{date} {counter}";
        let p = preview(&f.db.read(), &[1, 3], pattern).unwrap();
        assert_eq!((p.files, p.sidecars), (5, 2), "{p:?}");
        let report = run(&f.db, &[1, 3], pattern).unwrap();
        assert_eq!((report.done, report.failed), (5, 0), "{:?}", report.errors);
        assert_eq!(
            names(&f, "2023"),
            vec![
                "2023-06-14 001.CR2",
                "2023-06-14 001.CR2.xmp",
                "2023-06-14 001.JPG",
                "2023-06-14 001.xmp",
                "2023-06-14 002.JPG",
                "other.jpg",
            ]
        );
        // The library follows the files.
        let rel: String =
            f.db.read()
                .query_row("SELECT rel_path FROM files WHERE asset_id = 3", [], |r| {
                    r.get(0)
                })
                .unwrap();
        assert_eq!(rel, "2023/2023-06-14 002.JPG");

        fileops::undo(&f.db, report.op_id).unwrap();
        assert_eq!(names(&f, "2023"), before);
        assert_eq!(fileops::list(&f.db.read(), Kind::Move, 5).unwrap().len(), 1);
    }

    #[test]
    fn taken_names_get_numbers_and_renumbering_works() {
        let f = fixture(&[("a.jpg", 1), ("b.jpg", 2), ("2023-06-14 Paris.jpg", 7)]);
        // Both would be "2023-06-14 Paris"; one name is taken by a photo not renamed.
        f.db.write()
            .execute_batch(
                "INSERT INTO places (id, name, region, country, country_code)
                 VALUES (1, 'Paris', NULL, 'France', 'FR');
                 UPDATE assets SET place_id = 1;",
            )
            .unwrap();
        run(&f.db, &[1, 2], "{date} {place}").unwrap();
        assert_eq!(
            names(&f, ""),
            vec![
                "2023-06-14 Paris (2).jpg",
                "2023-06-14 Paris (3).jpg",
                "2023-06-14 Paris.jpg"
            ]
        );

        // Numbered names, then numbered again after the first photo was removed: each
        // file can move into the name the next one is leaving.
        let f = fixture(&[("a.jpg", 1), ("b.jpg", 2), ("c.jpg", 3)]);
        run(&f.db, &[1, 2, 3], "trip {counter}").unwrap();
        assert_eq!(
            names(&f, ""),
            vec!["trip 001.jpg", "trip 002.jpg", "trip 003.jpg"]
        );
        let first: i64 =
            f.db.read()
                .query_row("SELECT id FROM files WHERE asset_id = 1", [], |r| r.get(0))
                .unwrap();
        fileops::trash(&f.db, &[first], "test", false).unwrap();
        let report = run(&f.db, &[2, 3], "trip {counter}").unwrap();
        assert_eq!(report.failed, 0, "{:?}", report.errors);
        assert_eq!(names(&f, ""), vec!["trip 001.jpg", "trip 002.jpg"]);

        // Already named: nothing to do.
        assert!(run(&f.db, &[2, 3], "trip {counter}").is_err());
    }
}
