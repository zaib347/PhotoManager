use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use image::{Rgb, RgbImage};
use photo_core::db::Db;
use photo_core::scan::{Scanner, Summary};
use photo_core::{library, schema, thumbs, timeline};
use rusqlite::params;

struct Fixture {
    _dir: tempfile::TempDir,
    photos: std::path::PathBuf,
    library: Db,
    thumbs: Db,
    root_id: i64,
}

fn write_jpeg(path: &Path, seed: u8) {
    let img = RgbImage::from_fn(64, 48, |x, y| Rgb([seed, x as u8 * 3, y as u8 * 5]));
    fs::write(path, thumbs::encode_jpeg(&img).unwrap()).unwrap();
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let photos = dir.path().join("photos");
    fs::create_dir_all(photos.join("2019")).unwrap();
    fs::create_dir_all(photos.join(".cache")).unwrap();
    fs::create_dir_all(photos.join("@eaDir")).unwrap();

    write_jpeg(&photos.join("2019/IMG_20190312_101530.jpg"), 1);
    write_jpeg(&photos.join("2019/beach.jpg"), 2);
    RgbImage::from_pixel(40, 30, Rgb([9, 9, 9]))
        .save(photos.join("chart.png"))
        .unwrap();
    fs::write(photos.join("clip.mp4"), b"not really a video").unwrap();
    fs::write(photos.join("notes.txt"), b"ignored").unwrap();
    write_jpeg(&photos.join(".cache/hidden.jpg"), 3);
    write_jpeg(&photos.join("@eaDir/synology.jpg"), 4);

    let library = Db::open(
        &dir.path().join("library.sqlite"),
        2,
        schema::migrate_library,
    )
    .unwrap();
    let thumbs = Db::open(&dir.path().join("thumbs.sqlite"), 2, schema::migrate_thumbs).unwrap();
    let root_id = library::add_root(&library.write(), &photos).unwrap();
    Fixture {
        _dir: dir,
        photos,
        library,
        thumbs,
        root_id,
    }
}

impl Fixture {
    fn scan(&self) -> Summary {
        let cancel = AtomicBool::new(false);
        let scanner = Scanner {
            library: &self.library,
            thumbs: &self.thumbs,
            cancel: &cancel,
        };
        scanner.scan_root(self.root_id, &|_| {}).unwrap()
    }

    fn visible_assets(&self) -> i64 {
        timeline::stats(&self.library.read()).unwrap().assets
    }

    fn asset_of(&self, rel: &str) -> i64 {
        self.library
            .read()
            .query_row(
                "SELECT asset_id FROM files WHERE rel_path = ?1 AND missing_since IS NULL",
                params![rel],
                |r| r.get(0),
            )
            .unwrap()
    }
}

#[test]
fn indexes_supported_files_and_skips_excluded() {
    let f = fixture();
    let s = f.scan();
    assert_eq!((s.discovered, s.added, s.errors), (4, 4, 0), "{s:?}");
    assert!(s.completed && !s.missing_check_skipped);
    assert_eq!(f.visible_assets(), 4);

    let (src, kind): (u8, u8) = f
        .library
        .read()
        .query_row(
            "SELECT a.taken_src, a.media_kind FROM assets a JOIN files f ON f.asset_id = a.id
             WHERE f.rel_path = '2019/IMG_20190312_101530.jpg'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((src, kind), (2, 0), "date should come from the filename");

    let layout = timeline::layout(&f.library.read()).unwrap();
    assert_eq!(layout.counts.iter().sum::<u32>(), 4);
}

#[test]
fn rescan_skips_unchanged_files() {
    let f = fixture();
    f.scan();
    let s = f.scan();
    assert_eq!(
        (s.unchanged, s.added, s.updated, s.missing),
        (4, 0, 0, 0),
        "{s:?}"
    );
}

#[test]
fn rename_keeps_asset_and_user_data() {
    let f = fixture();
    f.scan();
    let asset = f.asset_of("2019/beach.jpg");
    f.library
        .write()
        .execute(
            "INSERT INTO user_meta (asset_id, favorite) VALUES (?1, 1)",
            params![asset],
        )
        .unwrap();

    fs::rename(
        f.photos.join("2019/beach.jpg"),
        f.photos.join("beach-renamed.jpg"),
    )
    .unwrap();
    let s = f.scan();
    assert_eq!(s.missing, 1, "old path is marked missing: {s:?}");
    assert_eq!(
        f.asset_of("beach-renamed.jpg"),
        asset,
        "moved file links to the same asset"
    );
    assert_eq!(f.visible_assets(), 4);
    let favorite: i64 = f
        .library
        .read()
        .query_row(
            "SELECT favorite FROM user_meta WHERE asset_id = ?1",
            params![asset],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(favorite, 1);
}

#[test]
fn identical_copy_joins_existing_asset() {
    let f = fixture();
    f.scan();
    fs::copy(
        f.photos.join("2019/beach.jpg"),
        f.photos.join("beach-copy.jpg"),
    )
    .unwrap();
    f.scan();
    assert_eq!(f.asset_of("beach-copy.jpg"), f.asset_of("2019/beach.jpg"));
    assert_eq!(
        f.visible_assets(),
        4,
        "a copy is another file of the same asset"
    );
}

#[test]
fn deleted_file_leaves_timeline_after_complete_scan() {
    let f = fixture();
    f.scan();
    fs::remove_file(f.photos.join("chart.png")).unwrap();
    let s = f.scan();
    assert_eq!(s.missing, 1);
    assert_eq!(f.visible_assets(), 3);
    let layout = timeline::layout(&f.library.read()).unwrap();
    assert_eq!(layout.counts.iter().sum::<u32>(), 3);
}

#[test]
fn edited_file_updates_in_place() {
    let f = fixture();
    f.scan();
    let asset = f.asset_of("2019/beach.jpg");
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_jpeg(&f.photos.join("2019/beach.jpg"), 200);
    let s = f.scan();
    assert_eq!((s.updated, s.added), (1, 0), "{s:?}");
    assert_eq!(f.asset_of("2019/beach.jpg"), asset);
}

#[test]
fn rejects_overlapping_roots() {
    let f = fixture();
    let err = library::add_root(&f.library.write(), &f.photos.join("2019")).unwrap_err();
    assert!(err.to_string().contains("overlaps"), "{err}");
}

#[test]
fn removing_root_drops_its_assets() {
    let f = fixture();
    f.scan();
    let removed = library::remove_root(&mut f.library.write(), f.root_id).unwrap();
    assert_eq!(removed.len(), 4);
    assert_eq!(f.visible_assets(), 0);
}

#[test]
fn thumbnails_from_files() {
    let f = fixture();
    let jpeg = thumbs::from_file(&f.photos.join("chart.png"), Some(6)).unwrap();
    let img = image::load_from_memory(&jpeg).unwrap();
    // 40x30 is smaller than the grid size, so it is kept, then rotated 90°.
    assert_eq!((img.width(), img.height()), (30, 40));
}

#[test]
fn clearing_test_data_keeps_real_assets() {
    let f = fixture();
    f.scan();
    photo_core::synth::generate(&mut f.library.write(), 500, 7).unwrap();
    assert_eq!(f.visible_assets(), 504);
    photo_core::synth::clear(&f.library.write()).unwrap();
    assert_eq!(f.visible_assets(), 4);
}
