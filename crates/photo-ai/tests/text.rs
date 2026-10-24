//! Text in photos with Windows' own OCR: a scanned folder is read, and photos are found
//! by the words in them. Needs no models; skipped where Windows can't read text.

#![cfg(windows)]

use std::sync::atomic::AtomicBool;

use image::{DynamicImage, Rgb, RgbImage};
use photo_ai::text;
use photo_core::db::Db;
use photo_core::scan::Scanner;
use photo_core::watermark::{self, Position, Watermark};
use photo_core::{library, schema};

/// A dark photo with `words` written across its middle (in white, as watermarks are).
fn sign(words: &str) -> RgbImage {
    let mut img = DynamicImage::ImageRgb8(RgbImage::from_fn(1200, 800, |x, y| {
        Rgb([30 + (x / 40) as u8, 34, 48 + (y / 40) as u8])
    }));
    let mark = watermark::prepare(&Watermark {
        text: words.into(),
        position: Position::Center,
        size: 9,
        opacity: 100,
        ..Default::default()
    })
    .unwrap()
    .unwrap();
    mark.apply(&mut img);
    img.to_rgb8()
}

#[test]
fn reads_text_in_photos_and_finds_them_by_word() {
    if let Some(why) = text::unavailable() {
        eprintln!("skipped: {why}");
        return;
    }
    let read = text::read_image(&sign("INVOICE 4521")).unwrap();
    assert!(
        read.contains("INVOICE") && read.contains("4521"),
        "{read:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    sign("INVOICE 4521")
        .save(photos.join("invoice.jpg"))
        .unwrap();
    sign("Wifi password")
        .save(photos.join("router.png"))
        .unwrap();
    RgbImage::from_pixel(600, 400, Rgb([40, 120, 200]))
        .save(photos.join("sky.jpg"))
        .unwrap();
    let library = Db::open(&dir.path().join("lib.sqlite"), 1, schema::migrate_library).unwrap();
    let thumbs = Db::open(&dir.path().join("thumbs.sqlite"), 1, schema::migrate_thumbs).unwrap();
    let root = library::add_root(&library.write(), &photos).unwrap();
    let cancel = AtomicBool::new(false);
    Scanner {
        library: &library,
        thumbs: &thumbs,
        cancel: &cancel,
    }
    .scan_root(root, &|_| {})
    .unwrap();

    let done = text::index_pending(&library, usize::MAX, &cancel, &|| false, &|| {}).unwrap();
    assert_eq!(done, 3);
    let progress = text::progress(&library.read()).unwrap();
    assert_eq!((progress.indexed, progress.total), (3, 3));
    assert_eq!(
        text::index_pending(&library, usize::MAX, &cancel, &|| false, &|| {}).unwrap(),
        0
    );

    let id_of = |name: &str| -> i64 {
        library
            .read()
            .query_row(
                "SELECT asset_id FROM files WHERE rel_path LIKE ?1",
                [format!("%{name}")],
                |r| r.get(0),
            )
            .unwrap()
    };
    let (invoice, router, sky) = (id_of("invoice.jpg"), id_of("router.png"), id_of("sky.jpg"));
    let conn = library.read();
    assert_eq!(text::search(&conn, "invoice").unwrap(), vec![invoice]);
    // Words in any order and case; a word's start finds it.
    assert_eq!(
        text::search(&conn, "PASS wifi").unwrap(),
        vec![router],
        "read: {:?}",
        text::text_of(&conn, router)
    );
    assert!(text::search(&conn, "invoice wifi").unwrap().is_empty());
    assert!(
        text::text_of(&conn, router)
            .unwrap()
            .unwrap()
            .to_lowercase()
            .contains("password")
    );
    assert_eq!(text::text_of(&conn, sky).unwrap(), None);
    drop(conn);

    // A photo that goes takes its text with it.
    library
        .write()
        .execute("DELETE FROM files WHERE asset_id = ?1", [invoice])
        .unwrap();
    library
        .write()
        .execute("DELETE FROM assets WHERE id = ?1", [invoice])
        .unwrap();
    assert!(text::search(&library.read(), "invoice").unwrap().is_empty());
    text::clear(&library).unwrap();
    assert_eq!(text::progress(&library.read()).unwrap().indexed, 0);
}
