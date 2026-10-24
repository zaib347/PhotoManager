//! Importing what Google Photos (Takeout) and iCloud Photos (privacy.apple.com) exports
//! know but their files don't: capture dates and places lost from EXIF, favorites,
//! hidden photos, descriptions and albums.
//!
//! The export is added as a library folder and scanned like any other; an import then
//! reads its metadata files and applies them to the photos already indexed:
//!
//! - **Google Takeout**: a `.json` beside each file (`photoTakenTime`, `geoData`,
//!   `description`, `favorited`), named after the file in one of Google's shortened
//!   forms (`IMG_1.jpg.json`, `IMG_1.jpg.supplemental-metadata.json` cut to fit,
//!   `IMG_1.jpg(1).json` for `IMG_1(1).jpg`); `-edited` copies share their original's.
//!   Album folders have a `metadata.json` with the album's title.
//! - **iCloud**: `Photo Details.csv` files (`imgName`, `favorite`, `hidden`,
//!   `originalCreationDate`) and an `Albums` folder of `<album>.csv` file lists.
//!
//! Only blanks are filled: a date is taken only when the file had none of its own (it
//! was guessed from the file name or modified time), a place only when there's no GPS,
//! and favorites, hidden and descriptions are only ever added. Dates go through the
//! same correction as date fixes, so "Restore original date" and rescans treat them
//! alike. Every change is journaled, so a whole import can be undone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::db::Db;
use crate::{Error, Result, library, now_ms, timeline};

/// `assets.taken_src` for dates from an export's metadata.
pub const SRC_IMPORT: u8 = 5;
/// Date sources an import may replace: the file name or the modified time.
const GUESSED_SOURCES: [u8; 2] = [2, 3];
/// Suffixes Google adds to edited copies, by language.
const EDITED: &[&str] = &[
    "-edited",
    "-bearbeitet",
    "-modifié",
    "-editado",
    "-modificato",
    "-bewerkt",
];
/// Metadata files bigger than this aren't Google's.
const MAX_JSON: u64 = 1 << 20;

#[derive(Serialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub id: i64,
    /// Metadata entries found, and those matched to a file in the library.
    pub found: usize,
    pub matched: usize,
    pub dates: usize,
    pub places: usize,
    pub favorites: usize,
    pub hidden: usize,
    pub descriptions: usize,
    pub albums: usize,
    pub album_items: usize,
    /// "google", "icloud", or both.
    pub sources: Vec<String>,
}

impl Report {
    pub fn changed(&self) -> usize {
        self.dates
            + self.places
            + self.favorites
            + self.hidden
            + self.descriptions
            + self.album_items
    }
}

/// What the export says about one file.
#[derive(Debug, Default, Clone, PartialEq)]
struct Facts {
    taken_at: Option<i64>,
    place: Option<(f64, f64)>,
    favorite: bool,
    hidden: bool,
    description: Option<String>,
}

impl Facts {
    fn merge(&mut self, other: &Facts) {
        self.taken_at = self.taken_at.or(other.taken_at);
        self.place = self.place.or(other.place);
        self.favorite |= other.favorite;
        self.hidden |= other.hidden;
        if self.description.is_none() {
            self.description = other.description.clone();
        }
    }
}

/// Library files of a root, by folder and lower-case name.
struct Files {
    by_folder: HashMap<String, HashMap<String, (String, i64)>>,
    by_name: HashMap<String, Vec<i64>>,
}

