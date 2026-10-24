//! Non-destructive edits. A photo's edit is a [`Recipe`] kept in `user_meta.edit`
//! (JSON); the file is never rewritten. The recipe is applied after EXIF orientation
//! and the user's turns, wherever pixels are produced: the editor preview, thumbnails,
//! the viewer and export. `user_meta.edit_v` counts saves, so image URLs change with
//! every edit and the webview never shows a cached look.
//!
//! Order: straighten, crop, light and colour, sharpen. Sliders run from -100 to 100
//! (exposure in stops), with 0 meaning "as shot".

use image::imageops;
use image::{DynamicImage, Rgb, RgbImage};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A crop in the straightened photo, as fractions of its width and height.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Recipe {
    pub crop: Option<Crop>,
    /// Degrees, -45 to 45, clockwise. The photo is zoomed so no corners show.
    pub straighten: f32,
    /// Stops, -3 to 3.
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    /// Warmer (positive) or cooler.
    pub temperature: f32,
    /// Magenta (positive) or green.
    pub tint: f32,
    /// Saturation that spares already-colourful areas and skin.
    pub vibrance: f32,
    pub saturation: f32,
    /// 0 to 100.
    pub sharpen: f32,
}

impl Recipe {
    pub fn is_identity(&self) -> bool {
        *self == Recipe::default()
    }

    fn has_geometry(&self) -> bool {
        self.crop.is_some() || self.straighten.abs() > 0.01
    }

    fn has_tone(&self) -> bool {
        Recipe {
            crop: None,
            straighten: 0.0,
            sharpen: 0.0,
            ..self.clone()
        } != Recipe::default()
    }

    /// The same edit without its crop, for showing the whole photo while cropping.
    pub fn without_crop(&self) -> Recipe {
        Recipe {
            crop: None,
            ..self.clone()
        }
    }

    /// Values pulled into their ranges, and a crop that stays inside the photo.
    fn clamped(&self) -> Recipe {
        let s = |v: f32| {
            if v.is_finite() {
                v.clamp(-100.0, 100.0)
            } else {
                0.0
            }
        };
        let crop = self.crop.and_then(|c| {
            let x = c.x.clamp(0.0, 1.0);
            let y = c.y.clamp(0.0, 1.0);
            let w = c.w.clamp(0.0, 1.0 - x);
            let h = c.h.clamp(0.0, 1.0 - y);
            // A crop of (nearly) everything is no crop.
            (w > 0.001 && h > 0.001 && (w < 0.999 || h < 0.999)).then_some(Crop { x, y, w, h })
        });
        Recipe {
            crop,
            straighten: if self.straighten.is_finite() {
                self.straighten.clamp(-45.0, 45.0)
            } else {
                0.0
            },
            exposure: if self.exposure.is_finite() {
                self.exposure.clamp(-3.0, 3.0)
            } else {
                0.0
            },
            contrast: s(self.contrast),
            highlights: s(self.highlights),
            shadows: s(self.shadows),
            temperature: s(self.temperature),
            tint: s(self.tint),
            vibrance: s(self.vibrance),
            saturation: s(self.saturation),
            sharpen: s(self.sharpen).max(0.0),
        }
    }
}

/// Applies `recipe` to an upright photo.
pub fn render(image: DynamicImage, recipe: &Recipe) -> DynamicImage {
    let recipe = recipe.clamped();
    if recipe.is_identity() {
        return image;
    }
    let mut rgb = match image {
        DynamicImage::ImageRgb8(rgb) => rgb,
        other => other.to_rgb8(),
    };
    if recipe.has_geometry() {
        rgb = geometry(&rgb, recipe.straighten, recipe.crop);
    }
    if recipe.has_tone() {
        tone(&mut rgb, &recipe);
    }
    if recipe.sharpen > 0.0 {
        rgb = sharpen(&rgb, recipe.sharpen / 100.0);
    }
    DynamicImage::ImageRgb8(rgb)
}

