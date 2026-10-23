//! Library scanner: discover → extract metadata + quick hash (parallel) → commit in
//! batches → finalize (missing files, timeline buckets).
//!
//! Resuming is implicit: committed files are skipped as unchanged on the next scan.
//! Safety rule: files are only marked missing after a complete, error-free walk of a
//! mounted root. An unplugged drive or unreadable folder never looks like a deletion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use jwalk::WalkDir;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::db::Db;
use crate::media::{self, MediaKind, Metadata};
use crate::{Error, Result, hash, library, now_ms, thumbs, timeline};

const CHUNK: usize = 256;
const BUCKET_REFRESH: Duration = Duration::from_secs(5);
const EXCLUDED_DIRS: &[&str] = &[
    "$RECYCLE.BIN",
    "System Volume Information",
    "@eaDir",
    "#recycle",
    "lost+found",
    "node_modules",
];

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Discovering,
    Processing,
    Finalizing,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    pub discovered: u64,
    pub to_process: u64,
    pub processed: u64,
    pub errors: u64,
}

#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub discovered: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub missing: u64,
    pub errors: u64,
    /// False if cancelled. Missing-file detection only runs on complete scans.
    pub completed: bool,
    /// True if the walk hit unreadable folders, so missing-file detection was skipped.
    pub missing_check_skipped: bool,
}

struct Existing {
    id: i64,
    size: i64,
    mtime: i64,
    asset_id: Option<i64>,
}

struct Work {
    rel: String,
    abs: PathBuf,
    size: i64,
    mtime: i64,
    kind: MediaKind,
    /// (file id, asset id) when this path is already indexed but changed.
    existing: Option<(i64, Option<i64>)>,
}

/// (file id, asset id, full hash if known) of a file that may hold the same content.
type Candidate = (i64, i64, Option<[u8; 32]>);

struct Prepared {
    work: Work,
    meta: Metadata,
    quick: Option<[u8; 32]>,
    full: Option<[u8; 32]>,
    /// Existing asset this file is a copy or moved version of.
    link_asset: Option<i64>,
    /// A full hash computed for another file while checking for a match.
    candidate_full: Option<(i64, [u8; 32])>,
    thumb: Option<Vec<u8>>,
}

pub struct Scanner<'a> {
    pub library: &'a Db,
    pub thumbs: &'a Db,
    pub cancel: &'a AtomicBool,
}