impl Files {
    fn load(conn: &Connection, root_id: i64) -> Result<Files> {
        let mut stmt = conn.prepare(
            "SELECT rel_path, asset_id FROM files
             WHERE root_id = ?1 AND missing_since IS NULL AND asset_id IS NOT NULL",
        )?;
        let mut files = Files {
            by_folder: HashMap::new(),
            by_name: HashMap::new(),
        };
        for row in stmt.query_map(params![root_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get(1)?))
        })? {
            let (rel, asset): (String, i64) = row?;
            let (folder, name) = split(&rel);
            let lower = name.to_lowercase();
            files.by_name.entry(lower.clone()).or_default().push(asset);
            files
                .by_folder
                .entry(folder.to_owned())
                .or_default()
                .insert(lower, (name.to_owned(), asset));
        }
        Ok(files)
    }

    fn in_folder(&self, folder: &str, name: &str) -> Option<i64> {
        self.by_folder
            .get(folder)?
            .get(&name.to_lowercase())
            .map(|f| f.1)
    }
}

fn split(rel: &str) -> (&str, &str) {
    rel.rsplit_once('/').unwrap_or(("", rel))
}

fn stem_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `IMG_1.jpg` with counter 1 is `IMG_1(1).jpg`.
fn with_counter(name: &str, counter: Option<&str>) -> String {
    match counter {
        Some(n) => {
            let (stem, ext) = stem_ext(name);
            format!("{stem}({n}){ext}")
        }
        None => name.to_owned(),
    }
}

/// A trailing `(n)`, split off.
fn take_counter(s: &str) -> (&str, Option<&str>) {
    if let Some(open) = s
        .strip_suffix(')')
        .and_then(|t| t.rfind('(').map(|i| (t, i)))
        && open.0[open.1 + 1..].chars().all(|c| c.is_ascii_digit())
        && open.0.len() > open.1 + 1
    {
        return (&s[..open.1], Some(&open.0[open.1 + 1..]));
    }
    (s, None)
}

/// The names a Takeout JSON's media file may have, most likely first.
fn takeout_candidates(json_name: &str, title: Option<&str>) -> Vec<String> {
    let base = &json_name[..json_name.len() - ".json".len()];
    let (base, counter) = take_counter(base);
    let mut names = vec![with_counter(base, counter)];
    // `.supplemental-metadata`, cut short to fit Google's 51-character limit.
    if let Some(dot) = base.rfind('.') {
        let tail = &base[dot + 1..];
        if !tail.is_empty() && "supplemental-metadata".starts_with(&tail.to_lowercase()) {
            let (inner, inner_counter) = take_counter(&base[..dot]);
            names.push(with_counter(inner, counter.or(inner_counter)));
        }
    }
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        names.push(with_counter(t, counter));
    }
    names.dedup();
    names
}

fn takeout_facts(v: &Value) -> Option<Facts> {
    let seconds = |key: &str| -> Option<i64> {
        let t = &v[key]["timestamp"];
        t.as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| t.as_i64())
    };
    let taken_at = seconds("photoTakenTime").map(|s| s * 1000);
    // Without a capture time this isn't a photo's metadata (albums, comments…).
    taken_at?;
    let place = ["geoData", "geoDataExif"].iter().find_map(|key| {
        let g = &v[*key];
        let (lat, lon) = (g["latitude"].as_f64()?, g["longitude"].as_f64()?);
        (lat != 0.0 || lon != 0.0).then_some((lat, lon))
    });
    Some(Facts {
        taken_at,
        place,
        favorite: v["favorited"].as_bool().unwrap_or(false),
        hidden: false,
        description: v["description"]
            .as_str()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_owned),
    })
}

/// "Monday March 12,2019 10:15 AM GMT" (also without the weekday or the zone, and with
/// a space after the comma), as UTC ms.
pub fn parse_icloud_date(s: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let cleaned = s.replace(',', " ");
    let mut words: Vec<String> = cleaned.split_whitespace().map(str::to_lowercase).collect();
    if words.first().is_some_and(|w| w.ends_with("day")) {
        words.remove(0);
    }
    if words.last().is_some_and(|w| w == "gmt" || w == "utc") {
        words.pop();
    }
    let [month, day, year, time, ampm] = words.as_slice() else {
        return None;
    };
    let month = MONTHS
        .iter()
        .position(|m| m.starts_with(month.as_str()) && month.len() >= 3)? as u32
        + 1;
    let (day, year): (u32, i32) = (day.parse().ok()?, year.parse().ok()?);
    let (h, m) = time.split_once(':')?;
    let (mut hour, minute): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    match ampm.as_str() {
        "am" if hour == 12 => hour = 0,
        "pm" if hour < 12 => hour += 12,
        "am" | "pm" => {}
        _ => return None,
    }
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let at = date.and_hms_opt(hour, minute, 0)?;
    Some(at.and_utc().timestamp_millis())
}

