//! The organizer: moves a library folder's files into folders by date.
//!
//! It plans first (`preview` shows what would happen and changes nothing), then moves
//! through `fileops`, so every move is journaled and can be undone. Files stay inside
//! their own library folder and keep their names; a name already taken gets " (2)".
//! Files whose only date is the file's modified time are left where they are unless
//! asked for, since that date is often wrong. A Live Photo's clip follows its photo.

use std::collections::HashSet;

use chrono::{DateTime, Datelike};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::fileops::{self, Report};
use crate::{Error, Result, library};

/// `taken_src` for dates read from the file's modified time.
const SRC_MTIME: u8 = 3;
/// Moves listed in a preview.
const SAMPLE: usize = 100;

/// Folder layout under the library folder.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// 2023/2023-06
    #[default]
    YearMonth,
    /// 2023/06
    YearMonthNumber,
    /// 2023/2023-06-14
    YearDay,
}

impl Layout {
    fn folder(self, year: i32, month: u32, day: u32) -> String {
        match self {
            Layout::YearMonth => format!("{year:04}/{year:04}-{month:02}"),
            Layout::YearMonthNumber => format!("{year:04}/{month:02}"),
            Layout::YearDay => format!("{year:04}/{year:04}-{month:02}-{day:02}"),
        }
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    pub file_id: i64,
    /// Paths relative to the library folder, with `/`.
    pub from: String,
    pub to: String,
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// Files that would move.
    pub moves: usize,
    /// Files already where they belong.
    pub in_place: usize,
    /// Files left alone because their only date is the file's modified time.
    pub undated: usize,
    /// Moves that need " (2)" because the name is taken there.
    pub renamed: usize,
    /// The first few moves.
    pub sample: Vec<Move>,
}

/// Works out where each present file of a library folder belongs.
pub fn plan(
    conn: &Connection,
    root_id: i64,
    layout: Layout,
    include_undated: bool,
) -> Result<(Vec<Move>, Preview)> {
    // Every path in the folder is taken, including missing files' (their rows still
    // hold the name).
    let mut taken: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT rel_path FROM files WHERE root_id = ?1")?;
        stmt.query_map(params![root_id], |r| r.get::<_, String>(0))?
            .map(|p| p.map(|p| p.to_lowercase()))
            .collect::<rusqlite::Result<_>>()?
    };
    let files: Vec<(i64, String, i64, i32, u8)> = {
        // A Live Photo's clip takes its photo's date, so they land together.
        let mut stmt = conn.prepare(
            "SELECT f.id, f.rel_path, COALESCE(s.taken_at, a.taken_at),
                    COALESCE(s.tz_offset_min, a.tz_offset_min), COALESCE(s.taken_src, a.taken_src)
             FROM files f JOIN assets a ON a.id = f.asset_id
             LEFT JOIN assets s ON s.id = a.paired_with
             WHERE f.root_id = ?1 AND f.missing_since IS NULL
             ORDER BY f.rel_path",
        )?;
        stmt.query_map(params![root_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let mut preview = Preview::default();
    let mut moves = Vec::new();
    for (file_id, from, taken_at, tz, src) in files {
        if src == SRC_MTIME && !include_undated {
            preview.undated += 1;
            continue;
        }
        let Some(date) = DateTime::from_timestamp_millis(taken_at + tz as i64 * 60_000) else {
            preview.undated += 1;
            continue;
        };
        let date = date.date_naive();
        let folder = layout.folder(date.year(), date.month(), date.day());
        let name = from.rsplit('/').next().unwrap_or(&from);
        let current = from.rsplit_once('/').map_or("", |(dir, _)| dir);
        if current.eq_ignore_ascii_case(&folder) {
            preview.in_place += 1;
            continue;
        }
        let mut to = format!("{folder}/{name}");
        if taken.contains(&to.to_lowercase()) {
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
                _ => (name, String::new()),
            };
            to = (2..)
                .map(|n| format!("{folder}/{stem} ({n}){ext}"))
                .find(|candidate| !taken.contains(&candidate.to_lowercase()))
                .expect("some number is free");
            preview.renamed += 1;
        }
        taken.insert(to.to_lowercase());
        moves.push(Move { file_id, from, to });
    }
    preview.moves = moves.len();
    preview.sample = moves.iter().take(SAMPLE).cloned().collect();
    Ok((moves, preview))
}

/// What organizing would do, without touching anything.
pub fn preview(
    conn: &Connection,
    root_id: i64,
    layout: Layout,
    include_undated: bool,
) -> Result<Preview> {
    Ok(plan(conn, root_id, layout, include_undated)?.1)
}

/// Moves the files (journaled, undoable). Plans again first, so it acts on the
/// library as it is now.
pub fn run(db: &Db, root_id: i64, layout: Layout, include_undated: bool) -> Result<Report> {
    if library::root_path(&db.read(), root_id)?.is_none() {
        return Err(Error::Invalid("That folder's drive isn't connected".into()));
    }
    let (moves, _) = plan(&db.read(), root_id, layout, include_undated)?;
    if moves.is_empty() {
        return Err(Error::Invalid("Everything is already in place".into()));
    }
    let moves: Vec<(i64, String)> = moves.into_iter().map(|m| (m.file_id, m.to)).collect();
    fileops::relocate(db, &moves, "organize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileops::Kind;

    const JUNE_14_2023: i64 = 1_686_744_000_000; // 12:00 UTC

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Db,
        photos: std::path::PathBuf,
        root: i64,
    }

    /// Files with (path, capture time, taken_src).
    fn fixture(files: &[(&str, i64, u8)]) -> Fixture {
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
        let conn = db.write();
        for (path, taken_at, src) in files {
            let abs = photos.join(path);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, path.as_bytes()).unwrap();
            let md = std::fs::metadata(&abs).unwrap();
            let mtime = md
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            conn.execute(
                "INSERT INTO assets (media_kind, taken_at, taken_src, local_day, created_at)
                 VALUES (0, ?1, ?2, 0, 0)",
                params![taken_at, src],
            )
            .unwrap();
            let asset = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO files (root_id, rel_path, size, mtime, asset_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![root, path, md.len() as i64, mtime, asset],
            )
            .unwrap();
        }
        drop(conn);
        Fixture {
            _dir: dir,
            db,
            photos,
            root,
        }
    }

    #[test]
    fn plans_folders_by_date_and_avoids_name_clashes() {
        let f = fixture(&[
            ("dump/IMG_1.jpg", JUNE_14_2023, 0),
            ("other/IMG_1.jpg", JUNE_14_2023, 0), // same name, same month
            ("2023/2023-06/IMG_2.jpg", JUNE_14_2023, 0), // already in place
            ("dump/screenshot.png", JUNE_14_2023, 3), // only a file date
        ]);
        let (moves, preview) = plan(&f.db.read(), f.root, Layout::YearMonth, false).unwrap();
        assert_eq!(
            (
                preview.moves,
                preview.in_place,
                preview.undated,
                preview.renamed
            ),
            (2, 1, 1, 1)
        );
        let targets: Vec<&str> = moves.iter().map(|m| m.to.as_str()).collect();
        assert_eq!(
            targets,
            ["2023/2023-06/IMG_1.jpg", "2023/2023-06/IMG_1 (2).jpg"]
        );

        let (moves, _) = plan(&f.db.read(), f.root, Layout::YearDay, true).unwrap();
        assert_eq!(
            moves.len(),
            4,
            "undated included, and the day layout moves the in-place one too"
        );
        assert!(
            moves
                .iter()
                .any(|m| m.to == "2023/2023-06-14/screenshot.png")
        );
    }

    #[test]
    fn organizing_moves_files_and_undo_puts_them_back() {
        let f = fixture(&[
            ("dump/IMG_1.jpg", JUNE_14_2023, 0),
            ("dump/deep/IMG_2.jpg", JUNE_14_2023 + 400 * 86_400_000, 0),
        ]);
        let report = run(&f.db, f.root, Layout::YearMonth, false).unwrap();
        assert_eq!((report.done, report.failed), (2, 0));
        assert!(f.photos.join("2023/2023-06/IMG_1.jpg").is_file());
        assert!(f.photos.join("2024/2024-07/IMG_2.jpg").is_file());
        assert!(
            !f.photos.join("dump").exists(),
            "emptied folders are tidied away"
        );
        assert!(
            run(&f.db, f.root, Layout::YearMonth, false).is_err(),
            "nothing left to do"
        );

        fileops::undo(&f.db, report.op_id).unwrap();
        assert!(f.photos.join("dump/IMG_1.jpg").is_file());
        assert!(f.photos.join("dump/deep/IMG_2.jpg").is_file());
        assert!(!f.photos.join("2023").exists() && !f.photos.join("2024").exists());
        assert_eq!(
            fileops::list(&f.db.read(), Kind::Move, 10).unwrap()[0].status,
            "undone"
        );
    }
}