/// Scale that fits a `w`×`h` frame, turned by `angle` radians, inside the original:
/// straightening zooms in just enough that no empty corners show.
fn straighten_scale(w: f32, h: f32, angle: f32) -> f32 {
    let (sin, cos) = (angle.abs().sin(), angle.abs().cos());
    (w / (w * cos + h * sin)).min(h / (w * sin + h * cos))
}

/// Straightens and crops in one resampling pass (bilinear). Without straightening,
/// the crop is a plain copy of pixels.
fn geometry(src: &RgbImage, degrees: f32, crop: Option<Crop>) -> RgbImage {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let crop = crop.unwrap_or(Crop {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    });
    let angle = degrees.to_radians();
    if angle.abs() < 1e-4 {
        let (x, y) = ((crop.x * w) as u32, (crop.y * h) as u32);
        let cw = ((crop.w * w).round() as u32).clamp(1, src.width() - x);
        let ch = ((crop.h * h).round() as u32).clamp(1, src.height() - y);
        return imageops::crop_imm(src, x, y, cw, ch).to_image();
    }
    // The straightened frame has the photo's own size, scaled down to fit.
    let scale = straighten_scale(w, h, angle);
    let (fw, fh) = (w * scale, h * scale);
    let out_w = ((crop.w * fw).round() as u32).max(1);
    let out_h = ((crop.h * fh).round() as u32).max(1);
    // Output pixel → position in the frame (centred) → turned back into the source.
    let (ox, oy) = (crop.x * fw - fw / 2.0, crop.y * fh - fh / 2.0);
    let (sin, cos) = angle.sin_cos();
    let (cx, cy) = (w / 2.0, h / 2.0);
    let mut out = RgbImage::new(out_w, out_h);
    out.par_chunks_mut(out_w as usize * 3)
        .enumerate()
        .for_each(|(y, row)| {
            let fy = oy + y as f32 + 0.5;
            for x in 0..out_w as usize {
                let fx = ox + x as f32 + 0.5;
                // Clockwise straightening shows the source turned counter-clockwise.
                let sx = cx + fx * cos + fy * sin - 0.5;
                let sy = cy - fx * sin + fy * cos - 0.5;
                let p = bilinear(src, sx, sy);
                row[x * 3..x * 3 + 3].copy_from_slice(&p);
            }
        });
    out
}

/// Rounds to the nearest byte, saturating. `f32::round` is a library call on the
/// baseline x86-64 target, far too slow per pixel; `as` saturates (and maps NaN to 0).
#[inline]
fn to_u8(v: f32) -> u8 {
    (v + 0.5) as u8
}

