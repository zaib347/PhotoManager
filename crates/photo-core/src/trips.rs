//! Trips: runs of days spent far from home.
//!
//! Home is the town with the most days of photos in each year; years with too little
//! GPS borrow the nearest year's home, so moving house is handled. A day is away when
//! most of its located photos are over `AWAY_KM` from that year's home. Away days form
//! one trip as long as at most `MAX_GAP_DAYS` days without located photos separate
//! them; a day of photos at home ends the trip.
//!
//! A trip is a range of local days, so it shows every photo of those days, located or
//! not. Detection reads only per-day place counts, which the place index covers.
//! Like events, trips the user renamed or removed are never redrawn.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Datelike};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::places::{Index, PLACE_COLUMNS, Place, place_row};
use crate::timeline::DAY_MS;
use crate::{Error, Result};

/// Further than this from home is away.
const AWAY_KM: f64 = 100.0;
/// Days with no located photos allowed inside a trip (a quiet day, a phone left behind).
const MAX_GAP_DAYS: i64 = 2;
/// Located photos taken away needed to call it a trip.
const MIN_AWAY_PHOTOS: i64 = 10;
/// Longer than this is living somewhere, not a trip.
const MAX_TRIP_DAYS: i64 = 90;
/// Days with located photos a year needs to judge its own home.
const MIN_HOME_DAYS: usize = 10;
/// Places named on a trip; the rest are counted.
const NAMED_PLACES: usize = 3;
const MAX_TITLE: usize = 200;

const SUGGESTED: &str = "suggested";
const ACCEPTED: &str = "accepted";
const REJECTED: &str = "rejected";

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TripSummary {
    pub id: i64,
    /// `None` until the user names it; the UI names it after `places`.
    pub title: Option<String>,
    /// Local days, inclusive.
    pub first_day: i64,
    pub last_day: i64,
    pub count: i64,
    pub cover_id: Option<i64>,
    /// The most photographed towns, in the order they were visited.
    pub places: Vec<Place>,
    /// Every town visited, and their countries in visit order.
    pub place_count: i64,
    pub countries: Vec<String>,
    /// Renamed by the user; detection leaves its days alone.
    pub edited: bool,
}

fn year_of(day: i64) -> i32 {
    DateTime::from_timestamp_millis(day * DAY_MS).map_or(1970, |d| d.year())
}

/// Located photos of one day: (place, count) pairs.
struct Day {
    day: i64,
    places: Vec<(i64, i64)>,
}

/// Home town for each year that has enough located days.
fn homes(days: &[Day]) -> BTreeMap<i32, i64> {
    let mut per_year: BTreeMap<i32, (usize, HashMap<i64, usize>)> = BTreeMap::new();
    for d in days {
        let (located, towns) = per_year.entry(year_of(d.day)).or_default();
        *located += 1;
        for &(place, _) in &d.places {
            *towns.entry(place).or_default() += 1;
        }
    }
    let mut out: BTreeMap<i32, i64> = per_year
        .iter()
        .filter(|(_, (located, _))| *located >= MIN_HOME_DAYS)
        .filter_map(|(&year, (_, towns))| Some((year, most_common(towns)?)))
        .collect();
    if out.is_empty() {
        // Too little GPS in any one year: one home for the whole library.
        let mut all: HashMap<i64, usize> = HashMap::new();
        for (_, towns) in per_year.values() {
            for (&place, &n) in towns {
                *all.entry(place).or_default() += n;
            }
        }
        if let Some(home) = most_common(&all) {
            out.insert(0, home);
        }
    }
    out
}

fn most_common(counts: &HashMap<i64, usize>) -> Option<i64> {
    counts
        .iter()
        .max_by_key(|&(&id, &n)| (n, std::cmp::Reverse(id)))
        .map(|(&id, _)| id)
}

/// The home of `year`, or of the nearest year that has one (earlier on ties).
fn home_in(homes: &BTreeMap<i32, i64>, year: i32) -> Option<i64> {
    let before = homes.range(..=year).next_back();
    let after = homes.range(year..).next();
    match (before, after) {
        (Some((&y1, &h1)), Some((&y2, &h2))) => Some(if year - y1 <= y2 - year { h1 } else { h2 }),
        (Some((_, &h)), None) | (None, Some((_, &h))) => Some(h),
        (None, None) => None,
    }
}

