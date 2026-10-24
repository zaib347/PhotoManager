//! Collages and print layouts: 2–9 photos arranged on one canvas (the layout comes
//! from the frontend's templates), rendered at full size and saved as a JPEG with the
//! first photo's capture date and camera, and the print resolution set.
//!
//! Each cell shows its photo (with the user's turns and edit) cropped to fill it:
//! scaled to cover the cell, times `zoom`, with `pan` choosing which part shows (0 is
//! the left or top edge, 1 the right or bottom). The frontend's preview uses the same
//! rules, so the saved file matches it.

use std::path::Path;

use image::{Rgb, RgbImage};
use rayon::prelude::*;
use serde::Deserialize;

use crate::db::Db;
use crate::library::{self, AssetSource};
use crate::{Error, Result};

pub const MAX_PHOTOS: usize = 9;
/// Biggest canvas side: A3 at 300 dpi is 4961 px.
const MAX_SIDE: u32 = 8000;

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    /// Position and size as fractions of the canvas.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub asset_id: i64,
    #[serde(default = "half")]
    pub pan_x: f32,
    #[serde(default = "half")]
    pub pan_y: f32,
    /// 1 fills the cell; more zooms in.
    #[serde(default = "one")]
    pub zoom: f32,
}

fn half() -> f32 {
    0.5
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collage {
    pub width: u32,
    pub height: u32,
    /// `#rrggbb`.
    pub background: String,
    /// Gap around and between photos, as a fraction of the canvas's shorter side.
    pub spacing: f32,
    /// Corner rounding, as a fraction of the canvas's shorter side.
    pub radius: f32,
    /// Print resolution written to the file.
    pub dpi: u16,
    pub cells: Vec<Cell>,
}

fn parse_color(s: &str) -> Rgb<u8> {
    let hex = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("ff"), 16).unwrap_or(255);
    if hex.len() == 6 {
        Rgb([byte(0), byte(2), byte(4)])
    } else {
        Rgb([255, 255, 255])
    }
}

/// A cell's rectangle in pixels: (x, y, w, h).
fn cell_rect(c: &Cell, w: u32, h: u32, gap: f32) -> (u32, u32, u32, u32) {
    // The gap: half of it inside the canvas edge, half around each cell, so edges and
    // the space between photos match.
    let (cw, ch) = (w as f32 - gap, h as f32 - gap);
    let x0 = gap / 2.0 + c.x * cw + gap / 2.0;
    let y0 = gap / 2.0 + c.y * ch + gap / 2.0;
    let x1 = gap / 2.0 + (c.x + c.w) * cw - gap / 2.0;
    let y1 = gap / 2.0 + (c.y + c.h) * ch - gap / 2.0;
    let (x0, y0) = (x0.round().max(0.0) as u32, y0.round().max(0.0) as u32);
    let (x1, y1) = (
        (x1.round() as u32).min(w).max(x0 + 1),
        (y1.round() as u32).min(h).max(y0 + 1),
    );
    (x0, y0, x1 - x0, y1 - y0)
}

/// The part of an `iw`×`ih` image a `cw`×`ch` cell shows: (x, y, w, h) in image pixels.
pub fn crop_for(
    iw: f32,
    ih: f32,
    cw: f32,
    ch: f32,
    pan_x: f32,
    pan_y: f32,
    zoom: f32,
) -> (f32, f32, f32, f32) {
    let scale = (cw / iw).max(ch / ih) * zoom.clamp(1.0, 10.0);
    let (sw, sh) = (cw / scale, ch / scale);
    let x = (iw - sw) * pan_x.clamp(0.0, 1.0);
    let y = (ih - sh) * pan_y.clamp(0.0, 1.0);
    (x, y, sw, sh)
}

/// One cell's photo, cropped and sized to the cell.
fn cell_image(
    source: &Path,
    look: &crate::edit::Look,
    c: &Cell,
    cw: u32,
    ch: u32,
) -> Result<RgbImage> {
    // Decoded big enough for the zoomed crop to stay sharp.
    let need = (cw.max(ch) as f32 * c.zoom.clamp(1.0, 10.0) * 1.5).min(12_000.0) as u32;
    let base = crate::export::preview_base(source, look, need.max(64))?;
    let image = base.image();
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let (x, y, w, h) = crop_for(iw, ih, cw as f32, ch as f32, c.pan_x, c.pan_y, c.zoom);
    let cropped = image.crop_imm(
        x.floor() as u32,
        y.floor() as u32,
        (w.round() as u32).max(1),
        (h.round() as u32).max(1),
    );
    crate::export::resize_exact(&cropped.to_rgb8(), cw, ch)
}

