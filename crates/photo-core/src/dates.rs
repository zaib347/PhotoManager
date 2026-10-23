//! Correcting capture dates. Corrections shift times so a selection keeps its
//! internal order, remember the extracted date for undo, and survive rescans.

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::{Result, timeline};

const SRC_USER: u8 = 4;

/// Earliest and latest capture time of a set of assets, as wall-clock ms in each
/// photo's own time zone (what the user sees).
#[derive(Serialize, Debug, PartialEq)]
pub struct WallRange {
    pub first: i64,
    pub last: i64,
}

pub fn wall_range(conn: &Connection, ids: &[i64]) -> Result<Option<WallRange>> {
    let mut stmt =
        conn.prepare_cached("SELECT taken_at + tz_offset_min * 60000 FROM assets WHERE id = ?1")?;
    let mut range: Option<WallRange> = None;
    for &id in ids {
        let Ok(wall) = stmt.query_row(params![id], |r| r.get::<_, i64>(0)) else {
            continue;
        };
        range = Some(match range {
            None => WallRange {
                first: wall,
                last: wall,
            },
            Some(r) => WallRange {
                first: r.first.min(wall),
                last: r.last.max(wall),
            },
        });
    }
    Ok(range)
}

/// Moves every asset's capture time by `delta_ms`. Returns how many changed.
pub fn shift(conn: &mut Connection, ids: &[i64], delta_ms: i64) -> Result<usize> {
    if delta_ms == 0 {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    let mut changed = 0;
    {
        let mut read =
            tx.prepare("SELECT taken_at, tz_offset_min, taken_src FROM assets WHERE id = ?1")?;
        let mut keep_original = tx.prepare(
            "INSERT INTO user_meta (asset_id, original_taken_at, original_tz_offset_min, original_taken_src)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (asset_id) DO UPDATE SET
                 original_taken_at = COALESCE(original_taken_at, excluded.original_taken_at),
                 original_tz_offset_min = COALESCE(original_tz_offset_min, excluded.original_tz_offset_min),
                 original_taken_src = COALESCE(original_taken_src, excluded.original_taken_src)",
        )?;
        let mut write = tx.prepare(
            "UPDATE assets SET taken_at = ?2, local_day = ?3, taken_src = ?4 WHERE id = ?1",
        )?;
        let mut mark =
            tx.prepare("UPDATE user_meta SET corrected_time = ?2 WHERE asset_id = ?1")?;
        for &id in ids {
            let Ok((taken_at, tz, src)) = read.query_row(params![id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?, r.get::<_, u8>(2)?))
            }) else {
                continue;
            };
            keep_original.execute(params![id, taken_at, tz, src])?;
            let moved = taken_at + delta_ms;
            write.execute(params![id, moved, timeline::local_day(moved, tz), SRC_USER])?;
            mark.execute(params![id, moved])?;
            changed += 1;
        }
    }
    tx.commit()?;
    timeline::rebuild_day_buckets(conn)?;
    Ok(changed)
}

/// Puts back the dates read from the files. Returns how many were restored.
pub fn restore(conn: &mut Connection, ids: &[i64]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut restored = 0;
    {
        let mut read = tx.prepare(
            "SELECT original_taken_at, original_tz_offset_min, original_taken_src FROM user_meta
             WHERE asset_id = ?1 AND original_taken_at IS NOT NULL",
        )?;
        let mut write = tx.prepare(
            "UPDATE assets SET taken_at = ?2, tz_offset_min = ?3, taken_src = ?4, local_day = ?5
             WHERE id = ?1",
        )?;
        let mut clear = tx.prepare(
            "UPDATE user_meta SET corrected_time = NULL, original_taken_at = NULL,
                 original_tz_offset_min = NULL, original_taken_src = NULL
             WHERE asset_id = ?1",
        )?;
        for &id in ids {
            let Ok((taken_at, tz, src)) = read.query_row(params![id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?, r.get::<_, u8>(2)?))
            }) else {
                continue;
            };
            write.execute(params![
                id,
                taken_at,
                tz,
                src,
                timeline::local_day(taken_at, tz)
            ])?;
            clear.execute(params![id])?;
            restored += 1;
        }
    }
    tx.commit()?;
    timeline::rebuild_day_buckets(conn)?;
    Ok(restored)
}
