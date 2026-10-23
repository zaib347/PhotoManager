//! Thumbnail store: JPEG blobs in a dedicated SQLite file, keyed by asset id.

use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

pub const GRID_SIZE: u32 = 256;
const JPEG_QUALITY: u8 = 80;

pub fn get(conn: &Connection, asset_id: i64) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare_cached("SELECT data FROM thumbs_256 WHERE asset_id = ?1")?;
    Ok(stmt.query_row(params![asset_id], |r| r.get(0)).optional()?)
}

pub fn put(conn: &Connection, asset_id: i64, data: &[u8]) -> Result<()> {
    let mut stmt =
        conn.prepare_cached("INSERT OR REPLACE INTO thumbs_256 (asset_id, data) VALUES (?1, ?2)")?;
    stmt.execute(params![asset_id, data])?;
    Ok(())
}

pub fn delete(conn: &Connection, asset_id: i64) -> Result<()> {
    conn.prepare_cached("DELETE FROM thumbs_256 WHERE asset_id = ?1")?
        .execute(params![asset_id])?;
    Ok(())
}

pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM thumbs_256", [])?;
    Ok(())
}

fn orient(img: &mut DynamicImage, orientation: Option<u8>) {
    if let Some(o) = orientation.and_then(Orientation::from_exif) {
        img.apply_orientation(o);
    }
}

/// Uses an EXIF-embedded JPEG as the thumbnail, rotated upright if needed.
/// Untouched bytes are returned when no rotation is required.
pub fn from_embedded(jpeg: &[u8], orientation: Option<u8>) -> Result<Vec<u8>> {
    if matches!(orientation, None | Some(1)) {
        image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg)?;
        return Ok(jpeg.to_vec());
    }
    let mut img = image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg)?;
    orient(&mut img, orientation);
    encode_jpeg(&img.to_rgb8())
}

/// Full decode of the original, scaled so the short edge is `GRID_SIZE` (the grid
/// crops to squares) with the long edge capped for panoramas.
pub fn from_file(path: &Path, orientation: Option<u8>) -> Result<Vec<u8>> {
    let img = image::open(path)?;
    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = GRID_SIZE as f32 / w.min(h) as f32;
    let (tw, th) = if scale < 1.0 {
        let cap = GRID_SIZE * 4;
        (
            ((w as f32 * scale) as u32).clamp(1, cap),
            ((h as f32 * scale) as u32).clamp(1, cap),
        )
    } else {
        (w, h)
    };
    let mut thumb = img.thumbnail(tw, th);
    orient(&mut thumb, orientation);
    encode_jpeg(&thumb.to_rgb8())
}

pub fn encode_jpeg(img: &RgbImage) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::with_capacity(16 * 1024));
    JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY).encode_image(img)?;
    Ok(buf.into_inner())
}

/// Stand-in for the real thumbnail pipeline during the M0 spike: a deterministic
/// gradient per asset, so scrolling exercises the same encode/store/serve path.
pub fn render_placeholder(asset_id: i64, is_video: bool) -> Result<Vec<u8>> {
    let h = (asset_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let base = [(h >> 8) as u8, (h >> 24) as u8, (h >> 40) as u8];
    let img = RgbImage::from_fn(GRID_SIZE, GRID_SIZE, |x, y| {
        let t = (x + y) as f32 / (2 * GRID_SIZE) as f32;
        let dim = if is_video { 0.55 } else { 1.0 };
        let c = |v: u8| ((v as f32 * (0.6 + 0.4 * t) + 40.0 * (1.0 - t)) * dim).min(255.0) as u8;
        Rgb([c(base[0]), c(base[1]), c(base[2])])
    });
    encode_jpeg(&img)
}