/// How much of the pixel at (`x`, `y`) of a `w`×`h` rectangle with corners rounded by
/// `r` is inside it, 0 to 1 (edges anti-aliased).
fn coverage(x: u32, y: u32, w: u32, h: u32, r: f32) -> f32 {
    if r < 0.5 {
        return 1.0;
    }
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let cx = if px < r {
        r
    } else if px > w as f32 - r {
        w as f32 - r
    } else {
        return 1.0;
    };
    let cy = if py < r {
        r
    } else if py > h as f32 - r {
        h as f32 - r
    } else {
        return 1.0;
    };
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// Renders the collage at its full size.
pub fn render(db: &Db, collage: &Collage) -> Result<RgbImage> {
    let (w, h) = (collage.width, collage.height);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(Error::Invalid("That size can't be made".into()));
    }
    if collage.cells.is_empty() || collage.cells.len() > MAX_PHOTOS {
        return Err(Error::Invalid(format!(
            "A collage holds 1 to {MAX_PHOTOS} photos"
        )));
    }
    let short = w.min(h) as f32;
    let gap = (collage.spacing.clamp(0.0, 0.2) * short).round();
    let radius = collage.radius.clamp(0.0, 0.5) * short;
    let background = parse_color(&collage.background);

    // Sources first (database), then decoding in parallel.
    let mut jobs = Vec::with_capacity(collage.cells.len());
    {
        let conn = db.read();
        for c in &collage.cells {
            match library::asset_source(&conn, c.asset_id)? {
                Some(AssetSource::File {
                    path, look, kind, ..
                }) if kind != 1 => jobs.push((c, path, look)),
                Some(AssetSource::File { .. }) => {
                    return Err(Error::Invalid("Videos can't go in a collage".into()));
                }
                _ => {
                    return Err(Error::Invalid(
                        "A photo's drive isn't connected, so the collage can't be made".into(),
                    ));
                }
            }
        }
    }
    let cells: Vec<((u32, u32, u32, u32), RgbImage)> = jobs
        .par_iter()
        .map(|(c, path, look)| {
            let rect = cell_rect(c, w, h, gap);
            Ok((rect, cell_image(path, look, c, rect.2, rect.3)?))
        })
        .collect::<Result<_>>()?;

    let mut canvas = RgbImage::from_pixel(w, h, background);
    for ((x0, y0, cw, ch), img) in cells {
        let r = radius.min(cw.min(ch) as f32 / 2.0);
        for y in 0..ch.min(h - y0) {
            for x in 0..cw.min(w - x0) {
                let a = coverage(x, y, cw, ch, r);
                if a <= 0.0 {
                    continue;
                }
                let src = img.get_pixel(x, y).0;
                let dst = canvas.get_pixel_mut(x0 + x, y0 + y);
                for (d, s) in dst.0.iter_mut().zip(src) {
                    *d = (s as f32 * a + *d as f32 * (1.0 - a) + 0.5) as u8;
                }
            }
        }
    }
    Ok(canvas)
}

