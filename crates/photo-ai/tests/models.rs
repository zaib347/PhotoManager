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
    let done = search::index_pending(
        &library,
        &thumbs_db,
        &clip,
        usize::MAX,
        &cancel,
        &|| false,
        &|| {},
    )
    .unwrap();
    assert_eq!(
        done, 31,
        "the video is skipped; the photo without a file can't be read (and is recorded)"
    );
    let progress = search::progress(&library.read()).unwrap();
    assert_eq!(
        (progress.indexed, progress.total),
        (31, 31),
        "the unreadable one is recorded too"
    );
    // Nothing left to do.
    assert_eq!(
        search::index_pending(
            &library,
            &thumbs_db,
            &clip,
            usize::MAX,
            &cancel,
            &|| false,
            &|| {}
        )
        .unwrap(),
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
        search::index_pending(
            &library,
            &thumbs_db,
            &clip,
            usize::MAX,
            &cancel,
            &|| false,
            &|| {}
        )
        .unwrap(),
        1
    );
    // Pausing stops a pass before it starts.
    library
        .write()
        .execute("DELETE FROM ai_embeddings", [])
        .unwrap();
    assert_eq!(
        search::index_pending(
            &library,
            &thumbs_db,
            &clip,
            usize::MAX,
            &cancel,
            &|| true,
            &|| {}
        )
        .unwrap(),
        0
    );
}

/// Enlarges a photo 4×: the right size, true to the original, and no seams where the
/// model's tiles meet.
#[test]
fn upscale_enlarges_without_seams() {
    let Some(src) = models() else { return };
    use photo_ai::upscale::{FACTOR, Upscaler};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.install_from(Pack::Runtime, &src).unwrap();
    store.install_from(Pack::Upscale, &src).unwrap();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get() / 2);
    let upscaler = Upscaler::load(&store, threads.max(1)).unwrap();
    // Smooth colour gradients with a few hard-edged shapes, odd-sized.
    let (w, h) = (250u32, 170u32);
    let img = RgbImage::from_fn(w, h, |x, y| {
        let inside = (x / 40 + y / 40) % 3 == 0 && x % 40 > 8 && y % 40 > 8;
        if inside {
            Rgb([240, 230, 40])
        } else {
            Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 120])
        }
    });
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let tiles = std::sync::atomic::AtomicUsize::new(0);
    let big = upscaler
        .enlarge(&img, &cancel, &|_, total| {
            tiles.store(total, std::sync::atomic::Ordering::Relaxed)
        })
        .unwrap();
    let tiles = tiles.into_inner();
    eprintln!(
        "{tiles} tiles, {} ms each",
        started.elapsed().as_millis() / tiles.max(1) as u128
    );
    assert_eq!(big.dimensions(), (w * FACTOR, h * FACTOR));
    assert_eq!(tiles, photo_ai::upscale::tiles(w, h));
    // True to the original: shrunk back, it matches closely (no shift, no colour swap).
    let back = image::imageops::resize(&big, w, h, image::imageops::FilterType::Triangle);
    let diff: f64 = back
        .pixels()
        .zip(img.pixels())
        .map(|(a, b)| {
            (0..3)
                .map(|c| (a[c] as f64 - b[c] as f64).abs())
                .sum::<f64>()
        })
        .sum::<f64>()
        / (w * h * 3) as f64;
    assert!(diff < 6.0, "mean difference {diff}");
    // No seams: across a tile border, neighbouring columns differ no more than
    // elsewhere in the smooth gradient.
    let border = 104 * FACTOR; // the first tile's kept width
    let y = 30 * FACTOR + 2; // a smooth row
    let step = |x: u32| (big.get_pixel(x, y)[0] as i32 - big.get_pixel(x - 1, y)[0] as i32).abs();
    assert!(step(border) <= 4, "seam: {}", step(border));
}