impl Scanner<'_> {
    pub fn scan_root(&self, root_id: i64, progress: &dyn Fn(&Progress)) -> Result<Summary> {
        library::refresh_volumes(&self.library.write())?;
        let root = library::root_path(&self.library.read(), root_id)?
            .ok_or_else(|| Error::Invalid("This folder's drive is not connected".into()))?;

        let mut summary = Summary::default();
        let mut report = Progress {
            phase: Phase::Discovering,
            discovered: 0,
            to_process: 0,
            processed: 0,
            errors: 0,
        };
        progress(&report);

        // Discover.
        let existing = load_existing(&self.library.read(), root_id)?;
        let mut seen: Vec<i64> = Vec::with_capacity(existing.len());
        let mut work: Vec<Work> = Vec::new();
        let mut walk_errors = 0u64;

        let walker = WalkDir::new(&root)
            .skip_hidden(true)
            .process_read_dir(|_, _, _, children| {
                children.retain(|child| {
                    child.as_ref().map_or(true, |e| {
                        !(e.file_type().is_dir()
                            && EXCLUDED_DIRS
                                .iter()
                                .any(|x| e.file_name().eq_ignore_ascii_case(x)))
                    })
                });
            });
        for entry in walker {
            if self.cancelled() {
                return self.stop_early(summary);
            }
            let Ok(entry) = entry else {
                walk_errors += 1;
                continue;
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(kind) = media::kind_of(&path) else {
                continue;
            };
            let (Ok(md), Some(rel)) = (entry.metadata(), rel_path(&root, &path)) else {
                walk_errors += 1;
                continue;
            };
            let size = md.len() as i64;
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);

            summary.discovered += 1;
            match existing.get(&rel) {
                Some(e) if e.size == size && e.mtime == mtime && e.asset_id.is_some() => {
                    seen.push(e.id);
                    summary.unchanged += 1;
                }
                other => work.push(Work {
                    rel,
                    abs: path,
                    size,
                    mtime,
                    kind,
                    existing: other.map(|e| (e.id, e.asset_id)),
                }),
            }
            if summary.discovered % 1000 == 0 {
                report.discovered = summary.discovered;
                progress(&report);
            }
        }
        drop(existing);

        // Process.
        report.phase = Phase::Processing;
        report.discovered = summary.discovered;
        report.to_process = work.len() as u64;
        report.errors = walk_errors;
        progress(&report);

        let mut last_buckets = Instant::now();
        let mut pending = work.into_iter();
        loop {
            let chunk: Vec<Work> = pending.by_ref().take(CHUNK).collect();
            if chunk.is_empty() {
                break;
            }
            if self.cancelled() {
                return self.stop_early(summary);
            }
            let n = chunk.len() as u64;
            let prepared: Vec<Prepared> = chunk.into_par_iter().map(|w| self.prepare(w)).collect();
            self.commit(root_id, prepared, &mut summary, &mut seen)?;

            report.processed += n;
            report.errors = walk_errors + summary.errors;
            progress(&report);
            if last_buckets.elapsed() >= BUCKET_REFRESH {
                timeline::rebuild_day_buckets(&self.library.write())?;
                last_buckets = Instant::now();
            }
        }

        // Finalize.
        report.phase = Phase::Finalizing;
        progress(&report);
        summary.errors += walk_errors;
        summary.completed = true;
        let still_online = library::root_path(&self.library.read(), root_id)?.is_some();
        if walk_errors == 0 && still_online {
            summary.missing = mark_missing(&mut self.library.write(), root_id, &seen)?;
        } else {
            summary.missing_check_skipped = true;
        }
        let library = self.library.write();
        library.execute(
            "UPDATE roots SET last_scan_at = ?2 WHERE id = ?1",
            params![root_id, now_ms()],
        )?;
        timeline::rebuild_day_buckets(&library)?;
        Ok(summary)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Keeps everything committed so far visible, without touching missing state.
    fn stop_early(&self, summary: Summary) -> Result<Summary> {
        timeline::rebuild_day_buckets(&self.library.write())?;
        Ok(summary)
    }

    /// Runs on rayon workers: file reads only, no writes.
    fn prepare(&self, work: Work) -> Prepared {
        let mut meta = media::extract(&work.abs, work.kind, work.mtime);
        let quick = hash::quick_hash(&work.abs, work.size as u64).ok();
        let mut full = None;
        let mut link_asset = None;
        let mut candidate_full = None;

        if let (None, Some(q)) = (work.existing, quick) {
            let candidates =
                find_candidates(&self.library.read(), work.size, &q).unwrap_or_default();
            for (file_id, asset_id, stored_full) in candidates {
                let Ok(path) = library::file_path(&self.library.read(), file_id) else {
                    continue;
                };
                // The other file is gone from disk: this is the same file, moved or renamed.
                // Same size and identical first 64 KiB is treated as the same content.
                let Some(path) = path.filter(|p| p.is_file()) else {
                    link_asset = Some(asset_id);
                    break;
                };
                let theirs = match stored_full {
                    Some(h) => h,
                    None => match hash::full_hash(&path) {
                        Ok(h) => {
                            candidate_full = Some((file_id, h));
                            h
                        }
                        Err(_) => continue,
                    },
                };
                if full.is_none() {
                    full = hash::full_hash(&work.abs).ok();
                }
                if full == Some(theirs) {
                    link_asset = Some(asset_id);
                    break;
                }
            }
        }

        let thumb = meta
            .embedded_thumb
            .take()
            .and_then(|jpeg| thumbs::from_embedded(&jpeg, meta.orientation).ok());
        Prepared {
            work,
            meta,
            quick,
            full,
            link_asset,
            candidate_full,
            thumb,
        }
    }

    fn commit(
        &self,
        root_id: i64,
        prepared: Vec<Prepared>,
        summary: &mut Summary,
        seen: &mut Vec<i64>,
    ) -> Result<()> {
        let mut new_thumbs: Vec<(i64, Vec<u8>)> = Vec::new();
        let mut stale_thumbs: Vec<i64> = Vec::new();
        {
            let mut library = self.library.write();
            let tx = library.transaction()?;
            for p in prepared {
                let Some(quick) = p.quick else {
                    // Unreadable right now: keep the existing row so it isn't marked missing.
                    summary.errors += 1;
                    if let Some((file_id, _)) = p.work.existing {
                        seen.push(file_id);
                    }
                    continue;
                };

                let asset_id = match (p.work.existing, p.link_asset) {
                    // Same path, changed content: update its asset unless copies share it.
                    (Some((file_id, Some(asset_id))), _) if !shared(&tx, asset_id, file_id)? => {
                        update_asset(&tx, asset_id, p.work.kind, &p.meta)?;
                        summary.updated += 1;
                        stale_thumbs.push(asset_id);
                        asset_id
                    }
                    (None, Some(asset_id)) => {
                        summary.added += 1;
                        asset_id
                    }
                    (existing, _) => {
                        if existing.is_some() {
                            summary.updated += 1
                        } else {
                            summary.added += 1
                        }
                        insert_asset(&tx, p.work.kind, &p.meta)?
                    }
                };
                tx.execute(
                    "UPDATE assets SET status = 0 WHERE id = ?1",
                    params![asset_id],
                )?;

                let file_id = match p.work.existing {
                    Some((file_id, _)) => {
                        tx.execute(
                            "UPDATE files SET size = ?2, mtime = ?3, quick_hash = ?4, full_hash = ?5,
                                 asset_id = ?6, missing_since = NULL
                             WHERE id = ?1",
                            params![file_id, p.work.size, p.work.mtime, &quick[..], p.full.as_ref().map(|h| &h[..]), asset_id],
                        )?;
                        file_id
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO files (root_id, rel_path, size, mtime, quick_hash, full_hash, asset_id)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![root_id, p.work.rel, p.work.size, p.work.mtime, &quick[..], p.full.as_ref().map(|h| &h[..]), asset_id],
                        )?;
                        tx.last_insert_rowid()
                    }
                };
                seen.push(file_id);

                if let Some((other, h)) = p.candidate_full {
                    tx.execute(
                        "UPDATE files SET full_hash = ?2 WHERE id = ?1",
                        params![other, &h[..]],
                    )?;
                }
                if p.link_asset.is_none()
                    && let Some(thumb) = p.thumb
                {
                    new_thumbs.push((asset_id, thumb));
                }
            }
            tx.commit()?;
        }

        if !new_thumbs.is_empty() || !stale_thumbs.is_empty() {
            let mut store = self.thumbs.write();
            let tx = store.transaction()?;
            for id in stale_thumbs {
                thumbs::delete(&tx, id)?;
            }
            for (id, data) in new_thumbs {
                thumbs::put(&tx, id, &data)?;
            }
            tx.commit()?;
        }
        Ok(())
    }
}

fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

fn load_existing(conn: &Connection, root_id: i64) -> Result<HashMap<String, Existing>> {
    let mut stmt =
        conn.prepare("SELECT id, rel_path, size, mtime, asset_id FROM files WHERE root_id = ?1")?;
    let rows = stmt.query_map(params![root_id], |r| {
        Ok((
            r.get::<_, String>(1)?,
            Existing {
                id: r.get(0)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                asset_id: r.get(4)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn find_candidates(conn: &Connection, size: i64, quick: &[u8; 32]) -> Result<Vec<Candidate>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, asset_id, full_hash FROM files
         WHERE size = ?1 AND quick_hash = ?2 AND asset_id IS NOT NULL LIMIT 4",
    )?;
    let rows = stmt.query_map(params![size, &quick[..]], |r| {
        let full: Option<Vec<u8>> = r.get(2)?;
        Ok((r.get(0)?, r.get(1)?, full.and_then(|f| f.try_into().ok())))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// True if another file row also points at this asset.
fn shared(conn: &Connection, asset_id: i64, file_id: i64) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM files WHERE asset_id = ?1 AND id != ?2 LIMIT 1")?
        .query_row(params![asset_id, file_id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn insert_asset(conn: &Connection, kind: MediaKind, m: &Metadata) -> Result<i64> {
    conn.prepare_cached(
        "INSERT INTO assets (media_kind, taken_at, taken_src, tz_offset_min, local_day, width, height,
             orientation, duration_ms, camera_make, camera_model, lat, lon, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        kind as u8,
        m.taken_at,
        m.taken_src.map(|s| s as u8),
        m.tz_offset_min,
        m.local_day(),
        m.width,
        m.height,
        m.orientation,
        m.duration_ms,
        m.camera_make,
        m.camera_model,
        m.lat,
        m.lon,
        now_ms(),
    ])?;
    Ok(conn.last_insert_rowid())
}

fn update_asset(conn: &Connection, id: i64, kind: MediaKind, m: &Metadata) -> Result<()> {
    conn.prepare_cached(
        "UPDATE assets SET media_kind = ?2, taken_at = ?3, taken_src = ?4, tz_offset_min = ?5,
             local_day = ?6, width = ?7, height = ?8, orientation = ?9, duration_ms = ?10,
             camera_make = ?11, camera_model = ?12, lat = ?13, lon = ?14, phash = NULL, blur = NULL
         WHERE id = ?1",
    )?
    .execute(params![
        id,
        kind as u8,
        m.taken_at,
        m.taken_src.map(|s| s as u8),
        m.tz_offset_min,
        m.local_day(),
        m.width,
        m.height,
        m.orientation,
        m.duration_ms,
        m.camera_make,
        m.camera_model,
        m.lat,
        m.lon,
    ])?;
    Ok(())
}

/// Marks files under the root that were not seen as missing, clears the flag on files
/// that came back, and hides assets whose every file is now confirmed missing.
fn mark_missing(conn: &mut Connection, root_id: i64, seen: &[i64]) -> Result<u64> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS seen_files (id INTEGER PRIMARY KEY);
         DELETE FROM temp.seen_files;",
    )?;
    {
        let mut insert = tx.prepare("INSERT OR IGNORE INTO temp.seen_files (id) VALUES (?1)")?;
        for id in seen {
            insert.execute(params![id])?;
        }
    }
    let missing = tx.execute(
        "UPDATE files SET missing_since = ?2
         WHERE root_id = ?1 AND missing_since IS NULL AND id NOT IN (SELECT id FROM temp.seen_files)",
        params![root_id, now_ms()],
    )?;
    tx.execute(
        "UPDATE files SET missing_since = NULL
         WHERE root_id = ?1 AND missing_since IS NOT NULL AND id IN (SELECT id FROM temp.seen_files)",
        params![root_id],
    )?;
    tx.execute(
        "UPDATE assets SET status = CASE WHEN EXISTS (
             SELECT 1 FROM files f WHERE f.asset_id = assets.id AND f.missing_since IS NULL
         ) THEN 0 ELSE 1 END
         WHERE id IN (SELECT asset_id FROM files WHERE root_id = ?1)",
        params![root_id],
    )?;
    tx.execute("DELETE FROM temp.seen_files", [])?;
    tx.commit()?;
    Ok(missing as u64)
}