/// Renders and saves the collage as a JPEG at `path`. The first photo's EXIF (date,
/// camera; not its location) goes in, so the collage sorts with its photos.
pub fn save(db: &Db, collage: &Collage, path: &Path) -> Result<()> {
    let canvas = render(db, collage)?;
    let encoded = crate::jpeg::encode_print(&canvas, 92, collage.dpi.clamp(72, 1200))?;
    let first = collage.cells.first().map(|c| c.asset_id);
    let exif = match first.map(|id| library::asset_source(&db.read(), id)) {
        Some(Ok(Some(AssetSource::File { path, .. }))) => crate::export::exif_for_copy(&path, true),
        _ => None,
    };
    std::fs::write(path, crate::export::attach_jpeg(encoded, exif))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_fill_the_canvas_with_even_gaps() {
        let left = Cell {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
            asset_id: 1,
            pan_x: 0.5,
            pan_y: 0.5,
            zoom: 1.0,
        };
        let right = Cell {
            x: 0.5,
            ..left.clone()
        };
        let a = cell_rect(&left, 1000, 500, 20.0);
        let b = cell_rect(&right, 1000, 500, 20.0);
        assert_eq!(a, (20, 20, 470, 460));
        assert_eq!(b, (510, 20, 470, 460));
        assert_eq!(b.0 - (a.0 + a.2), 20, "gap between equals the edge");
        assert_eq!(cell_rect(&left, 1000, 500, 0.0), (0, 0, 500, 500));
    }

    #[test]
    fn crops_cover_the_cell_and_pan_within_the_photo() {
        // A landscape photo in a square cell: full height, middle of the width.
        let (x, y, w, h) = crop_for(400.0, 200.0, 100.0, 100.0, 0.5, 0.5, 1.0);
        assert_eq!((x, y, w, h), (100.0, 0.0, 200.0, 200.0));
        assert_eq!(crop_for(400.0, 200.0, 100.0, 100.0, 0.0, 0.5, 1.0).0, 0.0);
        assert_eq!(crop_for(400.0, 200.0, 100.0, 100.0, 1.0, 0.5, 1.0).0, 200.0);
        // Zoomed twice: half the size, still centred.
        assert_eq!(
            crop_for(400.0, 200.0, 100.0, 100.0, 0.5, 0.5, 2.0),
            (150.0, 50.0, 100.0, 100.0)
        );
    }

    #[test]
    fn rounded_corners_are_soft() {
        assert_eq!(coverage(0, 0, 100, 100, 0.0), 1.0);
        assert_eq!(coverage(0, 0, 100, 100, 20.0), 0.0);
        assert_eq!(coverage(50, 50, 100, 100, 20.0), 1.0);
        assert_eq!(coverage(50, 0, 100, 100, 20.0), 1.0, "edges are straight");
        let edge = coverage(6, 5, 100, 100, 20.0);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
    }

    #[test]
    fn renders_and_saves_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            1,
            crate::schema::migrate_library,
        )
        .unwrap();
        let root = library::add_root(&db.write(), &photos).unwrap();
        for (id, color) in [(1i64, [200u8, 0, 0]), (2, [0, 0, 200])] {
            let name = format!("{id}.jpg");
            let img = RgbImage::from_pixel(300, 200, Rgb(color));
            std::fs::write(
                photos.join(&name),
                crate::jpeg::encode(&img, 90, crate::jpeg::Purpose::Keep).unwrap(),
            )
            .unwrap();
            let conn = db.write();
            conn.execute(
                "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at) VALUES (?1, 0, 0, 0, 0, 0)",
                rusqlite::params![id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO files (root_id, rel_path, size, mtime, asset_id) VALUES (?1, ?2, 1, 0, ?3)",
                rusqlite::params![root, name, id],
            )
            .unwrap();
        }
        let cell = |id: i64, x: f32| Cell {
            x,
            y: 0.0,
            w: 0.5,
            h: 1.0,
            asset_id: id,
            pan_x: 0.5,
            pan_y: 0.5,
            zoom: 1.0,
        };
        let collage = Collage {
            width: 600,
            height: 300,
            background: "#00ff00".into(),
            spacing: 0.1,
            radius: 0.05,
            dpi: 300,
            cells: vec![cell(1, 0.0), cell(2, 0.5)],
        };
        let image = render(&db, &collage).unwrap();
        assert_eq!(
            image.get_pixel(5, 5).0,
            [0, 255, 0],
            "background in the gap"
        );
        let red = image.get_pixel(150, 150).0;
        let blue = image.get_pixel(450, 150).0;
        assert!(red[0] > 180 && red[2] < 30, "{red:?}");
        assert!(blue[2] > 180 && blue[0] < 30, "{blue:?}");

        let out = dir.path().join("collage.jpg");
        save(&db, &collage, &out).unwrap();
        let saved = image::open(&out).unwrap();
        assert_eq!((saved.width(), saved.height()), (600, 300));
        let bytes = std::fs::read(&out).unwrap();
        // JFIF density: units 1 (inches), 300 × 300.
        let jfif = bytes.windows(5).position(|w| w == b"JFIF\0").unwrap();
        assert_eq!(&bytes[jfif + 7..jfif + 12], &[1, 1, 44, 1, 44]);

        let too_many = Collage {
            cells: (0..10).map(|i| cell(1, i as f32 / 10.0)).collect(),
            ..collage
        };
        assert!(render(&db, &too_many).is_err());
    }
}
