//! Thumbnail throughput on a real folder of JPEGs: embedded EXIF thumbnail
//! extraction versus full decode + resize + re-encode.
//!
//! cargo run --release -p photo-core --example thumb_bench -- <folder> [embedded|decode] [limit]
//!
//! Run each mode separately and twice: the first run measures cold disk reads,
//! the second mostly measures CPU because the OS file cache is warm.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use exif::{In, Tag};
use photo_core::thumbs::{self, GRID_SIZE};
use rayon::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .expect("usage: thumb_bench <folder> [mode] [limit]"),
    );
    let mode = args.next().unwrap_or_else(|| "embedded".into());
    let limit: usize = args
        .next()
        .map_or(usize::MAX, |s| s.parse().expect("limit"));

    let t = Instant::now();
    let files: Vec<PathBuf> = jwalk::WalkDir::new(&root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        })
        .take(limit)
        .collect();
    println!("walk: {} jpegs in {:?}", files.len(), t.elapsed());

    let ok = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let t = Instant::now();
    files.par_iter().for_each(|path| {
        let result = match mode.as_str() {
            "decode" => decode(path),
            _ => embedded(path),
        };
        if let Some(len) = result {
            ok.fetch_add(1, Ordering::Relaxed);
            bytes.fetch_add(len as u64, Ordering::Relaxed);
        }
    });
    let elapsed = t.elapsed();
    let ok = ok.into_inner();
    println!(
        "{mode}: {ok}/{} ok in {elapsed:?} — {:.0} files/s, avg thumb {} bytes",
        files.len(),
        files.len() as f64 / elapsed.as_secs_f64(),
        bytes.into_inner() / ok.max(1)
    );
}

fn embedded(path: &Path) -> Option<usize> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let offset = exif
        .get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL)?
        .value
        .get_uint(0)?;
    let len = exif
        .get_field(Tag::JPEGInterchangeFormatLength, In::THUMBNAIL)?
        .value
        .get_uint(0)?;
    let data = exif.buf().get(offset as usize..(offset + len) as usize)?;
    Some(data.len())
}

fn decode(path: &Path) -> Option<usize> {
    let img = image::open(path).ok()?;
    let thumb = img.thumbnail(GRID_SIZE, GRID_SIZE).to_rgb8();
    thumbs::encode_jpeg(&thumb).ok().map(|j| j.len())
}
