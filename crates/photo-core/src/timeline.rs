use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Result;

pub const DAY_MS: i64 = 86_400_000;

pub fn local_day(taken_at: i64, tz_offset_min: i32) -> i64 {
    (taken_at + tz_offset_min as i64 * 60_000).div_euclid(DAY_MS)
}

/// Every day that has assets, newest first. Parallel arrays keep the IPC payload small.
#[derive(Serialize)]
pub struct Layout {
    pub days: Vec<i64>,
    pub counts: Vec<u32>,
}

#[derive(Serialize)]
pub struct DayAssets {
    pub day: i64,
    pub ids: Vec<i64>,
    pub kinds: Vec<u8>,
}

#[derive(Serialize)]
pub struct Stats {
    pub assets: i64,
    pub days: i64,
}

pub fn layout(conn: &Connection) -> Result<Layout> {
    let mut stmt = conn.prepare_cached("SELECT day, count FROM day_buckets ORDER BY day DESC")?;
    let mut layout = Layout {
        days: Vec::new(),
        counts: Vec::new(),
    };
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        layout.days.push(row.get(0)?);
        layout.counts.push(row.get(1)?);
    }
    Ok(layout)
}

/// Asset ids for each requested day, newest first within the day.
pub fn day_assets(conn: &Connection, days: &[i64]) -> Result<Vec<DayAssets>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, media_kind FROM assets WHERE local_day = ?1 AND status = 0
         ORDER BY taken_at DESC, id DESC",
    )?;
    days.iter()
        .map(|&day| {
            let mut out = DayAssets {
                day,
                ids: Vec::new(),
                kinds: Vec::new(),
            };
            let mut rows = stmt.query(params![day])?;
            while let Some(row) = rows.next()? {
                out.ids.push(row.get(0)?);
                out.kinds.push(row.get(1)?);
            }
            Ok(out)
        })
        .collect()
}

pub fn media_kind(conn: &Connection, asset_id: i64) -> Result<Option<u8>> {
    let mut stmt = conn.prepare_cached("SELECT media_kind FROM assets WHERE id = ?1")?;
    let mut rows = stmt.query(params![asset_id])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    Ok(conn.query_row(
        "SELECT (SELECT count(*) FROM assets WHERE status = 0), (SELECT count(*) FROM day_buckets)",
        [],
        |r| {
            Ok(Stats {
                assets: r.get(0)?,
                days: r.get(1)?,
            })
        },
    )?)
}

pub fn rebuild_day_buckets(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
         DELETE FROM day_buckets;
         INSERT INTO day_buckets (day, count)
             SELECT local_day, count(*) FROM assets WHERE status = 0 GROUP BY local_day;
         COMMIT;",
    )?;
    Ok(())
}