/// Minimal CSV: commas, double quotes (with "" inside), CRLF or LF.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            ('"', true) => quoted = false,
            ('"', false) if field.is_empty() => quoted = true,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                if row.iter().any(|f| !f.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
            }
            (c, _) => field.push(c),
        }
    }
    row.push(field);
    if row.iter().any(|f| !f.is_empty()) {
        rows.push(row);
    }
    rows
}

/// Everything read from an export, before it touches the library.
#[derive(Default)]
struct Plan {
    facts: HashMap<i64, Facts>,
    /// Album name → assets.
    albums: Vec<(String, Vec<i64>)>,
    found: usize,
    matched: usize,
    google: bool,
    icloud: bool,
}

impl Plan {
    fn add(&mut self, asset: i64, facts: &Facts) {
        self.facts.entry(asset).or_default().merge(facts);
    }
}

fn read_small(path: &Path, max: u64) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    if md.len() > max {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Every metadata file under `root`: (relative path, absolute path).
fn metadata_files(root: &Path) -> Vec<(String, PathBuf)> {
    jwalk::WalkDir::new(root)
        .skip_hidden(true)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if ext != "json" && ext != "csv" {
                return None;
            }
            let rel = path.strip_prefix(root).ok()?;
            let parts: Option<Vec<&str>> =
                rel.components().map(|c| c.as_os_str().to_str()).collect();
            Some((parts?.join("/"), path))
        })
        .collect()
}

fn plan(root: &Path, files: &Files) -> Plan {
    let mut plan = Plan::default();
    for (rel, path) in metadata_files(root) {
        let (folder, name) = split(&rel);
        let lower = name.to_lowercase();
        if lower.ends_with(".json") {
            let Some(v) =
                read_small(&path, MAX_JSON).and_then(|t| serde_json::from_str::<Value>(&t).ok())
            else {
                continue;
            };
            if lower == "metadata.json" {
                takeout_album(&mut plan, files, folder, &v);
            } else if let Some(facts) = takeout_facts(&v) {
                plan.found += 1;
                plan.google = true;
                takeout_file(&mut plan, files, folder, name, v["title"].as_str(), &facts);
            }
        } else if lower.starts_with("photo details") {
            if let Some(text) = read_small(&path, 256 << 20) {
                icloud_details(&mut plan, files, folder, &text);
            }
        } else if (folder == "Albums" || folder.ends_with("/Albums"))
            && let Some(text) = read_small(&path, 64 << 20)
        {
            icloud_album(&mut plan, files, folder, stem_ext(name).0, &text);
        }
    }
    plan
}

fn takeout_file(
    plan: &mut Plan,
    files: &Files,
    folder: &str,
    json_name: &str,
    title: Option<&str>,
    facts: &Facts,
) {
    let candidates = takeout_candidates(json_name, title);
    let mut found = candidates
        .iter()
        .find_map(|n| files.in_folder(folder, n).map(|a| (n.clone(), a)));
    if found.is_none() {
        // Google also cuts long media names; the JSON's name starts like the file's.
        let short = candidates[0].to_lowercase();
        if short.chars().count() >= 30
            && let Some(list) = files.by_folder.get(folder)
        {
            let hits: Vec<&(String, i64)> = list
                .iter()
                .filter(|(n, _)| {
                    let stem = stem_ext(n).0;
                    n.starts_with(&short) || (stem.chars().count() >= 25 && short.starts_with(stem))
                })
                .map(|(_, f)| f)
                .collect();
            if let [only] = hits.as_slice() {
                found = Some((only.0.clone(), only.1));
            }
        }
    }
    let Some((name, asset)) = found else {
        return;
    };
    plan.matched += 1;
    plan.add(asset, facts);
    // Edited copies share their original's metadata.
    let (stem, ext) = stem_ext(&name);
    for suffix in EDITED {
        if let Some(edited) = files.in_folder(folder, &format!("{stem}{suffix}{ext}")) {
            plan.add(edited, facts);
        }
    }
}