fn bilinear(src: &RgbImage, x: f32, y: f32) -> [u8; 3] {
    let max_x = src.width() as f32 - 1.0;
    let max_y = src.height() as f32 - 1.0;
    let (x, y) = (x.clamp(0.0, max_x), y.clamp(0.0, max_y));
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (x0, y0) = (x0 as u32, y0 as u32);
    let (x1, y1) = ((x0 + 1).min(max_x as u32), (y0 + 1).min(max_y as u32));
    let (a, b) = (src.get_pixel(x0, y0).0, src.get_pixel(x1, y0).0);
    let (c, d) = (src.get_pixel(x0, y1).0, src.get_pixel(x1, y1).0);
    let mut out = [0u8; 3];
    for i in 0..3 {
        let top = a[i] as f32 * (1.0 - tx) + b[i] as f32 * tx;
        let bottom = c[i] as f32 * (1.0 - tx) + d[i] as f32 * tx;
        out[i] = to_u8(top * (1.0 - ty) + bottom * ty);
    }
    out
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Exposure (in linear light), then contrast, highlights/shadows, white balance and
/// saturation on display values.
fn tone(image: &mut RgbImage, r: &Recipe) {
    // Exposure and white balance per channel through lookup tables.
    let gain = 2f32.powf(r.exposure);
    let warm = r.temperature / 100.0;
    let tint = r.tint / 100.0;
    let channel_gain = [
        gain * (1.0 + 0.12 * warm) * (1.0 + 0.04 * tint),
        gain * (1.0 - 0.08 * tint),
        gain * (1.0 - 0.12 * warm) * (1.0 + 0.04 * tint),
    ];
    let luts: Vec<[f32; 256]> = channel_gain
        .iter()
        .map(|&g| {
            let mut lut = [0f32; 256];
            for (i, v) in lut.iter_mut().enumerate() {
                *v = to_srgb((to_linear(i as f32 / 255.0) * g).min(1.0));
            }
            lut
        })
        .collect();
    let contrast = r.contrast / 100.0;
    let highlights = r.highlights / 100.0;
    let shadows = r.shadows / 100.0;
    let saturation = 1.0 + r.saturation / 100.0;
    let vibrance = r.vibrance / 100.0;

    let row_len = image.width() as usize * 3;
    image.par_chunks_mut(row_len).for_each(|row| {
        for px in row.as_chunks_mut::<3>().0 {
            let mut c = [
                luts[0][px[0] as usize],
                luts[1][px[1] as usize],
                luts[2][px[2] as usize],
            ];
            if contrast != 0.0 {
                // An S-curve around mid-grey: steeper (or flatter) in the middle, so
                // blacks and whites ease in rather than clip.
                let k = 1.0 + contrast * if contrast > 0.0 { 1.2 } else { 0.7 };
                for v in &mut c {
                    let x = *v - 0.5;
                    *v = 0.5 + x * k / (1.0 + (k - 1.0) * 2.0 * x.abs()).max(0.2);
                }
            }
            let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            if highlights != 0.0 || shadows != 0.0 {
                let in_shadow = (1.0 - y / 0.55).clamp(0.0, 1.0).powi(2);
                let in_light = ((y - 0.45) / 0.55).clamp(0.0, 1.0).powi(2);
                let target =
                    (y + 0.45 * shadows * in_shadow + 0.35 * highlights * in_light).clamp(0.0, 1.0);
                let k = if y > 0.002 { target / y } else { 1.0 };
                for v in &mut c {
                    *v *= k;
                }
            }
            if saturation != 1.0 || vibrance != 0.0 {
                let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let max = c[0].max(c[1]).max(c[2]);
                let min = c[0].min(c[1]).min(c[2]);
                let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
                let k = saturation * (1.0 + vibrance * (1.0 - sat));
                for v in &mut c {
                    *v = y + (*v - y) * k.max(0.0);
                }
            }
            for (out, v) in px.iter_mut().zip(c) {
                *out = to_u8(v * 255.0);
            }
        }
    });
}

/// Unsharp mask, sized to the photo so a preview and the full image look alike.
fn sharpen(image: &RgbImage, amount: f32) -> RgbImage {
    let long = image.width().max(image.height()) as f32;
    let sigma = (long / 2000.0).clamp(0.6, 3.0);
    let blurred = gaussian_blur(image, sigma);
    let mut out = image.clone();
    out.par_chunks_mut(3)
        .zip(blurred.par_chunks(3))
        .for_each(|(px, soft)| {
            for i in 0..3 {
                let v = px[i] as f32 + (px[i] as f32 - soft[i] as f32) * amount * 1.5;
                px[i] = to_u8(v);
            }
        });
    out
}

/// A Gaussian blur as two passes (rows, then columns), each row in parallel.
fn gaussian_blur(image: &RgbImage, sigma: f32) -> RgbImage {
    let radius = (sigma * 2.5).ceil() as i64;
    let mut kernel: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    let (w, h) = (image.width() as usize, image.height() as usize);
    let src = image.as_raw();

    let r = radius as usize;
    let len = w * 3;

    // Across each row, from a copy padded with its edge pixels.
    let mut across = vec![0f32; len * h];
    across
        .par_chunks_mut(len)
        .zip(src.par_chunks(len))
        .for_each(|(out, row)| {
            let mut padded = Vec::with_capacity(len + 6 * r);
            for i in 0..w + 2 * r {
                let x = i.saturating_sub(r).min(w - 1);
                padded.extend(row[x * 3..x * 3 + 3].iter().map(|&v| v as f32));
            }
            for (k, &weight) in kernel.iter().enumerate() {
                let tap = &padded[k * 3..k * 3 + len];
                for (o, &v) in out.iter_mut().zip(tap) {
                    *o += v * weight;
                }
            }
        });

    // Down each column, a whole row at a time so memory is read in order.
    let mut out = RgbImage::new(w as u32, h as u32);
    out.par_chunks_mut(len).enumerate().for_each(|(y, row)| {
        let mut acc = vec![0f32; len];
        for (k, &weight) in kernel.iter().enumerate() {
            let sy = (y as i64 + k as i64 - radius).clamp(0, h as i64 - 1) as usize;
            for (a, &v) in acc.iter_mut().zip(&across[sy * len..(sy + 1) * len]) {
                *a += v * weight;
            }
        }
        for (o, a) in row.iter_mut().zip(acc) {
            *o = to_u8(a);
        }
    });
    out
}

/// `base` with `recipe` applied, as a JPEG for the editor.
pub fn preview_jpeg(base: &DynamicImage, recipe: &Recipe) -> Result<Vec<u8>> {
    let image = render(base.clone(), recipe).to_rgb8();
    crate::jpeg::encode(&image, 85, crate::jpeg::Purpose::Show)
}

/// Light and colour settings that make a typical photo look its best: midtones
/// brought to middle grey, a little more contrast when it looks flat, shadows and
/// highlights opened up when they're crowded, and a touch of vibrance. Crop and
/// straightening are kept from `current`.
pub fn auto_enhance(image: &DynamicImage, current: &Recipe) -> Recipe {
    let small = image.thumbnail(512, 512).to_rgb8();
    let mut histogram = [0u32; 256];
    for p in small.pixels() {
        let Rgb([r, g, b]) = *p;
        let y = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round() as usize;
        histogram[y.min(255)] += 1;
    }
    let total: u32 = histogram.iter().sum();
    let percentile = |p: f32| {
        let target = (total as f32 * p) as u32;
        let mut seen = 0;
        for (i, n) in histogram.iter().enumerate() {
            seen += n;
            if seen > target {
                return i as f32 / 255.0;
            }
        }
        1.0
    };
    let (low, median, high) = (percentile(0.01), percentile(0.5), percentile(0.99));
    let exposure = (to_linear(0.46) / to_linear(median.max(0.02))).log2() * 0.6;
    let spread = (high - low).max(0.05);
    let contrast = if spread < 0.85 {
        ((0.9 / spread - 1.0) * 60.0).min(35.0)
    } else {
        0.0
    };
    Recipe {
        crop: current.crop,
        straighten: current.straighten,
        sharpen: current.sharpen,
        exposure: (exposure.clamp(-1.2, 1.2) * 10.0).round() / 10.0,
        contrast: contrast.round(),
        shadows: if percentile(0.1) < 0.12 { 25.0 } else { 0.0 },
        highlights: if high > 0.97 { -30.0 } else { 0.0 },
        vibrance: 15.0,
        ..Recipe::default()
    }
}

/// How a photo is shown on top of its file: the user's turns, then their edit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub turns: u8,
    pub edit: Option<Recipe>,
}

