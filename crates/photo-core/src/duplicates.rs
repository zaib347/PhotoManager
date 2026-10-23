//! Exact duplicates: photos stored more than once.
//!
//! The scanner already joins identical files (same size, quick hash and, on a quick-hash
//! match, full hash) into one asset, so a duplicate is simply an asset with more than
//! one present file. Removing the extras goes through `fileops`, which keeps them
//! undoable and never takes the last copy.

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Result;
use crate::assets::{FileInfo, files_of};

/// Words in a path that suggest a copy rather than the original.
const COPY_HINTS: &[&str] = &[
    "copy",
    "kopie",
    "backup",
    "duplicate",
    "dupe",
    "download",
    "temp",
    "tmp",
    "whatsapp",
    "export",
    "trash",
];

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    /// Photos with more than one copy.
    pub groups: i64,
    /// Copies beyond the first, and the space they take.
    pub extra_files: i64,
    pub extra_bytes: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub asset_id: i64,
    pub media_kind: u8,
    pub taken_at: i64,
    pub tz_offset_min: i32,
    pub size: i64,
    /// Present copies, the suggested one to keep first.
    pub copies: Vec<FileInfo>,
}

/// Present files of duplicated assets: `(asset, copies, size)`.
const GROUPS_SQL: &str =
    "SELECT f.asset_id AS asset_id, count(*) AS n, max(f.size) AS size FROM files f
     JOIN assets a ON a.id = f.asset_id
     WHERE f.missing_since IS NULL AND a.status = 0
     GROUP BY f.asset_id HAVING count(*) > 1";

pub fn overview(conn: &Connection) -> Result<Overview> {
    Ok(conn.query_row(
        &format!(
            "SELECT count(*), COALESCE(sum(n - 1), 0), COALESCE(sum((n - 1) * size), 0)
             FROM ({GROUPS_SQL})"
        ),
        [],
        |r| {
            Ok(Overview {
                groups: r.get(0)?,
                extra_files: r.get(1)?,
                extra_bytes: r.get(2)?,
            })
        },
    )?)
}

/// A page of duplicated photos, most space wasted first.
pub fn groups(conn: &Connection, offset: usize, limit: usize) -> Result<Vec<Group>> {
    let page: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT asset_id, size FROM ({GROUPS_SQL})
             ORDER BY (n - 1) * size DESC, asset_id LIMIT ?1 OFFSET ?2"
        ))?;
        stmt.query_map(params![limit as i64, offset as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut asset = conn
        .prepare_cached("SELECT media_kind, taken_at, tz_offset_min FROM assets WHERE id = ?1")?;
    page.into_iter()
        .map(|(asset_id, size)| {
            let (media_kind, taken_at, tz_offset_min) =
                asset.query_row(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let mut copies: Vec<FileInfo> = files_of(conn, asset_id)?
                .into_iter()
                .filter(|f| !f.missing)
                .collect();
            copies.sort_by_key(keep_rank);
            Ok(Group {
                asset_id,
                media_kind,
                taken_at,
                tz_offset_min,
                size,
                copies,
            })
        })
        .collect()
}

/// True for names like "IMG_1234 (1).jpg", which is how Windows, browsers and phones
/// name a second copy. ("-2" endings are left alone: too many dates end that way.)
fn numbered_copy(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let Some((head, n)) = stem.strip_suffix(')').and_then(|s| s.rsplit_once('(')) else {
        return false;
    };
    !head.trim().is_empty() && (1..=2).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit())
}

/// Sort key for which copy to keep: reachable ones, then the fewest hints of being a
/// copy, then the oldest, then the shortest path.
fn keep_rank(f: &FileInfo) -> (bool, usize, i64, usize, i64) {
    let lower = f.path.to_lowercase();
    let hints =
        COPY_HINTS.iter().filter(|h| lower.contains(*h)).count() + numbered_copy(&lower) as usize;
    (!f.online, hints, f.modified_at, f.path.len(), f.id)
}

/// The copies to remove so each duplicated photo keeps one: the one in `keep`
/// (asset → file id) where given, else the suggested one. With `only`, just those
/// photos. Copies on drives that aren't connected are left out.
pub fn extras(
    conn: &Connection,
    only: Option<&[i64]>,
    keep: &HashMap<i64, i64>,
) -> Result<Vec<i64>> {
    let assets: Vec<i64> = match only {
        Some(ids) => ids.to_vec(),
        None => {
            let mut stmt = conn.prepare(&format!("SELECT asset_id FROM ({GROUPS_SQL})"))?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        }
    };
    let mut out = Vec::new();
    for asset in assets {
        let mut copies: Vec<FileInfo> = files_of(conn, asset)?
            .into_iter()
            .filter(|f| !f.missing)
            .collect();
        if copies.len() < 2 {
            continue;
        }
        copies.sort_by_key(keep_rank);
        let kept = keep
            .get(&asset)
            .copied()
            .filter(|id| copies.iter().any(|f| f.id == *id))
            .unwrap_or(copies[0].id);
        out.extend(
            copies
                .iter()
                .filter(|f| f.id != kept && f.online)
                .map(|f| f.id),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: i64, path: &str, modified_at: i64) -> FileInfo {
        FileInfo {
            id,
            path: path.into(),
            size: 1,
            modified_at,
            online: true,
            missing: false,
        }
    }

    #[test]
    fn spots_numbered_copies() {
        assert!(numbered_copy("D:/Photos/IMG_1234 (1).jpg"));
        assert!(numbered_copy("img_1234(12).JPG"));
        assert!(!numbered_copy("IMG_1234.jpg"));
        assert!(
            !numbered_copy("2024-05-01.jpg"),
            "dates aren't copy numbers"
        );
        assert!(!numbered_copy("(1).jpg"));
    }

    #[test]
    fn keeps_the_original_looking_copy() {
        let mut copies = [
            file(1, "D:/Backup/Photos/IMG_1.jpg", 100),
            file(2, "D:/Photos/2023/IMG_1 (1).jpg", 50),
            file(3, "D:/Photos/2023/IMG_1.jpg", 200),
            file(4, "D:/Downloads/IMG_1.jpg", 10),
        ];
        copies.sort_by_key(keep_rank);
        assert_eq!(copies[0].id, 3);
        // Offline copies are never preferred.
        copies[0].online = false;
        copies.sort_by_key(keep_rank);
        assert_ne!(copies[0].id, 3);
    }
}