/// Located photos of a day split into (at home, away), plus the away towns.
struct Judged {
    day: i64,
    home: i64,
    away: i64,
    away_places: Vec<(i64, i64)>,
}

fn judge(days: Vec<Day>, homes: &BTreeMap<i32, i64>, index: &Index) -> Vec<Judged> {
    days.into_iter()
        .filter_map(|d| {
            let home = home_in(homes, year_of(d.day))?;
            let mut out = Judged {
                day: d.day,
                home: 0,
                away: 0,
                away_places: Vec::new(),
            };
            for (place, n) in d.places {
                match index.km_between(home, place) {
                    Some(km) if km > AWAY_KM => {
                        out.away += n;
                        out.away_places.push((place, n));
                    }
                    Some(_) => out.home += n,
                    None => {} // not in the bundled list (older data): no opinion
                }
            }
            Some(out)
        })
        .collect()
}

/// Day ranges that look like trips.
fn candidates(judged: &[Judged]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    // (first, last, located photos away)
    let mut open: Option<(i64, i64, i64)> = None;
    let mut close = |trip: Option<(i64, i64, i64)>| {
        if let Some((first, last, photos)) = trip
            && last > first
            && last - first < MAX_TRIP_DAYS
            && photos >= MIN_AWAY_PHOTOS
        {
            out.push((first, last));
        }
    };
    for d in judged {
        if d.away > d.home {
            open = match open {
                Some((first, last, n)) if d.day - last - 1 <= MAX_GAP_DAYS => {
                    Some((first, d.day, n + d.away))
                }
                other => {
                    close(other);
                    Some((d.day, d.day, d.away))
                }
            };
        } else if d.home > 0 {
            close(open.take());
        }
    }
    close(open);
    out
}

fn overlaps(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// True when photos have places but no trip was ever found (e.g. right after upgrading).
/// A library with no trips at all answers yes every time, which costs one quick
/// `detect` per start.
pub fn needs_detection(conn: &Connection) -> Result<bool> {
    let never: bool = conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM trips) AND EXISTS (SELECT 1 FROM assets WHERE place_id > 0)",
        [],
        |r| r.get(0),
    )?;
    Ok(never)
}

/// Redraws suggested trips over the whole library and refreshes every trip's count,
/// cover and towns. Run after places and events are up to date.
pub fn detect(conn: &mut Connection) -> Result<()> {
    detect_with(conn, Index::bundled())
}