/// Finds and groups the faces in a scanned library: photos of three people (from the
/// folder `PHOTO_AI_TEST_FACES`, one sub-folder per person, like LFW), a group photo
/// of two of them, and a photo without faces. Then names, merges and corrects people.
#[test]
fn finds_people_in_a_library_and_lets_the_user_correct_them() {
    let Some(src) = models() else { return };
    let Some(faces_dir) = std::env::var_os("PHOTO_AI_TEST_FACES").map(PathBuf::from) else {
        eprintln!("skipped: set PHOTO_AI_TEST_FACES");
        return;
    };
    use photo_ai::faces::Faces;
    use photo_ai::people::People;
    use photo_core::db::Db;
    use photo_core::scan::Scanner;
    use photo_core::timeline::{self, View};
    use photo_core::{library, schema, thumbs};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("ai"));
    store.install_from(Pack::Runtime, &src).unwrap();
    store.install_from(Pack::People, &src).unwrap();
    let faces = Faces::load(&store, 2).unwrap();

    let photos = dir.path().join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let names = ["Colin_Powell", "Serena_Williams", "Hugo_Chavez"];
    for name in names {
        for i in 1..=5 {
            let file = format!("{name}_{i:04}.jpg");
            std::fs::copy(faces_dir.join(name).join(&file), photos.join(&file)).unwrap();
        }
    }
    // Two people side by side (photos 6, not among the singles).
    let left = image::open(faces_dir.join("Colin_Powell/Colin_Powell_0006.jpg"))
        .unwrap()
        .to_rgb8();
    let right = image::open(faces_dir.join("Serena_Williams/Serena_Williams_0006.jpg"))
        .unwrap()
        .to_rgb8();
    let mut pair = RgbImage::new(500, 250);
    image::imageops::replace(&mut pair, &left, 0, 0);
    image::imageops::replace(&mut pair, &right, 250, 0);
    pair.save(photos.join("pair.jpg")).unwrap();
    RgbImage::from_pixel(400, 300, Rgb([90, 140, 200]))
        .save(photos.join("sky.jpg"))
        .unwrap();

    let library = Db::open(&dir.path().join("lib.sqlite"), 2, schema::migrate_library).unwrap();
    let thumbs_db = Db::open(&dir.path().join("thumbs.sqlite"), 2, schema::migrate_thumbs).unwrap();
    let root = library::add_root(&library.write(), &photos).unwrap();
    let cancel = AtomicBool::new(false);
    Scanner {
        library: &library,
        thumbs: &thumbs_db,
        cancel: &cancel,
    }
    .scan_root(root, &|_| {})
    .unwrap();

    let people = People::default();
    let done = people
        .index_pending(
            &library,
            &thumbs_db,
            &faces,
            usize::MAX,
            &cancel,
            &|| false,
            &|| {},
        )
        .unwrap();
    assert_eq!(done, 17);
    let progress = photo_ai::people::progress(&library.read()).unwrap();
    assert_eq!((progress.indexed, progress.total), (17, 17));
    assert_eq!(
        people
            .index_pending(
                &library,
                &thumbs_db,
                &faces,
                usize::MAX,
                &cancel,
                &|| false,
                &|| {}
            )
            .unwrap(),
        0
    );

    let id_of = |file: &str| -> i64 {
        library
            .read()
            .query_row(
                "SELECT asset_id FROM files WHERE rel_path LIKE ?1",
                [format!("%{file}")],
                |r| r.get(0),
            )
            .unwrap()
    };
    let photos_of = |name: &str| -> Vec<i64> {
        (1..=5)
            .map(|i| id_of(&format!("{name}_{i:04}.jpg")))
            .collect()
    };
    let pair_id = id_of("pair.jpg");
    let in_view = |person: i64| -> Vec<i64> {
        let conn = library.read();
        let mut ids =
            timeline::asset_ids(&conn, View::Person(person), &Default::default()).unwrap();
        ids.sort();
        ids
    };

    // Three people, each in their five photos; the two in the pair photo in it too.
    let list = people.list(&library.read(), false).unwrap();
    assert_eq!(list.len(), 3, "{list:?}");
    let person_with = |asset: i64| {
        list.iter()
            .find(|p| in_view(p.id).contains(&asset))
            .unwrap()
            .id
    };
    for name in names {
        let mut expected = photos_of(name);
        let person = person_with(expected[0]);
        if name != "Hugo_Chavez" {
            expected.push(pair_id);
        }
        expected.sort();
        assert_eq!(in_view(person), expected, "{name}");
    }
    // Every face has a portrait.
    let face_ids: Vec<i64> = library
        .read()
        .prepare("SELECT id FROM ai_faces")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(face_ids.len(), 17);
    for id in &face_ids {
        assert!(thumbs::get_face(&thumbs_db.read(), *id).unwrap().is_some());
    }

    // Naming.
    let colin = person_with(photos_of("Colin_Powell")[0]);
    let serena = person_with(photos_of("Serena_Williams")[0]);
    let hugo = person_with(photos_of("Hugo_Chavez")[0]);
    people.rename(&library, colin, " Colin ").unwrap();
    let list = people.list(&library.read(), false).unwrap();
    assert_eq!(
        list[0].name.as_deref(),
        Some("Colin"),
        "named people come first"
    );
    assert_eq!(
        people.in_photo(&library.read(), pair_id).unwrap(),
        vec![(colin, "Colin".to_string())]
    );

    // "Not this person": the pair photo leaves Serena for good.
    assert_eq!(
        people.remove_photos(&library, serena, &[pair_id]).unwrap(),
        1
    );
    assert!(!in_view(serena).contains(&pair_id));
    library
        .write()
        .execute("DELETE FROM ai_face_scans WHERE asset_id = ?1", [pair_id])
        .unwrap();

    // Merging (as if Hugo were Colin); the name is kept.
    people.merge(&library, colin, &[hugo]).unwrap();
    let merged = people.get(&library.read(), colin).unwrap().unwrap();
    assert_eq!((merged.name.as_deref(), merged.photos), (Some("Colin"), 11));
    assert!(people.get(&library.read(), hugo).unwrap().is_none());

    // Hiding.
    people.set_hidden(&library, serena, true).unwrap();
    assert_eq!(people.list(&library.read(), false).unwrap().len(), 1);
    assert_eq!(people.list(&library.read(), true).unwrap()[0].id, serena);

    // A portrait is made again from the photo when the cache lost it.
    thumbs::clear_faces(&thumbs_db.write()).unwrap();
    let again = photo_ai::people::portrait_from_photo(&library, face_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(
        image::load_from_memory(&again).unwrap().width(),
        photo_ai::people::PORTRAIT
    );

    // Removing the feature forgets everything.
    people.clear(&library, &thumbs_db).unwrap();
    assert!(people.list(&library.read(), false).unwrap().is_empty());
    assert_eq!(
        photo_ai::people::progress(&library.read()).unwrap().indexed,
        0
    );
}

