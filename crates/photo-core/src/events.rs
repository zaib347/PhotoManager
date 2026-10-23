//! Events: runs of photos taken close together (an outing, a party, a day out).
//!
//! Detection splits the library where the gap between shots is unusually long
//! compared with the gaps around it, so someone who shoots in bursts and someone who
//! takes a photo an hour both get sensible groups. Events the user has edited are
//! kept as they are; detection only redraws the suggested ones and adds new photos
//! that fall inside an edited event's time span to it.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::{Error, Result};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// Gaps shorter than this never split an event...
const MIN_SPLIT_GAP: i64 = HOUR;
/// ...and longer ones always do (multi-day groupings are trips, not events).
const MAX_GAP: i64 = 8 * HOUR;
/// Neighbouring gaps on each side that define what "usual" means locally.
const WINDOW: usize = 10;
/// A gap splits when it is this many times the local typical (geometric mean) gap.
const GAP_RATIO: f64 = 17.0;
/// Smaller groups aren't worth calling an event.
pub const MIN_EVENT_SIZE: usize = 6;
const MAX_TITLE: usize = 200;

const SUGGESTED: &str = "suggested";
const ACCEPTED: &str = "accepted";
const REJECTED: &str = "rejected";

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventSummary {
    pub id: i64,
    /// `None` until the user names it; the UI shows the dates instead.
    pub title: Option<String>,
    /// Wall-clock ms of the first and last item.
    pub start: i64,
    pub end: i64,
    pub count: i64,
    pub cover_id: Option<i64>,
    /// Renamed, merged, split or made by hand; detection leaves it alone.
    pub edited: bool,
}

/// Indices where a new group starts in ascending capture times.
pub fn split_points(times: &[i64]) -> Vec<usize> {
    if times.len() < 2 {
        return Vec::new();
    }
    let gaps: Vec<i64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    // Log gaps (floored at a second) so "unusual" means a ratio, not a difference.
    let logs: Vec<f64> = gaps.iter().map(|&g| (g.max(1000) as f64).ln()).collect();
    let mut prefix = Vec::with_capacity(logs.len() + 1);
    prefix.push(0.0);
    for l in &logs {
        prefix.push(prefix.last().unwrap() + l);
    }
    let threshold = GAP_RATIO.ln();
    let mut starts = Vec::new();
    for (i, &gap) in gaps.iter().enumerate() {
        let split = if gap >= MAX_GAP {
            true
        } else if gap < MIN_SPLIT_GAP {
            false
        } else {
            let lo = i.saturating_sub(WINDOW);
            let hi = (i + WINDOW + 1).min(logs.len());
            let others = (hi - lo - 1) as f64;
            others == 0.0 || logs[i] - (prefix[hi] - prefix[lo] - logs[i]) / others >= threshold
        };
        if split {
            starts.push(i + 1);
        }
    }
    starts
}

/// One present asset as detection and summaries see it.
struct Member {
    id: i64,
    wall: i64,
    event: Option<i64>,
    event_status: Option<String>,
    /// Visible, favorite, rating, photo: higher makes a better cover.
    score: (bool, bool, u8, bool),
}

const MEMBER_SQL: &str = "SELECT a.id, a.taken_at + a.tz_offset_min * 60000, a.event_id, e.status,
            COALESCE(u.hidden, 0) = 0, COALESCE(u.favorite, 0), COALESCE(u.rating, 0),
            a.media_kind != 1
     FROM assets a
     LEFT JOIN user_meta u ON u.asset_id = a.id
     LEFT JOIN events e ON e.id = a.event_id
     WHERE a.status = 0 AND a.paired_with IS NULL";

fn read_member(r: &rusqlite::Row) -> rusqlite::Result<Member> {
    Ok(Member {
        id: r.get(0)?,
        wall: r.get(1)?,
        event: r.get(2)?,
        event_status: r.get(3)?,
        score: (r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?),
    })
}

/// (start, end, count, cover) of members sorted by time. The cover is the best-scored
/// item, nearest the middle among equals.
fn summarize(members: &[&Member]) -> Option<(i64, i64, i64, i64)> {
    let first = members.first()?;
    let last = members.last()?;
    let middle = members.len() / 2;
    let cover = members
        .iter()
        .enumerate()
        .max_by_key(|(i, m)| (m.score, std::cmp::Reverse(i.abs_diff(middle))))
        .map(|(_, m)| m.id)?;
    Some((first.wall, last.wall, members.len() as i64, cover))
}

