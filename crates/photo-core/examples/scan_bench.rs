//! Scans a real folder into a throwaway library and reports throughput, then
//! rescans to measure the unchanged-library path. Only reads the folder.
//!
//! cargo run --release -p photo-core --example scan_bench -- <photos-folder> <scratch-dir>

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use photo_core::db::Db;
use photo_core::scan::Scanner;
use photo_core::{library, schema, timeline};

fn main() -> photo_core::Result<()> {
    let mut args = std::env::args().skip(1);
    let photos = PathBuf::from(
        args.next()
            .expect("usage: scan_bench <photos> <scratch-dir>"),
    );
    let scratch = PathBuf::from(
        args.next()
            .expect("usage: scan_bench <photos> <scratch-dir>"),
    );
    std::fs::create_dir_all(&scratch)?;
    for name in ["bench-library.sqlite", "bench-thumbs.sqlite"] {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(scratch.join(format!("{name}{suffix}")));
        }
    }

    let library = Db::open(
        &scratch.join("bench-library.sqlite"),
        4,
        schema::migrate_library,
    )?;
    let thumbs = Db::open(
        &scratch.join("bench-thumbs.sqlite"),
        2,
        schema::migrate_thumbs,
    )?;
    let root_id = library::add_root(&library.write(), &photos)?;
    let cancel = AtomicBool::new(false);
    let scanner = Scanner {
        library: &library,
        thumbs: &thumbs,
        cancel: &cancel,
    };

    for pass in ["first scan", "rescan (unchanged)"] {
        let t = Instant::now();
        let s = scanner.scan_root(root_id, &|_| {})?;
        let secs = t.elapsed().as_secs_f64();
        println!(
            "{pass}: {:.1} s · {} files · {:.0} files/s · added {} · unchanged {} · errors {} · missing check skipped: {}",
            secs,
            s.discovered,
            s.discovered as f64 / secs.max(1e-9),
            s.added,
            s.unchanged,
            s.errors,
            s.missing_check_skipped
        );
    }

    let conn = library.read();
    let stats = timeline::stats(&conn)?;
    let by_source: Vec<(u8, i64)> = conn
        .prepare("SELECT taken_src, count(*) FROM assets GROUP BY taken_src ORDER BY taken_src")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let thumbs_stored: i64 =
        thumbs
            .read()
            .query_row("SELECT count(*) FROM thumbs_256", [], |r| r.get(0))?;
    let videos: i64 = conn.query_row(
        "SELECT count(*) FROM assets WHERE media_kind = 1",
        [],
        |r| r.get(0),
    )?;
    println!(
        "assets {} over {} days · videos {videos}",
        stats.assets, stats.days
    );
    println!("capture time source (0 exif, 1 video, 2 filename, 3 mtime): {by_source:?}");
    println!("embedded thumbnails stored during scan: {thumbs_stored}");
    Ok(())
}