fn detect_with(conn: &mut Connection, index: &Index) -> Result<()> {
    let tx = conn.transaction()?;
    let days: Vec<Day> = {
        let mut stmt = tx.prepare(
            "SELECT local_day, place_id, count(*) FROM assets
             WHERE place_id > 0 AND status = 0 AND paired_with IS NULL
             GROUP BY local_day, place_id ORDER BY local_day",
        )?;
        let mut rows = stmt.query([])?;
        let mut days: Vec<Day> = Vec::new();
        while let Some(r) = rows.next()? {
            let (day, place, n): (i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            match days.last_mut() {
                Some(d) if d.day == day => d.places.push((place, n)),
                _ => days.push(Day {
                    day,
                    places: vec![(place, n)],
                }),
            }
        }
        days
    };
    let homes = homes(&days);
    let judged = judge(days, &homes, index);

    let existing: Vec<(i64, (i64, i64), String)> = {
        let mut stmt = tx.prepare("SELECT id, first_day, last_day, status FROM trips")?;
        stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?), r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let fixed: Vec<(i64, i64)> = existing
        .iter()
        .filter(|e| e.2 != SUGGESTED)
        .map(|e| e.1)
        .collect();
    let mut previous: Vec<(i64, (i64, i64))> = existing
        .iter()
        .filter(|e| e.2 == SUGGESTED)
        .map(|e| (e.0, e.1))
        .collect();

    let mut kept: Vec<i64> = Vec::new();
    for range in candidates(&judged) {
        if fixed.iter().any(|&f| overlaps(f, range)) {
            continue;
        }
        // Keep the id of the suggested trip it overlaps most, so open views stay valid.
        let reuse = previous
            .iter()
            .enumerate()
            .filter(|(_, (_, r))| overlaps(*r, range))
            .max_by_key(|(_, (_, r))| r.1.min(range.1) - r.0.max(range.0))
            .map(|(i, _)| i);
        match reuse {
            Some(i) => {
                let (id, _) = previous.swap_remove(i);
                tx.execute(
                    "UPDATE trips SET first_day = ?2, last_day = ?3 WHERE id = ?1",
                    params![id, range.0, range.1],
                )?;
                kept.push(id);
            }
            None => {
                tx.execute(
                    "INSERT INTO trips (first_day, last_day, status) VALUES (?1, ?2, ?3)",
                    params![range.0, range.1, SUGGESTED],
                )?;
                kept.push(tx.last_insert_rowid());
            }
        }
    }
    for (id, _) in previous {
        tx.execute("DELETE FROM trips WHERE id = ?1", params![id])?;
    }

    let shown: Vec<(i64, i64, i64)> = {
        let mut stmt =
            tx.prepare("SELECT id, first_day, last_day FROM trips WHERE status != ?1")?;
        stmt.query_map(params![REJECTED], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, first, last) in shown {
        summarize(&tx, &judged, id, first, last)?;
    }
    tx.execute(
        "DELETE FROM trips WHERE count = 0 AND status != ?1",
        params![REJECTED],
    )?;
    tx.commit()?;
    Ok(())
}

/// Stores a trip's count, cover (favorites, ratings and photos first, among the photos
/// taken away, nearest the middle) and towns.
fn summarize(tx: &Transaction, judged: &[Judged], id: i64, first: i64, last: i64) -> Result<()> {
    let lo = judged.partition_point(|d| d.day < first);
    let hi = judged.partition_point(|d| d.day <= last);
    let mut towns: HashMap<i64, (i64, i64)> = HashMap::new();
    for d in &judged[lo..hi] {
        for &(place, n) in &d.away_places {
            let entry = towns.entry(place).or_insert((0, d.day));
            entry.0 += n;
        }
    }
    tx.execute("DELETE FROM trip_places WHERE trip_id = ?1", params![id])?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO trip_places (trip_id, place_id, count, first_day) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (&place, &(n, day)) in &towns {
            insert.execute(params![id, place, n, day])?;
        }
    }
    let away: Vec<String> = towns.keys().map(i64::to_string).collect();
    let away_only = if away.is_empty() {
        String::new()
    } else {
        format!("a.place_id IN ({}) DESC,", away.join(", "))
    };
    let middle = (first + last) / 2;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM assets
         WHERE local_day BETWEEN ?1 AND ?2 AND status = 0 AND paired_with IS NULL",
        params![first, last],
        |r| r.get(0),
    )?;
    let cover: Option<i64> = tx
        .query_row(
            &format!(
                "SELECT a.id FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id
                 WHERE a.local_day BETWEEN ?1 AND ?2 AND a.status = 0 AND a.paired_with IS NULL
                 ORDER BY COALESCE(u.hidden, 0) = 0 DESC, {away_only} COALESCE(u.favorite, 0) DESC,
                     COALESCE(u.rating, 0) DESC, a.media_kind != 1 DESC,
                     abs(a.local_day - ?3), a.taken_at, a.id
                 LIMIT 1"
            ),
            params![first, last, middle],
            |r| r.get(0),
        )
        .optional()?;
    tx.execute(
        "UPDATE trips SET count = ?2, cover_asset_id = ?3 WHERE id = ?1",
        params![id, count, cover],
    )?;
    Ok(())
}

const SUMMARY_SQL: &str =
    "SELECT id, title, first_day, last_day, count, cover_asset_id, status FROM trips";

fn summary_row(r: &rusqlite::Row) -> rusqlite::Result<TripSummary> {
    Ok(TripSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        first_day: r.get(2)?,
        last_day: r.get(3)?,
        count: r.get(4)?,
        cover_id: r.get(5)?,
        places: Vec::new(),
        place_count: 0,
        countries: Vec::new(),
        edited: r.get::<_, String>(6)? == ACCEPTED,
    })
}