/// Redraws suggested events over the whole library. Cheap to re-run: ids are reused
/// where groups stay the same and only changed assets are written.
pub fn detect(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let mut members: Vec<Member> = {
        let mut stmt = tx.prepare(MEMBER_SQL)?;
        stmt.query_map([], read_member)?
            .collect::<rusqlite::Result<_>>()?
    };
    members.sort_unstable_by_key(|m| (m.wall, m.id));

    // Edited events keep their members and absorb new items inside their span.
    let mut spans: Vec<(i64, i64, i64)> = {
        let mut stmt = tx.prepare("SELECT start_at, end_at, id FROM events WHERE status = ?1")?;
        stmt.query_map(params![ACCEPTED], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    spans.sort_unstable();
    let in_span = |wall: i64| {
        let i = spans.partition_point(|s| s.0 <= wall);
        spans[..i].iter().rev().find(|s| s.1 >= wall).map(|s| s.2)
    };

    let mut assigned: Vec<Option<i64>> = vec![None; members.len()];
    let mut free = Vec::new();
    for (i, m) in members.iter().enumerate() {
        match m.event_status.as_deref() {
            Some(ACCEPTED) | Some(REJECTED) => assigned[i] = m.event,
            _ => match in_span(m.wall) {
                Some(id) => assigned[i] = Some(id),
                None => free.push(i),
            },
        }
    }

    let times: Vec<i64> = free.iter().map(|&i| members[i].wall).collect();
    let mut bounds = vec![0];
    bounds.extend(split_points(&times));
    bounds.push(free.len());
    let mut reused: HashSet<i64> = HashSet::new();
    let mut insert =
        tx.prepare("INSERT INTO events (start_at, end_at, status) VALUES (0, 0, ?1)")?;
    for w in bounds.windows(2) {
        let group = &free[w[0]..w[1]];
        if group.len() < MIN_EVENT_SIZE {
            continue;
        }
        // Keep the id most of this group already had, so open views stay valid.
        let mut tally: HashMap<i64, usize> = HashMap::new();
        for &i in group {
            if let Some(id) = members[i].event
                && members[i].event_status.as_deref() == Some(SUGGESTED)
            {
                *tally.entry(id).or_default() += 1;
            }
        }
        let mut candidates: Vec<(usize, i64)> = tally.into_iter().map(|(id, n)| (n, id)).collect();
        candidates.sort_unstable_by(|a, b| b.cmp(a));
        let id = match candidates.into_iter().find(|(_, id)| !reused.contains(id)) {
            Some((_, id)) => id,
            None => {
                insert.execute(params![SUGGESTED])?;
                tx.last_insert_rowid()
            }
        };
        reused.insert(id);
        for &i in group {
            assigned[i] = Some(id);
        }
    }
    drop(insert);

    {
        let mut write = tx.prepare_cached("UPDATE assets SET event_id = ?2 WHERE id = ?1")?;
        for (m, &event) in members.iter().zip(&assigned) {
            if m.event != event {
                write.execute(params![m.id, event])?;
            }
        }
    }
    // Missing items and Live clips drop out of suggested events.
    tx.execute(
        "UPDATE assets SET event_id = NULL
         WHERE (status != 0 OR paired_with IS NOT NULL)
           AND event_id IN (SELECT id FROM events WHERE status = ?1)",
        params![SUGGESTED],
    )?;

    let mut groups: HashMap<i64, Vec<&Member>> = HashMap::new();
    for (m, event) in members.iter().zip(&assigned) {
        if let Some(id) = event {
            groups.entry(*id).or_default().push(m);
        }
    }
    store_summaries(&tx, &groups)?;
    tx.execute(
        "DELETE FROM events WHERE count = 0 AND status != ?1",
        params![REJECTED],
    )?;
    tx.commit()?;
    Ok(())
}

/// Writes start/end/count/cover for `groups` and zeroes every other event.
fn store_summaries(tx: &Transaction, groups: &HashMap<i64, Vec<&Member>>) -> Result<()> {
    tx.execute("UPDATE events SET count = 0, cover_asset_id = NULL", [])?;
    let mut update = tx.prepare_cached(
        "UPDATE events SET start_at = ?2, end_at = ?3, count = ?4, cover_asset_id = ?5 WHERE id = ?1",
    )?;
    for (id, members) in groups {
        if let Some((start, end, count, cover)) = summarize(members) {
            update.execute(params![id, start, end, count, cover])?;
        }
    }
    Ok(())
}

/// Recomputes the summaries of a few events after a manual edit; drops empty ones.
fn refresh(tx: &Transaction, ids: &[i64]) -> Result<()> {
    let mut stmt = tx.prepare_cached(&format!(
        "{MEMBER_SQL} AND a.event_id = ?1 ORDER BY a.taken_at + a.tz_offset_min * 60000, a.id"
    ))?;
    let mut update = tx.prepare_cached(
        "UPDATE events SET start_at = ?2, end_at = ?3, count = ?4, cover_asset_id = ?5 WHERE id = ?1",
    )?;
    for &id in ids {
        let members: Vec<Member> = stmt
            .query_map(params![id], read_member)?
            .collect::<rusqlite::Result<_>>()?;
        let refs: Vec<&Member> = members.iter().collect();
        match summarize(&refs) {
            Some((start, end, count, cover)) => {
                update.execute(params![id, start, end, count, cover])?;
            }
            None => {
                tx.execute(
                    "UPDATE assets SET event_id = NULL WHERE event_id = ?1",
                    params![id],
                )?;
                tx.execute("DELETE FROM events WHERE id = ?1", params![id])?;
            }
        }
    }
    Ok(())
}

/// Every event worth showing, newest first.
pub fn list(conn: &Connection) -> Result<Vec<EventSummary>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, start_at, end_at, count, cover_asset_id, status FROM events
         WHERE status != ?1 AND count > 0 ORDER BY start_at DESC",
    )?;
    let rows = stmt
        .query_map(params![REJECTED], summary_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<EventSummary>> {
    Ok(conn
        .query_row(
            "SELECT id, title, start_at, end_at, count, cover_asset_id, status FROM events
             WHERE id = ?1 AND status != ?2",
            params![id, REJECTED],
            summary_row,
        )
        .optional()?)
}

/// The event an asset belongs to, if it's one that is shown.
pub fn of_asset(conn: &Connection, asset_id: i64) -> Result<Option<EventSummary>> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT event_id FROM assets WHERE id = ?1",
            params![asset_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    match id {
        Some(id) => get(conn, id),
        None => Ok(None),
    }
}

fn summary_row(r: &rusqlite::Row) -> rusqlite::Result<EventSummary> {
    Ok(EventSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        start: r.get(2)?,
        end: r.get(3)?,
        count: r.get(4)?,
        cover_id: r.get(5)?,
        edited: r.get::<_, String>(6)? == ACCEPTED,
    })
}

fn clean_title(title: &str) -> Result<Option<String>> {
    let title = title.trim();
    if title.chars().count() > MAX_TITLE {
        return Err(Error::Invalid(format!(
            "Keep names under {MAX_TITLE} characters"
        )));
    }
    Ok((!title.is_empty()).then(|| title.to_owned()))
}

fn require(tx: &Transaction, id: i64) -> Result<()> {
    let found: Option<String> = tx
        .query_row(
            "SELECT status FROM events WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    match found.as_deref() {
        Some(REJECTED) | None => Err(Error::Invalid("That event no longer exists".into())),
        Some(_) => Ok(()),
    }
}

/// Names an event (an empty name goes back to showing dates).
pub fn rename(conn: &mut Connection, id: i64, title: &str) -> Result<()> {
    let title = clean_title(title)?;
    let tx = conn.transaction()?;
    require(&tx, id)?;
    tx.execute(
        "UPDATE events SET title = ?2, status = ?3 WHERE id = ?1",
        params![id, title, ACCEPTED],
    )?;
    tx.commit()?;
    Ok(())
}

/// Joins events into the earliest of them. Returns its id.
pub fn merge(conn: &mut Connection, ids: &[i64]) -> Result<i64> {
    let tx = conn.transaction()?;
    let mut found = Vec::new();
    for &id in ids {
        require(&tx, id)?;
        let start: i64 = tx.query_row(
            "SELECT start_at FROM events WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        found.push((start, id));
    }
    found.sort_unstable();
    found.dedup_by_key(|f| f.1);
    let Some(&(_, target)) = found.first() else {
        return Err(Error::Invalid("Choose events to merge".into()));
    };
    // Keep the first name anyone gave.
    let title: Option<String> = found.iter().find_map(|&(_, id)| {
        tx.query_row("SELECT title FROM events WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .ok()
        .flatten()
    });
    for &(_, id) in &found[1..] {
        tx.execute(
            "UPDATE assets SET event_id = ?2 WHERE event_id = ?1",
            params![id, target],
        )?;
        tx.execute("DELETE FROM events WHERE id = ?1", params![id])?;
    }
    tx.execute(
        "UPDATE events SET title = ?2, status = ?3 WHERE id = ?1",
        params![target, title, ACCEPTED],
    )?;
    refresh(&tx, &[target])?;
    tx.commit()?;
    Ok(target)
}

/// Splits an event so `at_asset` and everything after it form a new event.
/// Returns the new event's id.
pub fn split(conn: &mut Connection, id: i64, at_asset: i64) -> Result<i64> {
    let tx = conn.transaction()?;
    require(&tx, id)?;
    let at: Option<i64> = tx
        .query_row(
            "SELECT taken_at + tz_offset_min * 60000 FROM assets WHERE id = ?1 AND event_id = ?2",
            params![at_asset, id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(at) = at else {
        return Err(Error::Invalid("That item isn't in this event".into()));
    };
    let (before, after): (i64, i64) = tx.query_row(
        "SELECT SUM(w < ?2 OR (w = ?2 AND id < ?3)), SUM(w > ?2 OR (w = ?2 AND id >= ?3))
         FROM (SELECT id, taken_at + tz_offset_min * 60000 AS w FROM assets
               WHERE event_id = ?1 AND status = 0 AND paired_with IS NULL)",
        params![id, at, at_asset],
        |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
            ))
        },
    )?;
    if before == 0 || after == 0 {
        return Err(Error::Invalid(
            "Pick an item in the middle of the event".into(),
        ));
    }
    tx.execute(
        "INSERT INTO events (start_at, end_at, status) VALUES (0, 0, ?1)",
        params![ACCEPTED],
    )?;
    let new_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE assets SET event_id = ?4 WHERE event_id = ?1
           AND (taken_at + tz_offset_min * 60000 > ?2
                OR (taken_at + tz_offset_min * 60000 = ?2 AND id >= ?3))",
        params![id, at, at_asset, new_id],
    )?;
    tx.execute(
        "UPDATE events SET status = ?2 WHERE id = ?1",
        params![id, ACCEPTED],
    )?;
    refresh(&tx, &[id, new_id])?;
    tx.commit()?;
    Ok(new_id)
}

/// Makes an event from hand-picked items, taking them out of any event they were in.
pub fn create(conn: &mut Connection, asset_ids: &[i64], title: &str) -> Result<i64> {
    let title = clean_title(title)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO events (title, start_at, end_at, status) VALUES (?1, 0, 0, ?2)",
        params![title, ACCEPTED],
    )?;
    let new_id = tx.last_insert_rowid();
    let mut previous = Vec::new();
    {
        let mut read = tx.prepare("SELECT event_id FROM assets WHERE id = ?1")?;
        let mut write = tx.prepare("UPDATE assets SET event_id = ?2 WHERE id = ?1")?;
        for &asset in asset_ids {
            let Some(old) = read
                .query_row(params![asset], |r| r.get::<_, Option<i64>>(0))
                .optional()?
            else {
                continue;
            };
            if let Some(old) = old
                && !previous.contains(&old)
            {
                previous.push(old);
            }
            write.execute(params![asset, new_id])?;
        }
    }
    previous.push(new_id);
    refresh(&tx, &previous)?;
    if require(&tx, new_id).is_err() {
        return Err(Error::Invalid(
            "None of those items are in the library".into(),
        ));
    }
    tx.commit()?;
    Ok(new_id)
}

