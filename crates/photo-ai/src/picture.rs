//! Photos read for the AI features that need more than a thumbnail (faces, text):
//! upright, at a reduced size.

use std::path::Path;

use image::RgbImage;
use image::metadata::Orientation;
use photo_core::db::Db;
use photo_core::library::{self, AssetSource};

pub enum Picture {
    Ready(RgbImage),
    /// Unreadable: recorded so it isn't tried again.
    Failed,
    /// Drive not connected: another time.
    Later,
}

fn shrink(image: RgbImage, max_edge: u32) -> RgbImage {
    let (w, h) = image.dimensions();
    if w.max(h) <= max_edge {
        return image;
    }
    let scale = max_edge as f32 / w.max(h) as f32;
    image::imageops::resize(
        &image,
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

/// A photo upright (its EXIF turn applied, not the user's), with its long edge about
/// `edge` and at most `max_edge`. Positions found in it are kept relative to this.
pub fn read(path: &Path, orientation: Option<u8>, edge: u32, max_edge: u32) -> Option<RgbImage> {
    let upright = |mut img: image::DynamicImage| {
        if let Some(o) = orientation.and_then(Orientation::from_exif) {
            img.apply_orientation(o);
        }
        img.to_rgb8()
    };
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // JPEGs decode much faster at a reduced size (the OS decoder doesn't turn them).
    if matches!(ext.as_str(), "jpg" | "jpeg")
        && let Ok(img) = photo_core::os_thumb::reduced_image(path, edge)
    {
        return Some(shrink(upright(img), max_edge));
    }
    if let Ok(img) = photo_core::raw::open(path) {
        return Some(shrink(upright(img), max_edge));
    }
    // The OS decoders (HEIC…) hand over pixels already upright.
    photo_core::os_thumb::thumbnail(path, edge)
        .ok()
        .map(|img| shrink(img, max_edge))
}

/// A library photo read as `read` does.
pub fn picture(library: &Db, id: i64, edge: u32, max_edge: u32) -> Picture {
    match library::asset_source(&library.read(), id) {
        Ok(Some(AssetSource::File {
            path, orientation, ..
        })) => read(&path, orientation, edge, max_edge).map_or(Picture::Failed, Picture::Ready),
        Ok(Some(AssetSource::Unavailable { .. })) => Picture::Later,
        _ => Picture::Failed,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The reduced JPEG decode isn't turned by the OS, so the EXIF turn is applied once.
    #[test]
    fn jpegs_come_upright() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("turned.jpg");
        let img = RgbImage::from_fn(300, 200, |x, _| {
            image::Rgb([if x < 150 { 0 } else { 255 }; 3])
        });
        img.save(&path).unwrap();
        // Orientation 6: shown turned right, so 200 wide and 300 tall.
        let read = read(&path, Some(6), 1600, 2400).unwrap();
        assert_eq!(read.dimensions(), (200, 300));
        // The left half (black) is now the top.
        assert!(read.get_pixel(100, 20)[0] < 50 && read.get_pixel(100, 280)[0] > 200);
    }
}
