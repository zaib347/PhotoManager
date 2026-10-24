use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use image::{Rgb, RgbImage};
use photo_core::db::Db;
use photo_core::scan::Scanner;
use photo_core::{imports, library, schema, thumbs};
use rusqlite::params;

const MARCH_12_2019_10_15: i64 = 1_552_385_700_000; // UTC
const NOV_2_2019_16_38: i64 = 1_572_712_680_000; // UTC

fn write_jpeg(path: &Path, seed: u8) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = RgbImage::from_fn(32, 24, |x, y| Rgb([seed, x as u8 * 7, y as u8 * 9]));
    fs::write(path, thumbs::encode_jpeg(&img).unwrap()).unwrap();
}

fn takeout_json(seconds: i64, extra: &str) -> String {
    format!(
        r#"{{"title": "x", "photoTakenTime": {{"timestamp": "{seconds}", "formatted": "…"}}{extra}}}"#
    )
}

struct Fixture {
    _dir: tempfile::TempDir,
    library: Db,
    root_id: i64,
}

impl Fixture {
    fn asset(&self, rel: &str) -> i64 {
        self.library
            .read()
            .query_row(
                "SELECT asset_id FROM files WHERE rel_path = ?1",
                params![rel],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    /// (taken_at, taken_src, lat, favorite, hidden, note)
    fn state(&self, id: i64) -> (i64, u8, Option<f64>, bool, bool, Option<String>) {
        self.library
            .read()
            .query_row(
                "SELECT a.taken_at, a.taken_src, a.lat, COALESCE(u.favorite, 0), COALESCE(u.hidden, 0), u.note
                 FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id WHERE a.id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap()
    }

    fn album(&self, name: &str) -> Option<i64> {
        self.library
            .read()
            .query_row(
                "SELECT COUNT(*) FROM album_assets aa JOIN albums al ON al.id = aa.album_id WHERE al.name = ?1",
                params![name],
                |r| r.get(0),
            )
            .ok()
            .filter(|&n: &i64| n > 0)
    }
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("export");
    let year = root.join("Takeout/Google Photos/Photos from 2019");
    write_jpeg(&year.join("IMG_0001.jpg"), 1);
    fs::write(
        year.join("IMG_0001.jpg.supplemental-metadata.json"),
        takeout_json(
            MARCH_12_2019_10_15 / 1000,
            r#", "description": "Eiffel tower", "favorited": true,
               "geoData": {"latitude": 48.8584, "longitude": 2.2945, "altitude": 0}"#,
        ),
    )
    .unwrap();
    write_jpeg(&year.join("IMG_0001-edited.jpg"), 2);
    // A long name: Google cut the JSON's name short.
    let long = "PXL_holiday_with_a_really_long_file_name_01.jpg";
    write_jpeg(&year.join(long), 3);
    fs::write(
        year.join("PXL_holiday_with_a_really_long_file_name_01.jpg.supplemen.json"),
        takeout_json(MARCH_12_2019_10_15 / 1000 + 60, ""),
    )
    .unwrap();
    // Two files of the same name: the second is dup(1).jpg, its JSON dup.jpg(1).json.
    write_jpeg(&year.join("dup.jpg"), 4);
    write_jpeg(&year.join("dup(1).jpg"), 5);
    fs::write(
        year.join("dup.jpg.json"),
        takeout_json(MARCH_12_2019_10_15 / 1000 + 120, ""),
    )
    .unwrap();
    fs::write(
        year.join("dup.jpg(1).json"),
        takeout_json(MARCH_12_2019_10_15 / 1000 + 180, ""),
    )
    .unwrap();
    // An album folder: a copy of IMG_0001 and one more photo.
    let album = root.join("Takeout/Google Photos/Beach trip");
    write_jpeg(&album.join("IMG_0001.jpg"), 1);
    write_jpeg(&album.join("sea.jpg"), 6);
    fs::write(album.join("metadata.json"), r#"{"title": "Beach trip"}"#).unwrap();
    fs::write(
        year.join("metadata.json"),
        r#"{"title": "Photos from 2019"}"#,
    )
    .unwrap();

    // iCloud.
    let part = root.join("iCloud Photos Part 1 of 1");
    write_jpeg(&part.join("Photos/IMG_5000.jpg"), 7);
    write_jpeg(&part.join("Photos/IMG_5001.jpg"), 8);
    fs::write(
        part.join("Photos/Photo Details.csv"),
        "imgName,fileChecksum,favorite,hidden,deleted,originalCreationDate,viewCount,importDate\r\n\
         IMG_5000.jpg,abc=,yes,yes,no,\"Saturday November 2,2019 4:38 PM GMT\",3,\"Saturday November 2,2019 4:38 PM GMT\"\r\n\
         IMG_5001.jpg,def=,no,no,no,\"Saturday November 2,2019 4:40 PM GMT\",0,\r\n\
         IMG_9999.jpg,ghi=,yes,no,yes,\"Saturday November 2,2019 4:41 PM GMT\",0,\r\n",
    )
    .unwrap();
    fs::create_dir_all(part.join("Albums")).unwrap();
    fs::write(
        part.join("Albums/Family.csv"),
        "Images\nIMG_5000.jpg\nIMG_5001.jpg\n",
    )
    .unwrap();

    let library = Db::open(
        &dir.path().join("library.sqlite"),
        2,
        schema::migrate_library,
    )
    .unwrap();
    let thumbs_db = Db::open(&dir.path().join("thumbs.sqlite"), 2, schema::migrate_thumbs).unwrap();
    let root_id = library::add_root(&library.write(), &root).unwrap();
    let cancel = AtomicBool::new(false);
    Scanner {
        library: &library,
        thumbs: &thumbs_db,
        cancel: &cancel,
    }
    .scan_root(root_id, &|_| {})
    .unwrap();
    Fixture {
        _dir: dir,
        library,
        root_id,
    }
}

#[test]
fn takeout_and_icloud_metadata_fill_blanks_and_undo() {
    let f = fixture();
    let y = "Takeout/Google Photos/Photos from 2019";
    let first = f.asset(&format!("{y}/IMG_0001.jpg"));
    assert_eq!(
        first,
        f.asset("Takeout/Google Photos/Beach trip/IMG_0001.jpg"),
        "copies join"
    );
    let before = f.state(first);
    assert_eq!(before.1, 3, "no date in the file: modified time");

    let report = imports::run(&f.library, f.root_id).unwrap();
    assert_eq!(report.sources, vec!["google", "icloud"]);
    // Takeout JSONs and iCloud rows; rows of deleted photos are skipped.
    assert_eq!((report.found, report.matched), (4 + 2, 4 + 2));

    let (taken, src, lat, favorite, hidden, note) = f.state(first);
    assert_eq!((taken, src), (MARCH_12_2019_10_15, imports::SRC_IMPORT));
    assert!((lat.unwrap() - 48.8584).abs() < 1e-9);
    assert!(favorite && !hidden);
    assert_eq!(note.as_deref(), Some("Eiffel tower"));
    // The edited copy shares its original's metadata.
    assert_eq!(
        f.state(f.asset(&format!("{y}/IMG_0001-edited.jpg"))).0,
        MARCH_12_2019_10_15
    );
    let long = f.asset(&format!(
        "{y}/PXL_holiday_with_a_really_long_file_name_01.jpg"
    ));
    assert_eq!(f.state(long).0, MARCH_12_2019_10_15 + 60_000);
    assert_eq!(
        f.state(f.asset(&format!("{y}/dup.jpg"))).0,
        MARCH_12_2019_10_15 + 120_000
    );
    assert_eq!(
        f.state(f.asset(&format!("{y}/dup(1).jpg"))).0,
        MARCH_12_2019_10_15 + 180_000
    );
    assert_eq!(f.album("Beach trip"), Some(2));
    assert_eq!(
        f.album("Photos from 2019"),
        None,
        "year folders aren't albums"
    );

    let icloud = f.asset("iCloud Photos Part 1 of 1/Photos/IMG_5000.jpg");
    let (taken, _, _, favorite, hidden, _) = f.state(icloud);
    assert_eq!(taken, NOV_2_2019_16_38);
    assert!(favorite && hidden);
    assert_eq!(f.album("Family"), Some(2));
    assert_eq!(report.albums, 2);
    assert_eq!(report.hidden, 1);

    // The place was looked up.
    let place: Option<i64> = f
        .library
        .read()
        .query_row(
            "SELECT place_id FROM assets WHERE id = ?1",
            params![first],
            |r| r.get(0),
        )
        .unwrap();
    assert!(place.is_some_and(|p| p > 0));

    // Running it again changes nothing.
    let again = imports::run(&f.library, f.root_id).unwrap();
    assert_eq!(again.changed(), 0, "{again:?}");
    assert_eq!(again.albums, 0, "albums are reused");

    // Undo puts everything back.
    assert!(imports::undo(&f.library, report.id).unwrap() > 0);
    assert_eq!(f.state(first), before);
    let (_, src, _, favorite, hidden, _) = f.state(icloud);
    assert!(!favorite && !hidden && src == 3);
    assert_eq!(f.album("Beach trip"), None);
    assert_eq!(f.album("Family"), None);
    let list = imports::list(&f.library.read(), f.root_id).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[1].undone && list[1].report.dates == report.dates);
    assert_eq!(
        imports::undo(&f.library, report.id).unwrap(),
        0,
        "only once"
    );
}

#[test]
fn a_folder_without_export_metadata_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plain");
    write_jpeg(&root.join("a.jpg"), 1);
    let library = Db::open(
        &dir.path().join("library.sqlite"),
        1,
        schema::migrate_library,
    )
    .unwrap();
    let root_id = library::add_root(&library.write(), &root).unwrap();
    assert!(imports::run(&library, root_id).is_err());
}

#[test]
fn icloud_dates_parse() {
    assert_eq!(
        imports::parse_icloud_date("Saturday November 2,2019 4:38 PM GMT"),
        Some(NOV_2_2019_16_38)
    );
    assert_eq!(
        imports::parse_icloud_date("November 2, 2019 12:05 AM"),
        Some(NOV_2_2019_16_38 - 16 * 3_600_000 - 33 * 60_000)
    );
    assert_eq!(imports::parse_icloud_date("yesterday"), None);
}