/// People seen whole, with the real model (`persons.onnx` in `PHOTO_AI_TEST_MODELS`):
/// of two photos with no face found, the one of a person (from `PHOTO_AI_TEST_FACES`)
/// leaves No people and a plain sky stays.
#[test]
fn photos_with_a_person_but_no_face_leave_no_people() {
    let Some(src) = models() else { return };
    let Some(faces_dir) = std::env::var_os("PHOTO_AI_TEST_FACES").map(PathBuf::from) else {
        eprintln!("skipped: set PHOTO_AI_TEST_FACES");
        return;
    };
    if !src.join(photo_ai::bodies::MODEL_FILE).exists() {
        eprintln!("skipped: no person model in PHOTO_AI_TEST_MODELS");
        return;
    }
    use photo_ai::bodies::{self, Bodies};
    use photo_core::db::Db;
    use photo_core::scan::Scanner;
    use photo_core::timeline::{self, View};
    use photo_core::{library, schema};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("ai"));
    store.install_from(Pack::Runtime, &src).unwrap();
    store.install_from(Pack::Bodies, &src).unwrap();
    let detector = Bodies::load(&store, 2).unwrap();

    let photos = dir.path().join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::copy(
        faces_dir.join("Colin_Powell/Colin_Powell_0001.jpg"),
        photos.join("person.jpg"),
    )
    .unwrap();
    RgbImage::from_pixel(400, 300, Rgb([90, 140, 200]))
        .save(photos.join("sky.jpg"))
        .unwrap();
    let library = Db::open(&dir.path().join("lib.sqlite"), 2, schema::migrate_library).unwrap();
    let thumbs_db = Db::open(&dir.path().join("thumbs.sqlite"), 2, schema::migrate_thumbs).unwrap();
    let root = library::add_root(&library.write(), &photos).unwrap();
    let cancel = AtomicBool::new(false);
    Scanner {
        library: &library,
        thumbs: &thumbs_db,
        cancel: &cancel,
    }
    .scan_root(root, &|_| {})
    .unwrap();
    // As if the face finder had missed the face.
    library
        .write()
        .execute(
            "INSERT INTO ai_face_scans (asset_id, model, faces) SELECT id, 'test', 0 FROM assets",
            [],
        )
        .unwrap();
    let id_of = |file: &str| -> i64 {
        library
            .read()
            .query_row(
                "SELECT asset_id FROM files WHERE rel_path = ?1",
                [file],
                |r| r.get(0),
            )
            .unwrap()
    };
    let no_people =
        || timeline::asset_ids(&library.read(), View::NoPeople, &Default::default()).unwrap();
    assert_eq!(no_people().len(), 2);

    let done = bodies::index_pending(&library, &detector, 100, &cancel, &|| false, &|| {}).unwrap();
    assert_eq!(done, 2);
    assert_eq!(no_people(), vec![id_of("sky.jpg")]);
    let progress = bodies::progress(&library.read()).unwrap();
    assert_eq!((progress.indexed, progress.total), (2, 2));
    // Nothing left to look at; removing the feature forgets what it found.
    assert_eq!(
        bodies::index_pending(&library, &detector, 100, &cancel, &|| false, &|| {}).unwrap(),
        0
    );
    bodies::clear(&library).unwrap();
    assert_eq!(no_people().len(), 2);
}

/// Downloads the real People, Bodies and Upscale packs (~80 MB) through the same path as
/// the app: plain files, and a package with two files kept from it.
#[test]
#[ignore]
fn downloads_and_installs_people_and_upscale() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let cancel = AtomicBool::new(false);
    for pack in [Pack::People, Pack::Bodies, Pack::Upscale] {
        store.install(pack, &cancel, &|_, _| {}).unwrap();
        store.verify(pack).unwrap();
        assert!(store.is_installed(pack));
    }
    assert!(
        store
            .path(Pack::Upscale, "real_esrgan_general_x4v3.data")
            .exists()
    );
    assert!(!store.path(Pack::Upscale, "upscale.zip.download").exists());
    // Text needs no download: installing just turns it on.
    store
        .install(Pack::Text, &cancel, &|_, _| panic!("nothing to download"))
        .unwrap();
    assert!(store.is_installed(Pack::Text));
    assert!(store.remove(Pack::Text, false).unwrap());
    assert!(!store.is_installed(Pack::Text));
}
