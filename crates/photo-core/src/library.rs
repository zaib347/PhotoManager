//! Library roots, volume status, and resolving assets back to files on disk.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{Error, Result, now_ms, volume};

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    pub id: i64,
    pub path: String,
    pub volume_label: Option<String>,
    pub online: bool,
    pub last_scan_at: Option<i64>,
    pub files: i64,
}

pub fn add_root(conn: &Connection, path: &Path) -> Result<i64> {
    let (vol, rel) = volume::identify(path)?;
    conn.execute(
        "INSERT INTO volumes (uuid, label, last_mount_path, status, created_at)
         VALUES (?1, ?2, ?3, 'online', ?4)
         ON CONFLICT (uuid) DO UPDATE SET
             label = excluded.label, last_mount_path = excluded.last_mount_path, status = 'online'",
        params![vol.uuid, vol.label, vol.mount.to_string_lossy(), now_ms()],
    )?;
    let volume_id: i64 = conn.query_row(
        "SELECT id FROM volumes WHERE uuid = ?1",
        params![vol.uuid],
        |r| r.get(0),
    )?;

    // Nested roots would index the same files twice.
    let mut stmt = conn.prepare("SELECT rel_path FROM roots WHERE volume_id = ?1")?;
    let existing = stmt.query_map(params![volume_id], |r| r.get::<_, String>(0))?;
    for other in existing {
        let other = other?;
        let inside = |a: &str, b: &str| b.is_empty() || a == b || a.starts_with(&format!("{b}/"));
        if inside(&rel, &other) || inside(&other, &rel) {
            return Err(Error::Invalid(format!(
                "This folder overlaps a folder already in the library ({})",
                vol.mount.join(&other).display()
            )));
        }
    }

    conn.execute(
        "INSERT INTO roots (volume_id, rel_path) VALUES (?1, ?2)",
        params![volume_id, rel],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Removes a root, its files, and any assets that only existed under it.
/// Returns removed asset ids so callers can drop derived data (thumbnails).
pub fn remove_root(conn: &mut Connection, root_id: i64) -> Result<Vec<i64>> {
    let tx = conn.transaction()?;
    let orphans: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT f.asset_id FROM files f
             WHERE f.root_id = ?1 AND f.asset_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM files o WHERE o.asset_id = f.asset_id AND o.root_id != ?1)",
        )?;
        stmt.query_map(params![root_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.execute("DELETE FROM files WHERE root_id = ?1", params![root_id])?;
    {
        let mut delete = tx.prepare("DELETE FROM assets WHERE id = ?1")?;
        for id in &orphans {
            delete.execute(params![id])?;
        }
    }
    tx.execute("DELETE FROM roots WHERE id = ?1", params![root_id])?;
    tx.commit()?;
    crate::timeline::rebuild_day_buckets(conn)?;
    Ok(orphans)
}

/// Re-checks which volumes are mounted and where. Returns true if anything changed.
pub fn refresh_volumes(conn: &Connection) -> Result<bool> {
    let mounted = volume::mounted();
    let volumes: Vec<(i64, String, Option<String>, String)> = {
        let mut stmt = conn.prepare("SELECT id, uuid, last_mount_path, status FROM volumes")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut changed = false;
    for (id, uuid, last_mount, status) in volumes {
        let mount = if uuid.starts_with("win-serial:") {
            mounted
                .iter()
                .find(|v| v.uuid == uuid)
                .map(|v| v.mount.to_string_lossy().into_owned())
        } else {
            last_mount.clone().filter(|m| Path::new(m).is_dir())
        };
        let new_status = if mount.is_some() { "online" } else { "offline" };
        let new_mount = mount.or(last_mount.clone());
        if new_status != status || new_mount != last_mount {
            changed = true;
            conn.execute(
                "UPDATE volumes SET status = ?2, last_mount_path = ?3 WHERE id = ?1",
                params![id, new_status, new_mount],
            )?;
        }
    }
    Ok(changed)
}

/// Absolute path of a root if its volume is mounted and the folder exists.
pub fn root_path(conn: &Connection, root_id: i64) -> Result<Option<PathBuf>> {
    let row: Option<(Option<String>, String, String)> = conn
        .query_row(
            "SELECT v.last_mount_path, v.status, r.rel_path
             FROM roots r JOIN volumes v ON v.id = r.volume_id WHERE r.id = ?1",
            params![root_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.and_then(|(mount, status, rel)| {
        let path = PathBuf::from(mount?).join(rel);
        (status == "online" && path.is_dir()).then_some(path)
    }))
}

pub fn list_roots(conn: &Connection) -> Result<Vec<RootInfo>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, v.last_mount_path, r.rel_path, v.label, v.status, r.last_scan_at,
                (SELECT count(*) FROM files f WHERE f.root_id = r.id AND f.missing_since IS NULL)
         FROM roots r JOIN volumes v ON v.id = r.volume_id ORDER BY r.id",
    )?;
    let rows = stmt.query_map([], |r| {
        let mount: Option<String> = r.get(1)?;
        let rel: String = r.get(2)?;
        let status: String = r.get(4)?;
        let path = PathBuf::from(mount.unwrap_or_default()).join(rel);
        Ok(RootInfo {
            id: r.get(0)?,
            online: status == "online" && path.is_dir(),
            path: path.to_string_lossy().into_owned(),
            volume_label: r.get(3)?,
            last_scan_at: r.get(5)?,
            files: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const FILE_PATH_SQL: &str = "SELECT v.last_mount_path, r.rel_path, f.rel_path
     FROM files f JOIN roots r ON r.id = f.root_id JOIN volumes v ON v.id = r.volume_id";

fn join_path(mount: Option<String>, root_rel: String, file_rel: String) -> Option<PathBuf> {
    Some(PathBuf::from(mount?).join(root_rel).join(file_rel))
}

/// Absolute path of a file row, if its volume is online.
pub fn file_path(conn: &Connection, file_id: i64) -> Result<Option<PathBuf>> {
    let sql = format!("{FILE_PATH_SQL} WHERE f.id = ?1 AND v.status = 'online'");
    let row = conn
        .query_row(&sql, params![file_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    Ok(row.and_then(|(m, a, b)| join_path(m, a, b)))
}

pub enum AssetSource {
    /// A readable file for the asset.
    File {
        path: PathBuf,
        orientation: Option<u8>,
        kind: u8,
    },
    /// The asset has files, but none are reachable right now (drive offline).
    Unavailable { kind: u8 },
    /// The asset has no files at all (synthetic test data).
    NoFiles { kind: u8 },
}

pub fn asset_source(conn: &Connection, asset_id: i64) -> Result<Option<AssetSource>> {
    let asset: Option<(u8, Option<u8>)> = conn
        .prepare_cached("SELECT media_kind, orientation FROM assets WHERE id = ?1")?
        .query_row(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((kind, orientation)) = asset else {
        return Ok(None);
    };

    let sql = format!(
        "{FILE_PATH_SQL} WHERE f.asset_id = ?1 AND f.missing_since IS NULL AND v.status = 'online'"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let candidates =
        stmt.query_map(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    for row in candidates {
        let (m, a, b) = row?;
        if let Some(path) = join_path(m, a, b).filter(|p| p.is_file()) {
            return Ok(Some(AssetSource::File {
                path,
                orientation,
                kind,
            }));
        }
    }

    let has_files: bool = conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM files WHERE asset_id = ?1)")?
        .query_row(params![asset_id], |r| r.get(0))?;
    Ok(Some(if has_files {
        AssetSource::Unavailable { kind }
    } else {
        AssetSource::NoFiles { kind }
    }))
}

pub fn start_scan_job(conn: &Connection, root_id: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO jobs (kind, state, checkpoint_json, updated_at) VALUES ('scan', 'running', ?1, ?2)",
        params![format!("{{\"root_id\":{root_id}}}"), now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_job(conn: &Connection, job_id: i64, state: &str) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET state = ?2, updated_at = ?3 WHERE id = ?1",
        params![job_id, state, now_ms()],
    )?;
    Ok(())
}

/// Scan jobs left `running` by a crash or forced quit. Marks them interrupted and
/// returns their root ids so the caller can resume them. Resuming is just
/// rescanning: files already committed are skipped as unchanged.
pub fn take_interrupted_scans(conn: &Connection) -> Result<Vec<i64>> {
    let roots: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT json_extract(checkpoint_json, '$.root_id') FROM jobs
             WHERE kind = 'scan' AND state = 'running'
               AND json_extract(checkpoint_json, '$.root_id') IN (SELECT id FROM roots)",
        )?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    conn.execute(
        "UPDATE jobs SET state = 'interrupted', updated_at = ?1 WHERE kind = 'scan' AND state = 'running'",
        params![now_ms()],
    )?;
    Ok(roots)
}
