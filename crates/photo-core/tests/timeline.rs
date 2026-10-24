use photo_core::db::Db;
use photo_core::timeline::{Filter, View};
use photo_core::{places, schema, synth, thumbs, timeline, trips};
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
    assert_eq!(version, 16);
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
fn place_filters_match_their_options() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("lib.sqlite"), 1, schema::migrate_library).unwrap();
    synth::generate(&mut db.write(), 10_000, 42).unwrap();
    assert!(places::assign(&db).unwrap() > 5_000);

    let total = |filter: &Filter| -> i64 {
        let layout = timeline::layout(&db.read(), View::All, filter).unwrap();
        layout.counts.iter().map(|&c| c as i64).sum()
    };
    let options = timeline::filter_options(&db.read()).unwrap();
    let home = &options.places[0];
    assert_eq!(home.place.name, "Lahore", "synthetic home");
    let by_place = Filter {
        place_ids: vec![home.place.id],
        ..Default::default()
    };
    assert_eq!(total(&by_place), home.count);

    let pakistan = options.countries.iter().find(|c| c.code == "PK").unwrap();
    let by_country = Filter {
        countries: vec!["PK".into()],
        ..Default::default()
    };
    assert_eq!(total(&by_country), pakistan.count);
    assert!(pakistan.count > home.count, "Islamabad and Gilgit too");
    // Either one matching is enough.
    let both = Filter {
        place_ids: vec![home.place.id],
        countries: vec!["GB".into()],
        ..Default::default()
    };
    let london = options.countries.iter().find(|c| c.code == "GB").unwrap();
    assert_eq!(total(&both), home.count + london.count);
}

#[test]
fn synthetic_trips_leave_home_and_show_their_days() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("lib.sqlite"), 1, schema::migrate_library).unwrap();
    synth::generate(&mut db.write(), 20_000, 7).unwrap();
    places::assign(&db).unwrap();
    trips::detect(&mut db.write()).unwrap();
    let found = trips::list(&db.read()).unwrap();
    assert!(
        !found.is_empty(),
        "synthetic libraries have multi-day trips"
    );
    for trip in &found {
        assert!(trip.last_day > trip.first_day);
        assert!(
            trip.places.iter().all(|p| p.name != "Lahore"),
            "home is never a trip stop"
        );
        let layout =
            timeline::layout(&db.read(), View::Trip(trip.id), &Default::default()).unwrap();
        assert!(
            layout
                .days
                .iter()
                .all(|d| (trip.first_day..=trip.last_day).contains(d))
        );
        assert_eq!(
            layout.counts.iter().map(|&c| c as i64).sum::<i64>(),
            trip.count
        );
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
