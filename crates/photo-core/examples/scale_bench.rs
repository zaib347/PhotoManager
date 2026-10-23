//! Measures database behaviour at library scale with synthetic data.
//!
//! cargo run --release -p photo-core --example scale_bench -- <db-path> [count]

use std::path::PathBuf;
use std::time::Instant;

use photo_core::{db::Db, schema, synth, timeline};

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
    let layout = timeline::layout(&db.read())?;
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
        fetched += timeline::day_assets(&db.read(), days)?
            .iter()
            .map(|d| d.ids.len())
            .sum::<usize>();
    }
    println!(
        "scrubber jump (10 days): {:?} avg, {} ids avg",
        t.elapsed() / jumps as u32,
        fetched / jumps
    );

    let size = std::fs::metadata(&path)?.len();
    println!(
        "db size: {:.1} MiB ({} bytes/asset)",
        size as f64 / 1048576.0,
        size / count.max(1) as u64
    );
    Ok(())
}
