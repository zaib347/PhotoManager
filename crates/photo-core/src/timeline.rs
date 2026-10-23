use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::Result;

pub const DAY_MS: i64 = 86_400_000;

pub fn local_day(taken_at: i64, tz_offset_min: i32) -> i64 {
    (taken_at + tz_offset_min as i64 * 60_000).div_euclid(DAY_MS)
}

/// Which assets a timeline shows. `All` is served from precomputed day buckets;
/// the others are small or rarely used enough to aggregate on demand.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    All,
    Favorites,
    Videos,
    Hidden,
}

impl View {
    /// SQL condition over `assets a LEFT JOIN user_meta u`.
    fn condition(self) -> &'static str {
        match self {
            View::All => "a.status = 0 AND COALESCE(u.hidden, 0) = 0",
            View::Favorites => "a.status = 0 AND COALESCE(u.hidden, 0) = 0 AND u.favorite = 1",
            View::Videos => "a.status = 0 AND COALESCE(u.hidden, 0) = 0 AND a.media_kind = 1",
            View::Hidden => "a.status = 0 AND u.hidden = 1",
        }
    }
}

const FROM: &str = "FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id";

/// Every day that has assets, newest first. Parallel arrays keep the IPC payload small.
#[derive(Serialize)]
pub struct Layout {
    pub days: Vec<i64>,
    pub counts: Vec<u32>,
}

/// Assets of one day in display order, as parallel arrays.
#[derive(Serialize)]
pub struct DayAssets {
    pub day: i64,
    pub ids: Vec<i64>,
    pub kinds: Vec<u8>,
    pub favorites: Vec<u8>,
    pub durations: Vec<Option<i64>>,
}

#[derive(Serialize)]
pub struct Stats {
    pub assets: i64,
    pub days: i64,
}

pub fn layout(conn: &Connection, view: View) -> Result<Layout> {
    let sql = match view {
        View::All => "SELECT day, count FROM day_buckets ORDER BY day DESC".to_owned(),
        _ => format!(
            "SELECT a.local_day, count(*) {FROM} WHERE {} GROUP BY a.local_day ORDER BY a.local_day DESC",
            view.condition()
        ),
    };
    let mut stmt = conn.prepare_cached(&sql)?;
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
pub fn day_assets(conn: &Connection, view: View, days: &[i64]) -> Result<Vec<DayAssets>> {
    let sql = format!(
        "SELECT a.id, a.media_kind, COALESCE(u.favorite, 0), a.duration_ms {FROM}
         WHERE a.local_day = ?1 AND {} ORDER BY a.taken_at DESC, a.id DESC",
        view.condition()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    days.iter()
        .map(|&day| {
            let mut out = DayAssets {
                day,
                ids: Vec::new(),
                kinds: Vec::new(),
                favorites: Vec::new(),
                durations: Vec::new(),
            };
            let mut rows = stmt.query(params![day])?;
            while let Some(row) = rows.next()? {
                out.ids.push(row.get(0)?);
                out.kinds.push(row.get(1)?);
                out.favorites.push(row.get(2)?);
                out.durations.push(row.get(3)?);
            }
            Ok(out)
        })
        .collect()
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let sql = format!(
        "SELECT (SELECT count(*) {FROM} WHERE {}), (SELECT count(*) FROM day_buckets)",
        View::All.condition()
    );
    Ok(conn.query_row(&sql, [], |r| {
        Ok(Stats {
            assets: r.get(0)?,
            days: r.get(1)?,
        })
    })?)
}

pub fn rebuild_day_buckets(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "BEGIN;
         DELETE FROM day_buckets;
         INSERT INTO day_buckets (day, count)
             SELECT a.local_day, count(*) {FROM} WHERE {} GROUP BY a.local_day;
         COMMIT;",
        View::All.condition()
    ))?;
    Ok(())
}