fn takeout_album(plan: &mut Plan, files: &Files, folder: &str, v: &Value) {
    let Some(title) = v["title"].as_str().map(str::trim).filter(|t| !t.is_empty()) else {
        return;
    };
    // Year folders ("Photos from 2019") aren't albums.
    let folder_name = split(folder).1;
    if folder_name.starts_with("Photos from ")
        && folder_name[12..].chars().all(|c| c.is_ascii_digit())
    {
        return;
    }
    let Some(list) = files.by_folder.get(folder) else {
        return;
    };
    let mut assets: Vec<i64> = list.values().map(|f| f.1).collect();
    assets.sort_unstable();
    assets.dedup();
    if !assets.is_empty() {
        plan.google = true;
        plan.albums.push((title.to_owned(), assets));
    }
}

/// A file named `name`: in `folder` if there, else the only one in the library.
fn icloud_file(files: &Files, folder: &str, name: &str) -> Option<i64> {
    files.in_folder(folder, name).or_else(|| {
        match files.by_name.get(&name.to_lowercase())?.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    })
}

fn icloud_details(plan: &mut Plan, files: &Files, folder: &str, text: &str) {
    let rows = csv_rows(text);
    let Some((header, rows)) = rows.split_first() else {
        return;
    };
    let col = |name: &str| {
        header
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let Some(img) = col("imgName") else {
        return;
    };
    let (fav, hid, del, date) = (
        col("favorite"),
        col("hidden"),
        col("deleted"),
        col("originalCreationDate"),
    );
    plan.icloud = true;
    let yes = |row: &Vec<String>, c: Option<usize>| {
        c.and_then(|c| row.get(c))
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("yes"))
    };
    for row in rows {
        let Some(name) = row.get(img).map(|n| n.trim()).filter(|n| !n.is_empty()) else {
            continue;
        };
        if yes(row, del) {
            continue;
        }
        plan.found += 1;
        let Some(asset) = icloud_file(files, folder, name) else {
            continue;
        };
        plan.matched += 1;
        plan.add(
            asset,
            &Facts {
                taken_at: date
                    .and_then(|c| row.get(c))
                    .and_then(|d| parse_icloud_date(d)),
                favorite: yes(row, fav),
                hidden: yes(row, hid),
                ..Facts::default()
            },
        );
    }
}

fn icloud_album(plan: &mut Plan, files: &Files, folder: &str, name: &str, text: &str) {
    let rows = csv_rows(text);
    let Some((header, rows)) = rows.split_first() else {
        return;
    };
    if !header
        .first()
        .is_some_and(|h| h.trim().eq_ignore_ascii_case("images"))
    {
        return;
    }
    // Album lists sit beside the photo folders, not in them.
    let photos = format!("{}/Photos", split(folder).0);
    let photos = photos.trim_start_matches('/');
    let mut assets: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.first())
        .filter_map(|n| icloud_file(files, photos, n.trim()))
        .collect();
    assets.sort_unstable();
    assets.dedup();
    if !assets.is_empty() && !name.trim().is_empty() {
        plan.icloud = true;
        plan.albums.push((name.trim().to_owned(), assets));
    }
}