impl Look {
    /// Shown exactly as the file is.
    pub fn is_plain(&self) -> bool {
        self.turns == 0 && self.edit.is_none()
    }

    /// Applies the look to a photo already upright by its EXIF orientation.
    pub fn apply(&self, image: DynamicImage) -> DynamicImage {
        let image = crate::rotate::turn(image, self.turns);
        match &self.edit {
            Some(recipe) => render(image, recipe),
            None => image,
        }
    }

    /// How much a crop zooms in, so a small result can be rendered from a decode big
    /// enough to stay sharp.
    pub fn zoom(&self) -> f32 {
        self.edit
            .as_ref()
            .and_then(|r| r.crop)
            .map_or(1.0, |c| 1.0 / c.w.min(c.h).max(0.05))
    }
}

/// Parses a stored recipe; one that doesn't parse (a newer version) is ignored.
pub fn parse(json: Option<String>) -> Option<Recipe> {
    json.and_then(|j| serde_json::from_str(&j).ok())
}

/// The turns and edit of `asset_id`.
pub fn look_of(conn: &Connection, asset_id: i64) -> Result<Look> {
    let row: Option<(u8, Option<String>)> = conn
        .prepare_cached("SELECT turns, edit FROM user_meta WHERE asset_id = ?1")?
        .query_row(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.map_or_else(Look::default, |(turns, edit)| Look {
        turns,
        edit: parse(edit),
    }))
}