/// Stops showing an event. Its items stay out of suggested events, so the same
/// grouping doesn't come back on the next scan.
pub fn remove(conn: &mut Connection, id: i64) -> Result<()> {
    let tx = conn.transaction()?;
    require(&tx, id)?;
    tx.execute(
        "UPDATE events SET status = ?2 WHERE id = ?1",
        params![id, REJECTED],
    )?;
    tx.commit()?;
    Ok(())
}

/// True when the library has items but detection has never run (e.g. right after
/// upgrading to a version with events).
pub fn needs_detection(conn: &Connection) -> Result<bool> {
    let never: bool = conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM events) AND EXISTS (SELECT 1 FROM assets WHERE status = 0)",
        [],
        |r| r.get(0),
    )?;
    Ok(never)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_700_000_000_000;

    #[test]
    fn splits_on_long_gaps_relative_to_neighbours() {
        // A burst every minute, a 2 h break, another burst: one split.
        let mut times: Vec<i64> = (0..10).map(|i| T0 + i * MINUTE).collect();
        times.extend((0..10).map(|i| T0 + 9 * MINUTE + 2 * HOUR + i * MINUTE));
        assert_eq!(split_points(&times), vec![10]);
    }

    #[test]
    fn sparse_shooters_are_not_chopped_up() {
        // A photo every 40 minutes with one 2 h gap: the gap isn't unusual enough.
        let mut times: Vec<i64> = (0..10).map(|i| T0 + i * 40 * MINUTE).collect();
        times.extend((0..10).map(|i| T0 + 6 * HOUR + 2 * HOUR + i * 40 * MINUTE));
        assert!(split_points(&times).is_empty());
    }

    fn library(n: usize) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        crate::synth::generate(&mut conn, n, 5).unwrap();
        conn
    }

    fn members_of(conn: &Connection, id: i64) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT id FROM assets WHERE event_id = ?1 ORDER BY taken_at + tz_offset_min * 60000, id")
            .unwrap();
        stmt.query_map(params![id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn detection_is_stable_and_consistent() {
        let mut conn = library(5_000);
        assert!(needs_detection(&conn).unwrap());
        detect(&mut conn).unwrap();
        assert!(!needs_detection(&conn).unwrap());
        let first = list(&conn).unwrap();
        assert!(first.len() > 10, "synthetic library should form events");
        for e in &first {
            assert!(e.count as usize >= MIN_EVENT_SIZE && e.start <= e.end);
            assert_eq!(members_of(&conn, e.id).len() as i64, e.count);
            assert!(members_of(&conn, e.id).contains(&e.cover_id.unwrap()));
        }
        detect(&mut conn).unwrap();
        assert_eq!(list(&conn).unwrap(), first, "re-running changes nothing");
    }

    #[test]
    fn edits_survive_detection() {
        let mut conn = library(5_000);
        detect(&mut conn).unwrap();
        let events = list(&conn).unwrap();
        let (a, b, c) = (&events[0], &events[1], &events[2]);

        rename(&mut conn, a.id, "  Beach day ").unwrap();
        let merged = merge(&mut conn, &[a.id, b.id]).unwrap();
        assert_eq!(merged, b.id, "merges into the earlier event");
        let m = get(&conn, merged).unwrap().unwrap();
        assert_eq!(m.title.as_deref(), Some("Beach day"));
        assert_eq!(m.count, a.count + b.count);
        assert!(m.edited && get(&conn, a.id).unwrap().is_none());

        let ids = members_of(&conn, c.id);
        let half = split(&mut conn, c.id, ids[ids.len() / 2]).unwrap();
        assert_eq!(members_of(&conn, half), ids[ids.len() / 2..]);
        assert!(
            split(&mut conn, half, ids[ids.len() / 2]).is_err(),
            "can't split at the start"
        );

        let picked = &members_of(&conn, merged)[..3];
        let made = create(&mut conn, picked, "Highlights").unwrap();
        assert_eq!(get(&conn, made).unwrap().unwrap().count, 3);
        assert_eq!(get(&conn, merged).unwrap().unwrap().count, m.count - 3);

        let gone = events[3].id;
        remove(&mut conn, gone).unwrap();

        let before = list(&conn).unwrap();
        detect(&mut conn).unwrap();
        assert_eq!(
            list(&conn).unwrap(),
            before,
            "detection keeps edits and removals"
        );
        assert!(get(&conn, gone).unwrap().is_none());
    }

    #[test]
    fn overnight_always_splits_and_short_gaps_never_do() {
        let times = [T0, T0 + 30 * MINUTE, T0 + 30 * MINUTE + 9 * HOUR];
        assert_eq!(split_points(&times), vec![2]);
        assert!(split_points(&[T0, T0 + 1000, T0 + 50 * MINUTE]).is_empty());
    }
}
