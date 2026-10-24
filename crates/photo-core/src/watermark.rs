//! Watermarks for exported copies: a line of text or a logo, drawn in a corner, in
//! the middle, or tiled across the photo. Sizes are relative to the photo's shorter
//! edge, so a mark looks the same on a full-size copy and a small one.
//!
//! Text uses a system font (Segoe UI on Windows, Arial/Helvetica on macOS, DejaVu or
//! Liberation on Linux), white with a soft shadow so it reads on light and dark areas.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::imageops::{self, FilterType};
use image::{DynamicImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Position {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
    Center,
    /// Repeated across the whole photo, which is harder to crop away.
    Tiled,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Watermark {
    /// Drawn when there is no logo.
    pub text: String,
    /// A logo image (a PNG with transparency works best); used instead of the text.
    pub logo: Option<PathBuf>,
    pub position: Position,
    /// Height of the mark as a percentage of the photo's shorter edge, 1–30.
    pub size: u8,
    /// 5–100.
    pub opacity: u8,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark {
            text: String::new(),
            logo: None,
            position: Position::BottomRight,
            size: 5,
            opacity: 70,
        }
    }
}

/// What gets drawn, loaded once per export.
enum Source {
    Text(String, &'static FontVec),
    Logo(RgbaImage),
}

/// A watermark ready to draw on any number of photos.
pub struct Mark {
    source: Source,
    position: Position,
    size: f32,
    opacity: f32,
}

/// Loads the logo or font. `None` when there is nothing to draw (no text, no logo).
pub fn prepare(watermark: &Watermark) -> Result<Option<Mark>> {
    let source = match &watermark.logo {
        Some(path) => Source::Logo(
            image::open(path)
                .map_err(|e| {
                    Error::Invalid(format!("Can't read the logo {}: {e}", path.display()))
                })?
                .to_rgba8(),
        ),
        None => {
            let text = watermark
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if text.is_empty() {
                return Ok(None);
            }
            let font = system_font()
                .ok_or_else(|| Error::Invalid("No font found for the watermark text".into()))?;
            Source::Text(text, font)
        }
    };
    Ok(Some(Mark {
        source,
        position: watermark.position,
        size: watermark.size.clamp(1, 30) as f32 / 100.0,
        opacity: watermark.opacity.clamp(5, 100) as f32 / 100.0,
    }))
}

fn system_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let windows = std::env::var_os("WINDIR").map(|w| Path::new(&w).join("Fonts"));
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &windows {
            for name in ["seguisb.ttf", "segoeui.ttf", "arial.ttf"] {
                candidates.push(dir.join(name));
            }
        }
        for path in [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ] {
            candidates.push(path.into());
        }
        candidates.iter().find_map(|path| {
            let data = std::fs::read(path).ok()?;
            FontVec::try_from_vec_and_index(data, 0).ok()
        })
    })
    .as_ref()
}

/// White text with a soft dark shadow, `height` pixels tall, on a transparent image.
fn render_text(text: &str, font: &FontVec, height: f32) -> RgbaImage {
    let scaled = font.as_scaled(PxScale::from(height));
    let shadow = (height / 20.0).max(1.0).round() as i32;
    // Glyph positions along one line, with kerning.
    let mut glyphs = Vec::new();
    let mut x = 0.0f32;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = previous {
            x += scaled.kern(p, id);
        }
        glyphs.push(id.with_scale_and_position(height, ab_glyph::point(x, scaled.ascent())));
        x += scaled.h_advance(id);
        previous = Some(id);
    }
    let width = x.ceil() as i32 + shadow * 2 + 2;
    let full = (scaled.ascent() - scaled.descent()).ceil() as i32 + shadow * 2 + 2;
    let mut coverage = vec![0f32; (width * full) as usize];
    for glyph in glyphs {
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, c| {
            let px = bounds.min.x as i32 + gx as i32 + 1;
            let py = bounds.min.y as i32 + gy as i32 + 1;
            if (0..width).contains(&px) && (0..full).contains(&py) {
                let cell = &mut coverage[(py * width + px) as usize];
                *cell = (*cell + c).min(1.0);
            }
        });
    }
    let at = |x: i32, y: i32| {
        if (0..width).contains(&x) && (0..full).contains(&y) {
            coverage[(y * width + x) as usize]
        } else {
            0.0
        }
    };
    RgbaImage::from_fn(width as u32, full as u32, |x, y| {
        let (x, y) = (x as i32, y as i32);
        let text = at(x, y);
        let shade = at(x - shadow, y - shadow) * 0.55;
        // Text over shadow: white over black.
        let alpha = text + shade * (1.0 - text);
        let white = if alpha > 0.0 { text / alpha } else { 0.0 };
        Rgba([
            (white * 255.0) as u8,
            (white * 255.0) as u8,
            (white * 255.0) as u8,
            (alpha * 255.0).round() as u8,
        ])
    })
}

