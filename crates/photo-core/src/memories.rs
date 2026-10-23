//! Resurfacing memories: "On this day" in earlier years.

use chrono::{DateTime, Datelike};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Result;
use crate::timeline::DAY_MS;

const COVERS: usize = 4;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DayMemory {
    pub day: i64,
    pub years_ago: i32,
    pub count: i64,
    /// Best picks for a preview: favorites and high ratings first, photos before videos.
    pub cover_ids: Vec<i64>,
}

fn year_month_day(day: i64) -> Option<(i32, u32, u32)> {
    let date = DateTime::from_timestamp_millis(day * DAY_MS)?.date_naive();
    Some((date.year(), date.month(), date.day()))
}

/// Days in earlier years that share `today`'s month and day, most recent first.
pub fn on_this_day(conn: &Connection, today: i64) -> Result<Vec<DayMemory>> {
    let Some((this_year, month, day)) = year_month_day(today) else {
        return Ok(Vec::new());
    };
    let days: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT day, count FROM day_buckets
             WHERE day < ?1 AND strftime('%m-%d', day * 86400, 'unixepoch') = ?2
             ORDER BY day DESC",
        )?;
        stmt.query_map(params![today, format!("{month:02}-{day:02}")], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let mut covers = conn.prepare_cached(
        "SELECT a.id FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id
         WHERE a.local_day = ?1 AND a.status = 0 AND a.paired_with IS NULL
           AND COALESCE(u.hidden, 0) = 0
         ORDER BY COALESCE(u.favorite, 0) DESC, COALESCE(u.rating, 0) DESC,
                  a.media_kind = 1, a.taken_at
         LIMIT ?2",
    )?;
    days.into_iter()
        .map(|(d, count)| {
            let year = year_month_day(d).map_or(this_year, |(y, _, _)| y);
            let cover_ids = covers
                .query_map(params![d, COVERS as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(DayMemory {
                day: d,
                years_ago: this_year - year,
                count,
                cover_ids,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{schema, synth, timeline};

    #[test]
    fn finds_same_calendar_day_in_earlier_years() {
        let mut conn = Connection::open_in_memory().unwrap();
        schema::migrate_library(&mut conn).unwrap();
        synth::generate(&mut conn, 20_000, 9).unwrap();

        // Pick a day that has photos and look a few years ahead of it.
        let layout = timeline::layout(&conn, timeline::View::All, &Default::default()).unwrap();
        let past = layout.days[layout.days.len() / 2];
        let (y, m, d) = year_month_day(past).unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(y + 3, m, d.min(28))
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
            / DAY_MS;

        let memories = on_this_day(&conn, today).unwrap();
        for mem in &memories {
            let (_, mm, dd) = year_month_day(mem.day).unwrap();
            assert_eq!((mm, dd), (m, d.min(28)));
            assert!(mem.years_ago > 0 && mem.day < today);
            assert!(!mem.cover_ids.is_empty() && mem.cover_ids.len() <= COVERS);
        }
        if d <= 28 {
            assert!(memories.iter().any(|m| m.day == past && m.years_ago == 3));
        }
    }
}