/// What a photo has now, before an import fills in its blanks.
struct Current {
    src: u8,
    lat: Option<f64>,
    favorite: bool,
    hidden: bool,
    note: Option<String>,
}

fn journal(tx: &Connection, import: i64, asset: i64, field: &str, album: i64) -> Result<()> {
    tx.prepare_cached(
        "INSERT OR IGNORE INTO import_items (import_id, asset_id, field, album_id) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![import, asset, field, album])?;
    Ok(())
}

/// Reads the export metadata under root `root_id` into the library. Returns what changed.
pub fn run(db: &Db, root_id: i64) -> Result<Report> {
    let root = library::root_path(&db.read(), root_id)?
        .ok_or_else(|| Error::Invalid("This folder's drive is not connected".into()))?;
    let files = Files::load(&db.read(), root_id)?;
    let plan = plan(&root, &files);
    if plan.found == 0 && plan.albums.is_empty() {
        return Err(Error::Invalid(
            "No Google Takeout or iCloud metadata was found in this folder".into(),
        ));
    }
    let mut report = Report {
        found: plan.found,
        matched: plan.matched,
        sources: [("google", plan.google), ("icloud", plan.icloud)]
            .iter()
            .filter(|s| s.1)
            .map(|s| s.0.to_owned())
            .collect(),
        ..Report::default()
    };
    let mut hide: Vec<i64> = Vec::new();
    {
        let mut conn = db.write();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO imports (root_id, created_at, report) VALUES (?1, ?2, '{}')",
            params![root_id, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        report.id = id;
        let mut ids: Vec<&i64> = plan.facts.keys().collect();
        ids.sort_unstable();
        for &asset in ids {
            let facts = &plan.facts[&asset];
            let current: Option<Current> = tx
                .prepare_cached(
                    "SELECT a.taken_src, a.lat, COALESCE(u.favorite, 0), COALESCE(u.hidden, 0), u.note
                     FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id WHERE a.id = ?1",
                )?
                .query_row(params![asset], |r| {
                    Ok(Current {
                        src: r.get(0)?,
                        lat: r.get(1)?,
                        favorite: r.get(2)?,
                        hidden: r.get(3)?,
                        note: r.get(4)?,
                    })
                })
                .optional()?;
            let Some(Current {
                src,
                lat,
                favorite,
                hidden,
                note,
            }) = current
            else {
                continue;
            };
            if let Some(taken) = facts.taken_at
                && GUESSED_SOURCES.contains(&src)
            {
                set_date(&tx, asset, taken)?;
                journal(&tx, id, asset, "date", 0)?;
                report.dates += 1;
            }
            if let Some((plat, plon)) = facts.place
                && lat.is_none()
                && (-90.0..=90.0).contains(&plat)
                && (-180.0..=180.0).contains(&plon)
            {
                tx.prepare_cached(
                    "UPDATE assets SET lat = ?2, lon = ?3, place_id = NULL WHERE id = ?1",
                )?
                .execute(params![asset, plat, plon])?;
                tx.prepare_cached(
                    "INSERT INTO user_meta (asset_id, location_set) VALUES (?1, 1)
                     ON CONFLICT (asset_id) DO UPDATE SET location_set = 1",
                )?
                .execute(params![asset])?;
                journal(&tx, id, asset, "location", 0)?;
                report.places += 1;
            }
            if facts.favorite && !favorite {
                tx.prepare_cached(
                    "INSERT INTO user_meta (asset_id, favorite) VALUES (?1, 1)
                     ON CONFLICT (asset_id) DO UPDATE SET favorite = 1",
                )?
                .execute(params![asset])?;
                journal(&tx, id, asset, "favorite", 0)?;
                report.favorites += 1;
            }
            if facts.hidden && !hidden {
                hide.push(asset);
                journal(&tx, id, asset, "hidden", 0)?;
                report.hidden += 1;
            }
            if let Some(text) = &facts.description
                && note.is_none()
            {
                tx.prepare_cached(
                    "INSERT INTO user_meta (asset_id, note) VALUES (?1, ?2)
                     ON CONFLICT (asset_id) DO UPDATE SET note = excluded.note",
                )?
                .execute(params![asset, text])?;
                journal(&tx, id, asset, "description", 0)?;
                report.descriptions += 1;
            }
        }
        for (name, assets) in &plan.albums {
            let existing: Option<i64> = tx
                .prepare_cached(
                    "SELECT id FROM albums WHERE name = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
                )?
                .query_row(params![name], |r| r.get(0))
                .optional()?;
            let album = match existing {
                Some(a) => a,
                None => {
                    tx.execute(
                        "INSERT INTO albums (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
                        params![name, now_ms()],
                    )?;
                    let a = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO import_albums (import_id, album_id) VALUES (?1, ?2)",
                        params![id, a],
                    )?;
                    report.albums += 1;
                    a
                }
            };
            for &asset in assets {
                let added = tx
                    .prepare_cached(
                        "INSERT OR IGNORE INTO album_assets (album_id, asset_id) VALUES (?1, ?2)",
                    )?
                    .execute(params![album, asset])?;
                if added > 0 {
                    journal(&tx, id, asset, "album", album)?;
                    report.album_items += 1;
                }
            }
        }
        tx.execute(
            "UPDATE imports SET report = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(&report).expect("plain data")],
        )?;
        tx.commit()?;
    }
    if !hide.is_empty() {
        crate::assets::set_hidden(&mut db.write(), &hide, true)?;
    }
    settle(db, report.dates > 0, report.places > 0)?;
    Ok(report)
}

/// Sets an imported capture date, keeping the extracted one for "Restore original
/// date" (and an undo), like a date fix.
fn set_date(tx: &Connection, asset: i64, taken_at: i64) -> Result<()> {
    let (old, old_tz, src): (i64, i32, u8) = tx
        .prepare_cached("SELECT taken_at, tz_offset_min, taken_src FROM assets WHERE id = ?1")?
        .query_row(params![asset], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    tx.prepare_cached(
        "INSERT INTO user_meta (asset_id, original_taken_at, original_tz_offset_min, original_taken_src, corrected_time)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (asset_id) DO UPDATE SET
             original_taken_at = COALESCE(original_taken_at, excluded.original_taken_at),
             original_tz_offset_min = COALESCE(original_tz_offset_min, excluded.original_tz_offset_min),
             original_taken_src = COALESCE(original_taken_src, excluded.original_taken_src),
             corrected_time = excluded.corrected_time",
    )?
    .execute(params![asset, old, old_tz, src, taken_at])?;
    // Exports give UTC; show it in this computer's time zone, as with videos.
    let tz = crate::media::local_offset_min(taken_at);
    tx.prepare_cached(
        "UPDATE assets SET taken_at = ?2, tz_offset_min = ?3, local_day = ?4, taken_src = ?5 WHERE id = ?1",
    )?
    .execute(params![asset, taken_at, tz, timeline::local_day(taken_at, tz), SRC_IMPORT])?;
    Ok(())
}

/// After dates or places change: day counts, places, events and trips.
fn settle(db: &Db, dates: bool, places: bool) -> Result<()> {
    if places {
        crate::places::assign(db)?;
    }
    if dates || places {
        let mut conn = db.write();
        timeline::rebuild_day_buckets(&conn)?;
        crate::events::detect(&mut conn)?;
        crate::trips::detect(&mut conn)?;
    }
    Ok(())
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportInfo {
    pub id: i64,
    pub root_id: i64,
    pub created_at: i64,
    pub undone: bool,
    pub report: Report,
}

/// Imports into root `root_id`, newest first.
pub fn list(conn: &Connection, root_id: i64) -> Result<Vec<ImportInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, root_id, created_at, undone, report FROM imports WHERE root_id = ?1 ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![root_id], |r| {
        let json: String = r.get(4)?;
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, json))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, root_id, created_at, undone, json): (i64, i64, i64, bool, String) = row?;
        out.push(ImportInfo {
            id,
            root_id,
            created_at,
            undone,
            report: parse_report(&json, id),
        });
    }
    Ok(out)
}

