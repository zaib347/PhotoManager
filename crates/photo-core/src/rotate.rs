//! Rotating photos without touching their files. A turn is kept in `user_meta.turns`
//! (quarter turns clockwise, 0–3) and applied on top of the file's EXIF orientation
//! wherever pixels are produced: thumbnails, the viewer, similar-photo hashing, export.
//! Nothing is re-encoded, so turning back and forth never loses quality.

use image::DynamicImage;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Db;
use crate::{Result, thumbs};

/// EXIF orientation (1–8) as "mirror left-right first, then turn clockwise k times".
fn split(orientation: Option<u8>) -> (bool, u8) {
    match orientation {
        Some(2) => (true, 0),
        Some(3) => (false, 2),
        Some(4) => (true, 2),
        Some(5) => (true, 3),
        Some(6) => (false, 1),
        Some(7) => (true, 1),
        Some(8) => (false, 3),
        _ => (false, 0),
    }
}

/// The EXIF orientation that shows the file as it reads after `turns` more quarter
/// turns clockwise.
pub fn compose(orientation: Option<u8>, turns: u8) -> Option<u8> {
    let (mirror, k) = split(orientation);
    let k = (k + turns) % 4;
    let value = match (mirror, k) {
        (false, 0) => return orientation.filter(|_| turns == 0),
        (false, 1) => 6,
        (false, 2) => 3,
        (false, _) => 8,
        (true, 0) => 2,
        (true, 1) => 7,
        (true, 2) => 4,
        (true, _) => 5,
    };
    Some(value)
}

/// `image` turned clockwise `turns` quarter turns.
pub fn turn(image: DynamicImage, turns: u8) -> DynamicImage {
    match turns % 4 {
        1 => image.rotate90(),
        2 => image.rotate180(),
        3 => image.rotate270(),
        _ => image,
    }
}

/// The user's turns for one asset (0 when never turned).
pub fn turns_of(conn: &Connection, asset_id: i64) -> Result<u8> {
    Ok(conn
        .prepare_cached("SELECT turns FROM user_meta WHERE asset_id = ?1")?
        .query_row(params![asset_id], |r| r.get::<_, u8>(0))
        .optional()?
        .unwrap_or(0))
}

/// Turns photos by `quarters` (1 clockwise, -1 counter-clockwise, 2 upside down).
/// Videos are left alone. Cached thumbnails are turned in place, so the grid doesn't
/// wait for a fresh decode. Returns each turned photo's new total turns.
pub fn rotate(library: &Db, thumbs_db: &Db, ids: &[i64], quarters: i8) -> Result<Vec<(i64, u8)>> {
    let delta = quarters.rem_euclid(4) as u8;
    let mut turned = Vec::with_capacity(ids.len());
    {
        let mut conn = library.write();
        let tx = conn.transaction()?;
        {
            let mut kind = tx.prepare_cached("SELECT media_kind FROM assets WHERE id = ?1")?;
            let mut current =
                tx.prepare_cached("SELECT turns FROM user_meta WHERE asset_id = ?1")?;
            let mut set = tx.prepare_cached(
                "INSERT INTO user_meta (asset_id, turns) VALUES (?1, ?2)
                 ON CONFLICT (asset_id) DO UPDATE SET turns = excluded.turns",
            )?;
            for &id in ids {
                let media: Option<u8> = kind.query_row(params![id], |r| r.get(0)).optional()?;
                if !matches!(media, Some(k) if k != 1) {
                    continue;
                }
                let now: u8 = current
                    .query_row(params![id], |r| r.get(0))
                    .optional()?
                    .unwrap_or(0);
                let next = (now + delta) % 4;
                set.execute(params![id, next])?;
                crate::edit::turn_crop(&tx, id, delta)?;
                turned.push((id, next));
            }
        }
        tx.commit()?;
    }
    if delta != 0 {
        let mut store = thumbs_db.write();
        let tx = store.transaction()?;
        for &(id, _) in &turned {
            let Some(jpeg) = thumbs::get(&tx, id)? else {
                continue;
            };
            // A thumbnail that won't decode is dropped and made again from the file.
            match image::load_from_memory(&jpeg) {
                Ok(img) => {
                    thumbs::put(&tx, id, &thumbs::encode_jpeg(&turn(img, delta).to_rgb8())?)?
                }
                Err(_) => thumbs::delete(&tx, id)?,
            }
        }
        tx.commit()?;
    }
    Ok(turned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::metadata::Orientation;
    use image::{Rgb, RgbImage};

    /// A 3×2 image with every pixel different, to compare orientations exactly.
    fn sample() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(3, 2, |x, y| {
            Rgb([x as u8 * 50, y as u8 * 50, 7])
        }))
    }

    fn oriented(orientation: Option<u8>) -> RgbImage {
        let mut img = sample();
        if let Some(o) = orientation.and_then(Orientation::from_exif) {
            img.apply_orientation(o);
        }
        img.to_rgb8()
    }

    #[test]
    fn rotating_saves_turns_and_turns_the_cached_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let library = Db::open(
            &dir.path().join("lib.sqlite"),
            1,
            crate::schema::migrate_library,
        )
        .unwrap();
        let thumbs_db = Db::open(
            &dir.path().join("thumbs.sqlite"),
            1,
            crate::schema::migrate_thumbs,
        )
        .unwrap();
        library
            .write()
            .execute_batch(
                "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
                 VALUES (1, 0, 0, 0, 0, 0), (2, 1, 0, 0, 0, 0)",
            )
            .unwrap();
        let wide = thumbs::encode_jpeg(&RgbImage::new(300, 200)).unwrap();
        thumbs::put(&thumbs_db.write(), 1, &wide).unwrap();

        let turned = rotate(&library, &thumbs_db, &[1, 2], 1).unwrap();
        assert_eq!(turned, vec![(1, 1)], "the video is left alone");
        let thumb =
            image::load_from_memory(&thumbs::get(&thumbs_db.read(), 1).unwrap().unwrap()).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (200, 300));

        assert_eq!(
            rotate(&library, &thumbs_db, &[1], -1).unwrap(),
            vec![(1, 0)]
        );
        assert!(turned_list(&library).is_empty(), "back to upright");
        rotate(&library, &thumbs_db, &[1], -1).unwrap();
        assert_eq!(turned_list(&library), vec![(1, 3)]);
        assert_eq!(turns_of(&library.read(), 1).unwrap(), 3);
    }

    fn turned_list(library: &Db) -> Vec<(i64, u8)> {
        crate::edit::adjusted(&library.read())
            .unwrap()
            .into_iter()
            .filter(|&(_, turns, _)| turns != 0)
            .map(|(id, turns, _)| (id, turns))
            .collect()
    }

    #[test]
    fn composed_orientation_matches_turning_the_upright_image() {
        for exif in [
            None,
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
            Some(6),
            Some(7),
            Some(8),
        ] {
            for turns in 0..4 {
                let expected = turn(DynamicImage::ImageRgb8(oriented(exif)), turns).to_rgb8();
                let got = oriented(compose(exif, turns));
                assert_eq!(got, expected, "exif {exif:?}, {turns} turns");
            }
        }
        assert_eq!(compose(None, 0), None);
        assert_eq!(compose(Some(6), 3), None, "turned back to upright");
    }
}
