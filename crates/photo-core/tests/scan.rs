use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use image::{Rgb, RgbImage};
use photo_core::db::Db;
use photo_core::scan::{Scanner, Summary};
use photo_core::timeline::View;
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

    let layout = timeline::layout(&f.library.read(), View::All, &Default::default()).unwrap();
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
    let layout = timeline::layout(&f.library.read(), View::All, &Default::default()).unwrap();
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

#[test]
fn favorites_and_hidden_views() {
    use photo_core::assets;
    let f = fixture();
    f.scan();
    let beach = f.asset_of("2019/beach.jpg");
    let chart = f.asset_of("chart.png");

    assets::set_favorite(&mut f.library.write(), &[beach], true).unwrap();
    assets::set_rating(&mut f.library.write(), &[beach], Some(4)).unwrap();
    assert!(assets::set_rating(&mut f.library.write(), &[beach], Some(9)).is_err());
    assets::set_hidden(&mut f.library.write(), &[chart], true).unwrap();

    let count = |view| -> u32 {
        timeline::layout(&f.library.read(), view, &Default::default())
            .unwrap()
            .counts
            .iter()
            .sum()
    };
    assert_eq!(count(View::All), 3, "hidden asset leaves the main timeline");
    assert_eq!(count(View::Favorites), 1);
    assert_eq!(count(View::Hidden), 1);
    assert_eq!(count(View::Videos), 1);

    let d = assets::details(&f.library.read(), beach).unwrap().unwrap();
    assert!(d.favorite && !d.hidden);
    assert_eq!(d.rating, Some(4));
    assert_eq!(d.files.len(), 1);
    assert!(d.files[0].online && d.files[0].path.ends_with("beach.jpg"));

    let day = timeline::day_assets(
        &f.library.read(),
        View::All,
        &Default::default(),
        &[d.local_day],
    )
    .unwrap();
    let i = day[0].ids.iter().position(|&id| id == beach).unwrap();
    assert_eq!(day[0].favorites[i], 1);
}

#[test]
fn albums_hold_assets_without_touching_files() {
    use photo_core::{assets, collections};
    let f = fixture();
    f.scan();
    let beach = f.asset_of("2019/beach.jpg");
    let chart = f.asset_of("chart.png");

    let album = collections::create_album(&mut f.library.write(), "  Holiday ", &[beach]).unwrap();
    assert!(collections::create_album(&mut f.library.write(), "   ", &[]).is_err());
    let added = collections::add_to_album(&mut f.library.write(), album, &[beach, chart]).unwrap();
    assert_eq!(added, 1, "beach was already in the album");

    let albums = collections::list_albums(&f.library.read()).unwrap();
    assert_eq!((albums[0].name.as_str(), albums[0].count), ("Holiday", 2));

    // Hidden items stay visible inside albums.
    assets::set_hidden(&mut f.library.write(), &[chart], true).unwrap();
    let count = |view| -> u32 {
        timeline::layout(&f.library.read(), view, &Default::default())
            .unwrap()
            .counts
            .iter()
            .sum()
    };
    assert_eq!(count(View::Album(album)), 2);

    collections::set_album_cover(&f.library.write(), album, chart).unwrap();
    assert!(
        collections::set_album_cover(&f.library.write(), album, f.asset_of("clip.mp4")).is_err()
    );
    collections::remove_from_album(&mut f.library.write(), album, &[chart]).unwrap();
    let albums = collections::list_albums(&f.library.read()).unwrap();
    assert_eq!(albums[0].count, 1);
    assert_eq!(
        albums[0].cover_asset_id,
        Some(beach),
        "removed cover falls back to newest"
    );

    let details = assets::details(&f.library.read(), beach).unwrap().unwrap();
    assert_eq!(details.albums.len(), 1);

    collections::delete_album(&f.library.write(), album).unwrap();
    assert!(
        collections::list_albums(&f.library.read())
            .unwrap()
            .is_empty()
    );
    assert!(f.photos.join("2019/beach.jpg").exists());
}

#[test]
fn tags_are_case_insensitive_and_cleaned_up() {
    use photo_core::{assets, collections};
    let f = fixture();
    f.scan();
    let beach = f.asset_of("2019/beach.jpg");
    let chart = f.asset_of("chart.png");

    let t1 = collections::add_tag(&mut f.library.write(), &[beach], "Family").unwrap();
    let t2 = collections::add_tag(&mut f.library.write(), &[chart], "family").unwrap();
    assert_eq!(t1, t2, "same tag regardless of case");

    let tags = collections::list_tags(&f.library.read()).unwrap();
    assert_eq!((tags.len(), tags[0].count), (1, 2));
    let count: u32 = timeline::layout(&f.library.read(), View::Tag(t1), &Default::default())
        .unwrap()
        .counts
        .iter()
        .sum();
    assert_eq!(count, 2);

    let other = collections::add_tag(&mut f.library.write(), &[beach], "Beach").unwrap();
    assert!(collections::rename_tag(&f.library.write(), other, "FAMILY").is_err());

    collections::remove_tag(&mut f.library.write(), &[beach, chart], t1).unwrap();
    let names: Vec<String> = collections::list_tags(&f.library.read())
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["Beach"], "an unused tag is deleted");
    let details = assets::details(&f.library.read(), beach).unwrap().unwrap();
    assert_eq!(details.tags.len(), 1);
}

