use photo_core::timeline::View;
use photo_core::{schema, synth, thumbs, timeline};
use rusqlite::Connection;

fn library() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    schema::migrate_library(&mut conn).unwrap();
    conn
}

#[test]
fn migrations_are_idempotent() {
    let mut conn = library();
    schema::migrate_library(&mut conn).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 7);
}

#[test]
fn layout_matches_day_assets() {
    let mut conn = library();
    synth::generate(&mut conn, 10_000, 42).unwrap();

    let layout = timeline::layout(&conn, View::All, &Default::default()).unwrap();
    assert_eq!(
        layout.counts.iter().map(|&c| c as usize).sum::<usize>(),
        10_000
    );
    assert!(
        layout.days.windows(2).all(|w| w[0] > w[1]),
        "days must be newest first"
    );

    let sample: Vec<i64> = layout.days.iter().step_by(7).copied().collect();
    for day in timeline::day_assets(&conn, View::All, &Default::default(), &sample).unwrap() {
        let i = layout.days.iter().position(|&d| d == day.day).unwrap();
        assert_eq!(day.ids.len() as u32, layout.counts[i]);
        assert_eq!(day.ids.len(), day.kinds.len());
    }
}

#[test]
fn local_day_respects_timezone() {
    // 2024-01-01 23:30 UTC is already 2024-01-02 in UTC+5.
    let t = 1_704_151_800_000;
    assert_eq!(timeline::local_day(t, 300), timeline::local_day(t, 0) + 1);
}

#[test]
fn thumbnails_round_trip() {
    let mut conn = Connection::open_in_memory().unwrap();
    schema::migrate_thumbs(&mut conn).unwrap();
    let jpeg = thumbs::render_placeholder(7, false).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    thumbs::put(&conn, 7, &jpeg).unwrap();
    assert_eq!(thumbs::get(&conn, 7).unwrap(), Some(jpeg));
    assert_eq!(thumbs::get(&conn, 8).unwrap(), None);
}