impl Mark {
    /// The mark sized for a `width`×`height` photo: `size` of the shorter edge tall,
    /// and never wider than 90% of the photo.
    fn sized_for(&self, width: u32, height: u32) -> RgbaImage {
        let target = (width.min(height) as f32 * self.size).max(4.0);
        let max_width = width as f32 * 0.9;
        match &self.source {
            Source::Text(text, font) => {
                let mark = render_text(text, font, target);
                if mark.width() as f32 <= max_width {
                    mark
                } else {
                    render_text(text, font, target * max_width / mark.width() as f32)
                }
            }
            Source::Logo(logo) => {
                let (lw, lh) = (logo.width().max(1) as f32, logo.height().max(1) as f32);
                let scale = (target / lh).min(max_width / lw);
                let (w, h) = (
                    ((lw * scale).round() as u32).max(1),
                    ((lh * scale).round() as u32).max(1),
                );
                imageops::resize(logo, w, h, FilterType::Lanczos3)
            }
        }
    }

    /// Top-left corners where the mark goes on a `width`×`height` photo.
    fn spots(&self, width: u32, height: u32, mark: &RgbaImage) -> Vec<(i64, i64)> {
        let (w, h) = (width as i64, height as i64);
        let (mw, mh) = (mark.width() as i64, mark.height() as i64);
        let margin = (w.min(h) as f32 * 0.03).round() as i64;
        let (left, top) = (margin, margin);
        let (right, bottom) = (w - mw - margin, h - mh - margin);
        match self.position {
            Position::TopLeft => vec![(left, top)],
            Position::TopRight => vec![(right, top)],
            Position::BottomLeft => vec![(left, bottom)],
            Position::BottomRight => vec![(right, bottom)],
            Position::Center => vec![((w - mw) / 2, (h - mh) / 2)],
            Position::Tiled => {
                let (step_x, step_y) = (mw * 2, mh * 4);
                let mut spots = Vec::new();
                for (row, y) in (0..h).step_by(step_y.max(1) as usize).enumerate() {
                    let shift = if row % 2 == 1 { -mw } else { 0 };
                    let mut x = shift + mw / 2;
                    while x < w {
                        spots.push((x, y + mh));
                        x += step_x.max(1);
                    }
                }
                spots
            }
        }
    }

    /// Draws the mark on `image`.
    pub fn apply(&self, image: &mut DynamicImage) {
        let (width, height) = (image.width(), image.height());
        let mark = self.sized_for(width, height);
        let spots = self.spots(width, height, &mark);
        if image.color().has_alpha() {
            let mut rgba = image.to_rgba8();
            for &(x, y) in &spots {
                blend(&mut rgba, &mark, x, y, self.opacity);
            }
            *image = DynamicImage::ImageRgba8(rgba);
        } else {
            let mut rgb = match std::mem::take(image) {
                DynamicImage::ImageRgb8(rgb) => rgb,
                other => other.to_rgb8(),
            };
            for &(x, y) in &spots {
                blend_rgb(&mut rgb, &mark, x, y, self.opacity);
            }
            *image = DynamicImage::ImageRgb8(rgb);
        }
    }
}