/// Fills in the towns of each trip.
fn with_places(conn: &Connection, trips: &mut [TripSummary]) -> Result<()> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {PLACE_COLUMNS}, tp.count, tp.first_day FROM trip_places tp
         JOIN places p ON p.id = tp.place_id WHERE tp.trip_id = ?1
         ORDER BY tp.count DESC, tp.first_day"
    ))?;
    for trip in trips.iter_mut() {
        // Most photographed first.
        let towns: Vec<(Place, i64)> = stmt
            .query_map(params![trip.id], |r| {
                Ok((place_row(r, 0)?.expect("joined place"), r.get(6)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        trip.place_count = towns.len() as i64;
        let mut named: Vec<&(Place, i64)> = towns.iter().take(NAMED_PLACES).collect();
        named.sort_by_key(|t| t.1);
        trip.places = named.into_iter().map(|t| t.0.clone()).collect();
        let mut visits: Vec<&(Place, i64)> = towns.iter().collect();
        visits.sort_by_key(|t| t.1);
        for (place, _) in visits {
            if !trip.countries.contains(&place.country) {
                trip.countries.push(place.country.clone());
            }
        }
    }
    Ok(())
}

/// Every trip worth showing, newest first.
pub fn list(conn: &Connection) -> Result<Vec<TripSummary>> {
    let mut stmt = conn.prepare_cached(&format!(
        "{SUMMARY_SQL} WHERE status != ?1 AND count > 0 ORDER BY first_day DESC"
    ))?;
    let mut trips: Vec<TripSummary> = stmt
        .query_map(params![REJECTED], summary_row)?
        .collect::<rusqlite::Result<_>>()?;
    with_places(conn, &mut trips)?;
    Ok(trips)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<TripSummary>> {
    let trip = conn
        .query_row(
            &format!("{SUMMARY_SQL} WHERE id = ?1 AND status != ?2"),
            params![id, REJECTED],
            summary_row,
        )
        .optional()?;
    let mut trips: Vec<TripSummary> = trip.into_iter().collect();
    with_places(conn, &mut trips)?;
    Ok(trips.pop())
}

/// The trip a local day falls in, if one is shown.
pub fn of_day(conn: &Connection, day: i64) -> Result<Option<TripSummary>> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM trips WHERE first_day <= ?1 AND last_day >= ?1
               AND status != ?2 AND count > 0
             ORDER BY first_day DESC LIMIT 1",
            params![day, REJECTED],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => get(conn, id),
        None => Ok(None),
    }
}

fn require(conn: &Connection, id: i64) -> Result<()> {
    let found: Option<String> = conn
        .query_row("SELECT status FROM trips WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .optional()?;
    match found.as_deref() {
        Some(REJECTED) | None => Err(Error::Invalid("That trip no longer exists".into())),
        Some(_) => Ok(()),
    }
}

/// Names a trip (an empty name goes back to naming it after its towns). Its days are
/// kept from then on.
pub fn rename(conn: &Connection, id: i64, title: &str) -> Result<()> {
    let title = title.trim();
    if title.chars().count() > MAX_TITLE {
        return Err(Error::Invalid(format!(
            "Keep names under {MAX_TITLE} characters"
        )));
    }
    require(conn, id)?;
    conn.execute(
        "UPDATE trips SET title = ?2, status = ?3 WHERE id = ?1",
        params![id, (!title.is_empty()).then_some(title), ACCEPTED],
    )?;
    Ok(())
}

/// Stops showing a trip; the same days aren't suggested as a trip again.
pub fn remove(conn: &Connection, id: i64) -> Result<()> {
    require(conn, id)?;
    conn.execute(
        "UPDATE trips SET status = ?2 WHERE id = ?1",
        params![id, REJECTED],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAHORE: i64 = 1172451;
    const ISLAMABAD: i64 = 1176615;
    const GILGIT: i64 = 1178338;
    const KASUR: i64 = 1174625; // ~45 km from Lahore: still home

    /// A library of located photos: (day, place, how many).
    fn library(photos: &[(i64, i64, i64)]) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        let index = Index::bundled();
        for &(day, place, n) in photos {
            let c = index.city(place).expect("test town in the bundled list");
            conn.execute(
                "INSERT OR IGNORE INTO places (id, name, region, country, country_code)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![place, c.name, c.region, c.country, c.country_code],
            )
            .unwrap();
            for i in 0..n {
                conn.execute(
                    "INSERT INTO assets (media_kind, taken_at, taken_src, local_day, place_id,
                         lat, lon, created_at)
                     VALUES (0, ?1, 0, ?2, ?3, ?4, ?5, 0)",
                    params![day * DAY_MS + i * 60_000, day, place, c.lat, c.lon],
                )
                .unwrap();
            }
        }
        conn
    }

    /// 30 home days, then the given trip days, then 30 more home days.
    fn around(trip: &[(i64, i64, i64)]) -> Vec<(i64, i64, i64)> {
        let mut photos: Vec<(i64, i64, i64)> = (0..30).map(|d| (19_000 + d, LAHORE, 5)).collect();
        photos.extend_from_slice(trip);
        photos.extend((0..30).map(|d| (19_100 + d, LAHORE, 5)));
        photos
    }

    fn days(trips: &[TripSummary]) -> Vec<(i64, i64)> {
        let mut out: Vec<(i64, i64)> = trips.iter().map(|t| (t.first_day, t.last_day)).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn finds_multi_day_trips_and_names_their_towns() {
        let mut conn = library(&around(&[
            (19_050, ISLAMABAD, 8),
            (19_051, GILGIT, 20), // a day with no photos follows
            (19_053, GILGIT, 10),
            (19_054, KASUR, 3),      // near home again: the trip is over
            (19_070, ISLAMABAD, 30), // one day away isn't a trip
        ]));
        detect(&mut conn).unwrap();
        let trips = list(&conn).unwrap();
        assert_eq!(days(&trips), [(19_050, 19_053)]);
        let trip = &trips[0];
        let names: Vec<&str> = trip.places.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Islamabad", "Gilgit"], "in visit order");
        assert_eq!(trip.countries, ["Pakistan"]);
        assert_eq!(trip.count, 38);
        assert_eq!(of_day(&conn, 19_052).unwrap().unwrap().id, trip.id);
        assert!(of_day(&conn, 19_060).unwrap().is_none());

        detect(&mut conn).unwrap();
        assert_eq!(list(&conn).unwrap(), trips, "re-running changes nothing");
    }

    #[test]
    fn long_gaps_split_trips() {
        let mut conn = library(&around(&[
            (19_050, GILGIT, 10),
            (19_051, GILGIT, 10),
            (19_055, GILGIT, 10), // three days with no photos in between
            (19_056, GILGIT, 10),
        ]));
        detect(&mut conn).unwrap();
        assert_eq!(
            days(&list(&conn).unwrap()),
            [(19_050, 19_051), (19_055, 19_056)]
        );
    }

    #[test]
    fn home_follows_a_move() {
        // A year in Lahore, then a year in Islamabad: neither is a trip.
        let mut photos: Vec<(i64, i64, i64)> =
            (0..40).map(|d| (19_000 + d * 9, LAHORE, 5)).collect();
        photos.extend((0..40).map(|d| (19_400 + d * 9, ISLAMABAD, 5)));
        // A weekend in Lahore after moving is.
        photos.extend([(19_600, LAHORE, 10), (19_601, LAHORE, 10)]);
        let mut conn = library(&photos);
        detect(&mut conn).unwrap();
        assert_eq!(days(&list(&conn).unwrap()), [(19_600, 19_601)]);
    }

    #[test]
    fn renamed_and_removed_trips_stay_put() {
        let mut conn = library(&around(&[
            (19_050, GILGIT, 10),
            (19_051, GILGIT, 10),
            (19_060, ISLAMABAD, 10),
            (19_061, ISLAMABAD, 10),
        ]));
        detect(&mut conn).unwrap();
        let trips = list(&conn).unwrap();
        let (islamabad, gilgit) = (&trips[0], &trips[1]);
        rename(&conn, gilgit.id, " Mountains ").unwrap();
        remove(&conn, islamabad.id).unwrap();
        detect(&mut conn).unwrap();
        let after = list(&conn).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].title.as_deref(), Some("Mountains"));
        assert!(after[0].edited);
        assert!(get(&conn, islamabad.id).unwrap().is_none());
        assert!(rename(&conn, islamabad.id, "x").is_err());
    }
}
