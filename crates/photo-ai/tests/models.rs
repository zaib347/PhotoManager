//! Tests with the real models. They need the model files, so they're skipped unless
//! `PHOTO_AI_TEST_MODELS` names a folder holding them under their installed names
//! (`onnxruntime.dll`, `vision.onnx`, `text.onnx`, `vocab.json`, `merges.txt`). The download test runs only when asked (`--ignored`).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use image::{DynamicImage, Rgb, RgbImage};
use photo_ai::clip::Clip;
use photo_ai::models::{Pack, Store};

fn models() -> Option<PathBuf> {
    let dir = std::env::var_os("PHOTO_AI_TEST_MODELS").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: set PHOTO_AI_TEST_MODELS");
    }
    dir
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn plain(color: [u8; 3]) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_pixel(256, 192, Rgb(color)))
}

#[test]
fn installs_from_a_folder_and_matches_text_to_images() {
    let Some(src) = models() else { return };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    assert!(!store.is_installed(Pack::Search));
    store.install_from(Pack::Runtime, &src).unwrap();
    store.install_from(Pack::Search, &src).unwrap();
    assert!(store.is_installed(Pack::Search));
    store.verify(Pack::Search).unwrap();

    let clip = Clip::load(&store, 4).unwrap();
    let images = clip
        .embed_images(&[
            plain([220, 30, 30]),
            plain([30, 60, 220]),
            plain([40, 170, 50]),
        ])
        .unwrap();
    assert_eq!(images.len(), 3);
    assert!(
        images
            .iter()
            .all(|v| v.len() == 512 && (dot(v, v) - 1.0).abs() < 1e-3)
    );
    for (i, word) in ["red", "blue", "green"].iter().enumerate() {
        let text = clip.embed_text(&format!("a plain {word} image")).unwrap();
        let scores: Vec<f32> = images.iter().map(|v| dot(v, &text)).collect();
        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(best, i, "{word}: {scores:?}");
    }
    // The same image twice gives the same embedding (batching doesn't change results).
    let again = clip.embed_images(&[plain([30, 60, 220])]).unwrap();
    assert!(dot(&again[0], &images[1]) > 0.999);

    // A damaged file is caught before use.
    let vision = store.path(Pack::Search, "vision.onnx");
    let mut bytes = std::fs::read(&vision).unwrap();
    bytes[1000] ^= 0xFF;
    std::fs::write(&vision, bytes).unwrap();
    assert!(store.verify(Pack::Search).is_err());
    // A wrong file isn't accepted from a folder either.
    assert!(store.install_from(Pack::Search, dir.path()).is_err());
}

/// Downloads the real search pack (~165 MB) through the same path as the app.
#[test]
#[ignore]
fn downloads_and_installs_the_search_pack() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let cancel = AtomicBool::new(false);
    let last = std::sync::Mutex::new(0u64);
    store
        .install(Pack::Runtime, &cancel, &|done, _| {
            *last.lock().unwrap() = done
        })
        .unwrap();
    store
        .install(Pack::Search, &cancel, &|done, _| {
            *last.lock().unwrap() = done
        })
        .unwrap();
    assert_eq!(*last.lock().unwrap(), Pack::Search.download_size());
    store.verify(Pack::Runtime).unwrap();
    store.verify(Pack::Search).unwrap();
    // Installing again is instant: nothing to do.
    store
        .install(Pack::Search, &cancel, &|_, _| panic!("downloaded again"))
        .unwrap();
    assert!(store.remove(Pack::Search, false).unwrap());
    assert!(!store.is_installed(Pack::Search));
}

/// Indexes a small library from its thumbnails and searches it.
#[test]
fn indexes_a_library_and_finds_photos_by_description() {
    let Some(src) = models() else { return };
    use photo_ai::search::{self, Index};
    use photo_core::db::Db;
    use photo_core::{schema, thumbs};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("ai"));
    store.install_from(Pack::Runtime, &src).unwrap();
    store.install_from(Pack::Search, &src).unwrap();
    let clip = Clip::load(&store, 4).unwrap();
    let library = Db::open(&dir.path().join("lib.sqlite"), 1, schema::migrate_library).unwrap();
    let thumbs_db = Db::open(&dir.path().join("thumbs.sqlite"), 1, schema::migrate_thumbs).unwrap();
    // 30 grey photos, 3 red ones, a video, and a photo with no thumbnail or file.
    let red: Vec<i64> = vec![7, 19, 28];
    for id in 1..=32i64 {
        let kind = if id == 31 { 1 } else { 0 };
        library
            .write()
            .execute(
                "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
                 VALUES (?1, ?2, 0, 0, 0, 0)",
                rusqlite::params![id, kind],
            )
            .unwrap();
        if id <= 30 {
            let shade = 90 + (id as u8 * 3);
            let color = if red.contains(&id) {
                [210, 25, 30]
            } else {
                [shade, shade, shade]
            };
            let jpeg = thumbs::encode_jpeg(&RgbImage::from_pixel(256, 192, Rgb(color))).unwrap();
            thumbs::put(&thumbs_db.write(), id, &jpeg).unwrap();
        }
    }
    let cancel = AtomicBool::new(false);
    let done =
        search::index_pending(&library, &thumbs_db, &clip, &cancel, &|| false, &|| {}).unwrap();
    assert_eq!(
        done, 30,
        "the video is skipped; the photo without a file can't be read"
    );
    let progress = search::progress(&library.read()).unwrap();
    assert_eq!(
        (progress.indexed, progress.total),
        (31, 31),
        "the unreadable one is recorded too"
    );
    // Nothing left to do.
    assert_eq!(
        search::index_pending(&library, &thumbs_db, &clip, &cancel, &|| false, &|| {}).unwrap(),
        0
    );

    let index = Index::load(&library.read()).unwrap();
    assert_eq!(index.len(), 30);
    let hits = index.search(&clip.embed_text("a red photo").unwrap());
    let mut found: Vec<i64> = hits.iter().map(|h| h.0).collect();
    found.sort();
    assert_eq!(found, red, "{hits:?}");

    // A changed file loses its embedding (photo-core's scan does this) and is indexed again.
    library
        .write()
        .execute("DELETE FROM ai_embeddings WHERE asset_id = 7", [])
        .unwrap();
    assert_eq!(
        search::index_pending(&library, &thumbs_db, &clip, &cancel, &|| false, &|| {}).unwrap(),
        1
    );
    // Pausing stops a pass before it starts.
    library
        .write()
        .execute("DELETE FROM ai_embeddings", [])
        .unwrap();
    assert_eq!(
        search::index_pending(&library, &thumbs_db, &clip, &cancel, &|| true, &|| {}).unwrap(),
        0
    );
}