/// `mark` over an RGBA photo at (`x`, `y`), clipped to the photo.
fn blend(base: &mut RgbaImage, mark: &RgbaImage, x: i64, y: i64, opacity: f32) {
    for (mx, my, p) in mark.enumerate_pixels() {
        let (bx, by) = (x + mx as i64, y + my as i64);
        if bx < 0 || by < 0 || bx >= base.width() as i64 || by >= base.height() as i64 {
            continue;
        }
        let a = p.0[3] as f32 / 255.0 * opacity;
        if a <= 0.0 {
            continue;
        }
        let q = base.get_pixel_mut(bx as u32, by as u32);
        let ba = q.0[3] as f32 / 255.0;
        let out = a + ba * (1.0 - a);
        for i in 0..3 {
            let c = (p.0[i] as f32 * a + q.0[i] as f32 * ba * (1.0 - a)) / out;
            q.0[i] = c.round() as u8;
        }
        q.0[3] = (out * 255.0).round() as u8;
    }
}

/// `mark` over an opaque photo at (`x`, `y`), clipped to the photo.
fn blend_rgb(base: &mut image::RgbImage, mark: &RgbaImage, x: i64, y: i64, opacity: f32) {
    for (mx, my, p) in mark.enumerate_pixels() {
        let (bx, by) = (x + mx as i64, y + my as i64);
        if bx < 0 || by < 0 || bx >= base.width() as i64 || by >= base.height() as i64 {
            continue;
        }
        let a = p.0[3] as f32 / 255.0 * opacity;
        if a <= 0.0 {
            continue;
        }
        let q = base.get_pixel_mut(bx as u32, by as u32);
        for i in 0..3 {
            q.0[i] = (p.0[i] as f32 * a + q.0[i] as f32 * (1.0 - a)).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn grey(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([60, 60, 60])))
    }

    /// Pixels brighter than the grey background, as (min x, min y, max x, max y).
    fn lit_box(image: &DynamicImage) -> Option<(u32, u32, u32, u32)> {
        let rgb = image.to_rgb8();
        let mut found: Option<(u32, u32, u32, u32)> = None;
        for (x, y, p) in rgb.enumerate_pixels() {
            if p.0[0] > 120 {
                found = Some(match found {
                    None => (x, y, x, y),
                    Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                });
            }
        }
        found
    }

    #[test]
    fn text_lands_in_the_chosen_corner() {
        if system_font().is_none() {
            return; // No font on this machine; nothing to draw with.
        }
        let mark = prepare(&Watermark {
            text: "© Studio 2026".into(),
            size: 8,
            opacity: 100,
            ..Default::default()
        })
        .unwrap()
        .unwrap();
        let mut image = grey(1200, 800);
        mark.apply(&mut image);
        let (x0, y0, x1, y1) = lit_box(&image).expect("text drawn");
        assert!(x0 > 600 && y0 > 600, "bottom right: {x0},{y0}");
        assert!(
            x1 < 1200 - 20 && y1 < 800 - 20,
            "inside the margin: {x1},{y1}"
        );
        assert!(y1 - y0 < 80, "about 8% of 800 tall: {}", y1 - y0);
    }

    #[test]
    fn logo_keeps_its_shape_and_opacity_softens_it() {
        let dir = tempfile::tempdir().unwrap();
        let logo = dir.path().join("logo.png");
        RgbaImage::from_pixel(200, 100, Rgba([255, 255, 255, 255]))
            .save(&logo)
            .unwrap();
        let draw = |opacity| {
            let mark = prepare(&Watermark {
                logo: Some(logo.clone()),
                position: Position::TopLeft,
                size: 10,
                opacity,
                ..Default::default()
            })
            .unwrap()
            .unwrap();
            let mut image = grey(1000, 1000);
            mark.apply(&mut image);
            image.to_rgb8()
        };
        let full = draw(100);
        // 10% of 1000 tall, twice as wide, 3% margin.
        assert_eq!(full.get_pixel(30, 30).0[0], 255);
        assert_eq!(full.get_pixel(229, 129).0[0], 255);
        assert_eq!(full.get_pixel(231, 60).0[0], 60);
        let half = draw(50);
        let v = half.get_pixel(100, 60).0[0];
        assert!((150..=165).contains(&v), "half way between 60 and 255: {v}");
    }

    #[test]
    fn nothing_to_draw_without_text_or_logo() {
        assert!(prepare(&Watermark::default()).unwrap().is_none());
        assert!(
            prepare(&Watermark {
                text: "  \n ".into(),
                ..Default::default()
            })
            .unwrap()
            .is_none()
        );
    }
}