#[test]
fn filters_narrow_any_view() {
    use photo_core::assets;
    use photo_core::timeline::{Filter, MediaFilter};
    let f = fixture();
    f.scan();
    let count = |view, filter: &Filter| -> u32 {
        timeline::layout(&f.library.read(), view, filter)
            .unwrap()
            .counts
            .iter()
            .sum()
    };
    let total = count(View::All, &Filter::default());
    assert_eq!(total, 4);

    let videos = Filter {
        media: Some(MediaFilter::Video),
        ..Default::default()
    };
    let photos = Filter {
        media: Some(MediaFilter::Photo),
        ..Default::default()
    };
    assert_eq!(count(View::All, &videos), 1);
    assert_eq!(count(View::All, &photos), 3);

    // Test images carry no camera, so they all match the "unknown camera" entry.
    let options = timeline::filter_options(&f.library.read()).unwrap();
    assert_eq!(options.cameras[0].model, "");
    let unknown = Filter {
        cameras: vec![String::new()],
        ..Default::default()
    };
    let canon = Filter {
        cameras: vec!["EOS 80D'; DROP TABLE assets; --".into()],
        ..Default::default()
    };
    assert_eq!(count(View::All, &unknown), 4);
    assert_eq!(
        count(View::All, &canon),
        0,
        "camera names are bound, never spliced"
    );

    let beach = f.asset_of("2019/beach.jpg");
    assets::set_rating(&mut f.library.write(), &[beach], Some(4)).unwrap();
    let rated = Filter {
        min_rating: Some(3),
        ..Default::default()
    };
    assert_eq!(count(View::All, &rated), 1);

    let day = assets::details(&f.library.read(), beach)
        .unwrap()
        .unwrap()
        .local_day;
    let same_day = Filter {
        from_day: Some(day),
        to_day: Some(day),
        ..Default::default()
    };
    let days = timeline::day_assets(&f.library.read(), View::All, &same_day, &[day]).unwrap();
    assert!(days[0].ids.contains(&beach));

    let wrong_root = Filter {
        root_ids: vec![f.root_id + 100],
        ..Default::default()
    };
    assert_eq!(count(View::All, &wrong_root), 0);
    let this_root = Filter {
        root_ids: vec![f.root_id],
        media: Some(MediaFilter::Photo),
        ..Default::default()
    };
    assert_eq!(count(View::All, &this_root), 3);
}

/// A minimal MP4 whose moov says `seconds` long (enough for the metadata reader).
fn write_mp4(path: &Path, seconds: u32) {
    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }
    let mut mvhd = vec![0u8; 100];
    mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
    mvhd[16..20].copy_from_slice(&(600 * seconds).to_be_bytes());
    let file = [
        boxed(b"ftyp", b"qt  "),
        boxed(b"moov", &boxed(b"mvhd", &mvhd)),
    ]
    .concat();
    fs::write(path, file).unwrap();
}

#[test]
fn live_photos_pair_and_release() {
    use photo_core::{assets, live};
    let f = fixture();
    write_jpeg(&f.photos.join("2019/IMG_0001.JPG"), 10);
    write_mp4(&f.photos.join("2019/IMG_0001.MOV"), 3);
    write_jpeg(&f.photos.join("2019/IMG_0002.JPG"), 11);
    write_mp4(&f.photos.join("2019/IMG_0002.MOV"), 30); // a real video, not a Live Photo
    write_mp4(&f.photos.join("IMG_0001.MOV"), 3); // same name, different folder
    f.scan();

    let photo = f.asset_of("2019/IMG_0001.JPG");
    let details = assets::details(&f.library.read(), photo).unwrap().unwrap();
    assert_eq!(details.media_kind, live::KIND_LIVE);
    assert_eq!(details.live_clip_id, Some(f.asset_of("2019/IMG_0001.MOV")));

    let other = assets::details(&f.library.read(), f.asset_of("2019/IMG_0002.JPG"))
        .unwrap()
        .unwrap();
    assert_eq!(
        (other.media_kind, other.live_clip_id),
        (0, None),
        "30 s video is not a clip"
    );

    // 4 fixture files + 2 photos + the 30 s video + the clip in another folder; the paired clip is hidden.
    assert_eq!(f.visible_assets(), 8);

    fs::remove_file(f.photos.join("2019/IMG_0001.JPG")).unwrap();
    f.scan();
    assert_eq!(
        f.visible_assets(),
        8,
        "photo gone, its clip is a normal video again"
    );
    let clip = assets::details(&f.library.read(), f.asset_of("2019/IMG_0001.MOV"))
        .unwrap()
        .unwrap();
    assert_eq!(clip.media_kind, 1);
}

#[test]
fn hiding_updates_day_counts_like_a_rebuild() {
    use photo_core::{assets, synth};
    let f = fixture();
    f.scan();
    synth::generate(&mut f.library.write(), 2_000, 3).unwrap();
    let sample: Vec<i64> = f
        .library
        .read()
        .prepare("SELECT id FROM assets ORDER BY id LIMIT 300")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    let buckets = || -> Vec<(i64, i64)> {
        f.library
            .read()
            .prepare("SELECT day, count FROM day_buckets ORDER BY day")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let check = |label: &str| {
        let incremental = buckets();
        timeline::rebuild_day_buckets(&f.library.write()).unwrap();
        assert_eq!(incremental, buckets(), "{label}");
    };

    assets::set_hidden(&mut f.library.write(), &sample, true).unwrap();
    check("after hiding");
    assets::set_hidden(&mut f.library.write(), &sample, true).unwrap();
    check("hiding twice changes nothing");
    assets::set_hidden(&mut f.library.write(), &sample[..100], false).unwrap();
    check("after unhiding some");
}