fn parse_report(json: &str, id: i64) -> Report {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    let n = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    Report {
        id,
        found: n("found"),
        matched: n("matched"),
        dates: n("dates"),
        places: n("places"),
        favorites: n("favorites"),
        hidden: n("hidden"),
        descriptions: n("descriptions"),
        albums: n("albums"),
        album_items: n("albumItems"),
        sources: v["sources"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Takes back everything an import changed that hasn't been changed since: dates still
/// from the import, places, favorites, hidden, descriptions and album additions.
/// Albums it created are removed once empty. Returns how many changes were undone.
pub fn undo(db: &Db, import_id: i64) -> Result<usize> {
    let items: Vec<(i64, String, i64)> = {
        let conn = db.read();
        let undone: Option<bool> = conn
            .query_row(
                "SELECT undone FROM imports WHERE id = ?1",
                params![import_id],
                |r| r.get(0),
            )
            .optional()?;
        match undone {
            None => return Err(Error::Invalid("No such import".into())),
            Some(true) => return Ok(0),
            Some(false) => {}
        }
        let mut stmt = conn
            .prepare("SELECT asset_id, field, album_id FROM import_items WHERE import_id = ?1")?;
        stmt.query_map(params![import_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut undone = 0;
    let mut unhide = Vec::new();
    let mut dates = Vec::new();
    let mut places = false;
    {
        let mut conn = db.write();
        let tx = conn.transaction()?;
        for (asset, field, album) in &items {
            match field.as_str() {
                "date" => {
                    let src: Option<u8> = tx
                        .query_row(
                            "SELECT taken_src FROM assets WHERE id = ?1",
                            params![asset],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if src == Some(SRC_IMPORT) {
                        dates.push(*asset);
                    }
                }
                "location" => {
                    tx.execute(
                        "UPDATE assets SET lat = NULL, lon = NULL, place_id = NULL WHERE id = ?1",
                        params![asset],
                    )?;
                    tx.execute(
                        "UPDATE user_meta SET location_set = 0 WHERE asset_id = ?1",
                        params![asset],
                    )?;
                    places = true;
                    undone += 1;
                }
                "favorite" => {
                    undone += tx.execute(
                        "UPDATE user_meta SET favorite = 0 WHERE asset_id = ?1",
                        params![asset],
                    )?;
                }
                "hidden" => unhide.push(*asset),
                "description" => {
                    undone += tx.execute(
                        "UPDATE user_meta SET note = NULL WHERE asset_id = ?1",
                        params![asset],
                    )?;
                }
                "album" => {
                    undone += tx.execute(
                        "DELETE FROM album_assets WHERE album_id = ?1 AND asset_id = ?2",
                        params![album, asset],
                    )?;
                }
                _ => {}
            }
        }
        tx.execute(
            "DELETE FROM albums WHERE id IN (SELECT album_id FROM import_albums WHERE import_id = ?1)
               AND NOT EXISTS (SELECT 1 FROM album_assets WHERE album_id = albums.id)",
            params![import_id],
        )?;
        tx.execute(
            "UPDATE imports SET undone = 1 WHERE id = ?1",
            params![import_id],
        )?;
        tx.commit()?;
    }
    if !unhide.is_empty() {
        crate::assets::set_hidden(&mut db.write(), &unhide, false)?;
        undone += unhide.len();
    }
    if !dates.is_empty() {
        // Restoring rebuilds day counts, events and trips.
        undone += crate::dates::restore(&mut db.write(), &dates)?;
    }
    settle(db, false, places)?;
    Ok(undone)
}
