//! Measures database behaviour at library scale with synthetic data.
//!
//! cargo run --release -p photo-core --example scale_bench -- <db-path> [count]

use std::path::PathBuf;
use std::time::Instant;

use photo_core::{db::Db, events, places, schema, similar, synth, thumbs, timeline, trips};

fn main() -> photo_core::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("usage: scale_bench <db-path> [count]"));
    let count: usize = args.next().map_or(1_000_000, |s| s.parse().expect("count"));

    let db = Db::open(&path, 4, schema::migrate_library)?;
    synth::clear(&db.write())?;

    let t = Instant::now();
    synth::generate(&mut db.write(), count, 1)?;
    println!("insert {count} assets + day buckets: {:?}", t.elapsed());

    let t = Instant::now();
    let layout = timeline::layout(&db.read(), timeline::View::All, &Default::default())?;
    println!("layout ({} days): {:?}", layout.days.len(), t.elapsed());

    let busiest = layout.counts.iter().copied().max().unwrap_or(0);
    println!("busiest day: {busiest} assets");

    // Simulates a scrubber jump: fetch ~10 consecutive days at random positions.
    let jumps = 200;
    let t = Instant::now();
    let mut fetched = 0;
    for i in 0..jumps {
        let at = (i * 7919) % layout.days.len().max(1);
        let days = &layout.days[at..(at + 10).min(layout.days.len())];
        fetched +=
            timeline::day_assets(&db.read(), timeline::View::All, &Default::default(), days)?
                .iter()
                .map(|d| d.ids.len())
                .sum::<usize>();
    }
    println!(
        "scrubber jump (10 days): {:?} avg, {} ids avg",
        t.elapsed() / jumps as u32,
        fetched / jumps
    );

    // Filtered layouts are aggregated on demand instead of read from day buckets.
    let filters = [
        (
            "videos only",
            timeline::Filter {
                media: Some(timeline::MediaFilter::Video),
                ..Default::default()
            },
        ),
        (
            "one camera",
            timeline::Filter {
                cameras: vec!["EOS 80D".into()],
                ..Default::default()
            },
        ),
        (
            "one year",
            timeline::Filter {
                from_day: Some(18_628),
                to_day: Some(18_992),
                ..Default::default()
            },
        ),
    ];
    for (name, filter) in filters {
        let t = Instant::now();
        let layout = timeline::layout(&db.read(), timeline::View::All, &filter)?;
        let days = &layout.days[..layout.days.len().min(10)];
        let first = t.elapsed();
        timeline::day_assets(&db.read(), timeline::View::All, &filter, days)?;
        println!(
            "filter {name}: layout {first:?}, first 10 days {:?}",
            t.elapsed() - first
        );
    }

    let t = Instant::now();
    let placed = places::assign(&db)?;
    println!("place lookup ({placed} with GPS): {:?}", t.elapsed());
    let t = Instant::now();
    let options = timeline::filter_options(&db.read())?;
    println!(
        "filter options ({} places, {} countries): {:?}",
        options.places.len(),
        options.countries.len(),
        t.elapsed()
    );
    if let Some(top) = options.places.first() {
        let filter = timeline::Filter {
            place_ids: vec![top.place.id],
            ..Default::default()
        };
        let t = Instant::now();
        timeline::layout(&db.read(), timeline::View::All, &filter)?;
        println!(
            "filter one place ({}, {} items): {:?}",
            top.place.name,
            top.count,
            t.elapsed()
        );
    }

    let t = Instant::now();
    events::detect(&mut db.write())?;
    println!("event detection (first run): {:?}", t.elapsed());
    let t = Instant::now();
    events::detect(&mut db.write())?;
    println!("event detection (re-run): {:?}", t.elapsed());
    let t = Instant::now();
    trips::detect(&mut db.write())?;
    println!("trip detection: {:?}", t.elapsed());
    let t = Instant::now();
    let found = trips::list(&db.read())?;
    println!("trip list ({} trips): {:?}", found.len(), t.elapsed());
    if let Some(trip) = found.first() {
        let t = Instant::now();
        let view = timeline::View::Trip(trip.id);
        let layout = timeline::layout(&db.read(), view, &Default::default())?;
        timeline::day_assets(&db.read(), view, &Default::default(), &layout.days)?;
        println!("trip view ({} items): {:?}", trip.count, t.elapsed());
    }

    // Similar photos: grouping cost with a hash on every photo (random, so few groups),
    // and the per-photo cost of fingerprinting a grid thumbnail.
    db.write()
        .execute("UPDATE assets SET phash = random(), phash_v = 1", [])?;
    let t = Instant::now();
    similar::detect(&db)?;
    println!(
        "similar detection ({} groups): {:?}",
        similar::overview(&db.read())?.groups,
        t.elapsed()
    );
    let detailed = image::RgbImage::from_fn(256, 192, |x, y| {
        let v = ((x * 7 + y * 3) % 251) as u8 ^ ((x / 16 + y / 16) % 2 * 120) as u8;
        image::Rgb([v, v / 2, 255 - v])
    });
    let thumb = thumbs::encode_jpeg(&detailed)?;
    let t = Instant::now();
    for _ in 0..200 {
        let img = image::load_from_memory(&thumb).expect("thumbnail");
        std::hint::black_box(similar::fingerprint(&img));
    }
    println!("fingerprint one thumbnail: {:?}", t.elapsed() / 200);
    // Assessing a grouped photo decodes its original: a 12 MP JPEG here.
    let original = thumbs::encode_jpeg(&image::RgbImage::from_fn(4000, 3000, |x, y| {
        let v = ((x * 7 + y * 3) % 251) as u8 ^ ((x / 64 + y / 64) % 2 * 120) as u8;
        image::Rgb([v, v / 2, 255 - v])
    }))?;
    let t = Instant::now();
    for _ in 0..5 {
        let img = image::load_from_memory(&original).expect("original");
        std::hint::black_box(similar::assess(&img));
    }
    println!("assess one 12 MP original: {:?}", t.elapsed() / 5);

    let t = Instant::now();
    let list = events::list(&db.read())?;
    println!("event list ({} events): {:?}", list.len(), t.elapsed());
    if let Some(event) = list.get(list.len() / 2) {
        let t = Instant::now();
        let view = timeline::View::Event(event.id);
        let layout = timeline::layout(&db.read(), view, &Default::default())?;
        timeline::day_assets(&db.read(), view, &Default::default(), &layout.days)?;
        println!("event view ({} items): {:?}", event.count, t.elapsed());
    }

    let size = std::fs::metadata(&path)?.len();
    println!(
        "db size: {:.1} MiB ({} bytes/asset)",
        size as f64 / 1048576.0,
        size / count.max(1) as u64
    );
    Ok(())
}