/// After a photo is turned `quarters` times clockwise, turns its crop with it so the
/// same part of the picture stays selected.
pub fn turn_crop(conn: &Connection, asset_id: i64, quarters: u8) -> Result<()> {
    let Some(mut recipe) = get(conn, asset_id)? else {
        return Ok(());
    };
    let Some(mut c) = recipe.crop else {
        return Ok(());
    };
    for _ in 0..quarters % 4 {
        // A quarter turn clockwise: the left edge becomes the top.
        c = Crop {
            x: 1.0 - (c.y + c.h),
            y: c.x,
            w: c.h,
            h: c.w,
        };
    }
    recipe.crop = Some(c);
    conn.prepare_cached("UPDATE user_meta SET edit = ?2 WHERE asset_id = ?1")?
        .execute(params![
            asset_id,
            serde_json::to_string(&recipe).expect("plain data")
        ])?;
    Ok(())
}

/// The saved edit of `asset_id`, if it has one.
pub fn get(conn: &Connection, asset_id: i64) -> Result<Option<Recipe>> {
    let json: Option<String> = conn
        .prepare_cached("SELECT edit FROM user_meta WHERE asset_id = ?1")?
        .query_row(params![asset_id], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

/// Saves (or with `None` or a do-nothing recipe, removes) a photo's edit. Returns the
/// new edit version.
pub fn save(conn: &Connection, asset_id: i64, recipe: Option<&Recipe>) -> Result<i64> {
    let kind: Option<u8> = conn
        .prepare_cached("SELECT media_kind FROM assets WHERE id = ?1")?
        .query_row(params![asset_id], |r| r.get(0))
        .optional()?;
    match kind {
        None => return Err(Error::Invalid("No such photo".into())),
        Some(1) => return Err(Error::Invalid("Videos can't be edited".into())),
        Some(_) => {}
    }
    let json = recipe
        .map(Recipe::clamped)
        .filter(|r| !r.is_identity())
        .map(|r| serde_json::to_string(&r).expect("plain data"));
    Ok(conn
        .prepare_cached(
            "INSERT INTO user_meta (asset_id, edit, edit_v) VALUES (?1, ?2, 1)
             ON CONFLICT (asset_id) DO UPDATE SET edit = excluded.edit, edit_v = edit_v + 1
             RETURNING edit_v",
        )?
        .query_row(params![asset_id, json], |r| r.get(0))?)
}

/// How much of a copied edit to paste.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PasteWhat {
    Everything,
    /// Light, colour and sharpening; each photo keeps its own crop and straightening,
    /// which rarely suit another photo.
    Tone,
}

/// Pastes `recipe` onto every photo in `ids` in one transaction, replacing their
/// edits; videos and unknown ids are skipped. Each photo can still be reverted on its
/// own. Returns (id, new edit version) for each photo pasted onto.
pub fn paste(
    conn: &mut Connection,
    ids: &[i64],
    recipe: &Recipe,
    what: PasteWhat,
) -> Result<Vec<(i64, i64)>> {
    let tx = conn.transaction()?;
    let mut pasted = Vec::with_capacity(ids.len());
    for &id in ids {
        let kind: Option<u8> = tx
            .prepare_cached("SELECT media_kind FROM assets WHERE id = ?1")?
            .query_row(params![id], |r| r.get(0))
            .optional()?;
        if !matches!(kind, Some(k) if k != 1) {
            continue;
        }
        let next = match what {
            PasteWhat::Everything => recipe.clone(),
            PasteWhat::Tone => {
                let own = get(&tx, id)?.unwrap_or_default();
                Recipe {
                    crop: own.crop,
                    straighten: own.straighten,
                    ..recipe.clone()
                }
            }
        };
        pasted.push((id, save(&tx, id, Some(&next))?));
    }
    tx.commit()?;
    Ok(pasted)
}

/// Every photo that looks different from its file (turned or edited, or edited once
/// and reverted), with its turns and edit version: the frontend puts these in image
/// URLs.
pub fn adjusted(conn: &Connection) -> Result<Vec<(i64, u8, i64)>> {
    let mut stmt = conn
        .prepare("SELECT asset_id, turns, edit_v FROM user_meta WHERE turns != 0 OR edit_v != 0")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 128])
        }))
    }

    fn mean(image: &DynamicImage) -> f32 {
        let rgb = image.to_rgb8();
        rgb.pixels()
            .map(|p| p.0.iter().map(|&v| v as f32).sum::<f32>())
            .sum::<f32>()
            / (rgb.width() * rgb.height() * 3) as f32
    }

    #[test]
    fn nothing_changes_without_edits() {
        let image = gradient(64, 48);
        assert_eq!(render(image.clone(), &Recipe::default()), image);
    }

    #[test]
    fn crop_and_straighten_set_the_size() {
        let cropped = render(
            gradient(400, 300),
            &Recipe {
                crop: Some(Crop {
                    x: 0.25,
                    y: 0.5,
                    w: 0.5,
                    h: 0.5,
                }),
                ..Default::default()
            },
        );
        assert_eq!((cropped.width(), cropped.height()), (200, 150));
        // Left edge of the crop is a quarter of the way across the gradient.
        assert!((cropped.to_rgb8().get_pixel(0, 0).0[0] as i32 - 64).abs() <= 1);

        let straight = render(
            gradient(400, 300),
            &Recipe {
                straighten: 10.0,
                ..Default::default()
            },
        );
        let s = straighten_scale(400.0, 300.0, 10f32.to_radians());
        assert_eq!(straight.width(), (400.0 * s).round() as u32);
        assert!(straight.width() < 400 && straight.height() < 300);
    }

    #[test]
    fn exposure_and_contrast_move_tones_the_right_way() {
        let base = gradient(100, 100);
        let brighter = render(
            base.clone(),
            &Recipe {
                exposure: 1.0,
                ..Default::default()
            },
        );
        assert!(mean(&brighter) > mean(&base) + 20.0);

        let punchy = render(
            base.clone(),
            &Recipe {
                contrast: 60.0,
                ..Default::default()
            },
        )
        .to_rgb8();
        let original = base.to_rgb8();
        // Darks darker, lights lighter.
        assert!(punchy.get_pixel(10, 50).0[0] < original.get_pixel(10, 50).0[0]);
        assert!(punchy.get_pixel(90, 50).0[0] > original.get_pixel(90, 50).0[0]);

        let grey = render(
            base,
            &Recipe {
                saturation: -100.0,
                ..Default::default()
            },
        )
        .to_rgb8();
        let p = grey.get_pixel(80, 20).0;
        assert!(
            p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1,
            "{p:?}"
        );
    }

    #[test]
    fn auto_enhance_brightens_a_dark_photo() {
        let dark = DynamicImage::ImageRgb8(RgbImage::from_fn(200, 200, |x, _| {
            Rgb([(x / 5) as u8 + 10, 20, 30])
        }));
        let recipe = auto_enhance(&dark, &Recipe::default());
        assert!(recipe.exposure > 0.5, "{recipe:?}");
        assert!(mean(&render(dark.clone(), &recipe)) > mean(&dark) * 1.5);
    }

    #[test]
    fn a_crop_turns_with_the_photo() {
        let image = gradient(400, 300);
        let crop = Crop {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.5,
        };
        let cropped = |img: DynamicImage, crop| {
            render(
                img,
                &Recipe {
                    crop: Some(crop),
                    ..Default::default()
                },
            )
        };
        let turned_after = crate::rotate::turn(cropped(image.clone(), crop), 1);

        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
             VALUES (1, 0, 0, 0, 0, 0)",
        )
        .unwrap();
        save(
            &conn,
            1,
            Some(&Recipe {
                crop: Some(crop),
                ..Default::default()
            }),
        )
        .unwrap();
        turn_crop(&conn, 1, 1).unwrap();
        let moved = get(&conn, 1).unwrap().unwrap().crop.unwrap();
        let turned_first = cropped(crate::rotate::turn(image, 1), moved);
        assert_eq!(
            (turned_first.width(), turned_first.height()),
            (turned_after.width(), turned_after.height())
        );
        assert_eq!(turned_first.to_rgb8(), turned_after.to_rgb8());
    }

    #[test]
    fn saving_counts_versions_and_reverting_clears() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
             VALUES (1, 0, 0, 0, 0, 0), (2, 1, 0, 0, 0, 0)",
        )
        .unwrap();
        let warm = Recipe {
            temperature: 30.0,
            ..Default::default()
        };
        assert_eq!(save(&conn, 1, Some(&warm)).unwrap(), 1);
        assert_eq!(get(&conn, 1).unwrap(), Some(warm));
        assert_eq!(save(&conn, 1, None).unwrap(), 2);
        assert_eq!(get(&conn, 1).unwrap(), None);
        assert_eq!(adjusted(&conn).unwrap(), vec![(1, 0, 2)]);
        assert!(save(&conn, 2, Some(&Recipe::default())).is_err(), "videos");
    }

    #[test]
    fn pasting_skips_videos_and_can_keep_each_crop() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at)
             VALUES (1, 0, 0, 0, 0, 0), (2, 0, 0, 0, 0, 0), (3, 1, 0, 0, 0, 0)",
        )
        .unwrap();
        let own_crop = Crop {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        };
        let cropped = Recipe {
            crop: Some(own_crop),
            straighten: 3.0,
            exposure: -1.0,
            ..Default::default()
        };
        save(&conn, 2, Some(&cropped)).unwrap();
        let copied = Recipe {
            crop: Some(Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            }),
            straighten: -5.0,
            contrast: 40.0,
            vibrance: 20.0,
            ..Default::default()
        };

        let pasted = paste(&mut conn, &[1, 2, 3, 99], &copied, PasteWhat::Tone).unwrap();
        assert_eq!(pasted, vec![(1, 1), (2, 2)]);
        let first = get(&conn, 1).unwrap().unwrap();
        assert_eq!((first.crop, first.straighten), (None, 0.0));
        assert_eq!(first.contrast, 40.0);
        let second = get(&conn, 2).unwrap().unwrap();
        assert_eq!(second.crop, Some(own_crop));
        assert_eq!(second.straighten, 3.0);
        assert_eq!(
            (second.exposure, second.vibrance),
            (0.0, 20.0),
            "tone replaced"
        );
        assert_eq!(get(&conn, 3).unwrap(), None, "videos are skipped");

        paste(&mut conn, &[2], &copied, PasteWhat::Everything).unwrap();
        assert_eq!(get(&conn, 2).unwrap(), Some(copied));
    }
}
