//! Upscale: enlarges photos with Real-ESRGAN (its general model), which adds believable
//! detail where plain resizing only blurs. The model takes 128-pixel tiles and gives
//! them back 4× bigger; tiles overlap so their edges, where the model sees least,
//! are cut away.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{Rgb, RgbImage};
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use photo_core::{Error, Result};

use crate::models::{Pack, Store};
use crate::runtime::{Runtime, ai_error};

const MODEL: &str = "real_esrgan_general_x4v3.onnx";
/// The model's tile, and how much bigger it makes it.
const TILE: u32 = 128;
pub const FACTOR: u32 = 4;
/// Pixels around each tile's kept part, seen by the model but cut from its result.
const MARGIN: u32 = 12;
/// Photos with a longer edge than this aren't enlarged: they're big already, and it
/// would take minutes.
pub const MAX_SOURCE: u32 = 2048;

pub struct Upscaler {
    session: Mutex<Session>,
}

impl Upscaler {
    /// Loads the model (checking its files first).
    pub fn load(store: &Store, threads: usize) -> Result<Upscaler> {
        store.verify(Pack::Upscale)?;
        let runtime = Runtime::get(store)?;
        Ok(Upscaler {
            session: Mutex::new(runtime.session(&store.path(Pack::Upscale, MODEL), threads)?),
        })
    }

    /// `image` 4× bigger. Calls `progress(tiles done, tiles)`; stops when `cancel` is
    /// set.
    pub fn enlarge(
        &self,
        image: &RgbImage,
        cancel: &AtomicBool,
        progress: &dyn Fn(usize, usize),
    ) -> Result<RgbImage> {
        let (w, h) = image.dimensions();
        if w == 0 || h == 0 {
            return Err(Error::Invalid("The photo is empty".into()));
        }
        let step = TILE - 2 * MARGIN;
        let (cols, rows) = (w.div_ceil(step), h.div_ceil(step));
        let total = (cols * rows) as usize;
        let mut out = RgbImage::new(w * FACTOR, h * FACTOR);
        let side = TILE as usize;
        let plane = side * side;
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        for row in 0..rows {
            for col in 0..cols {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Invalid("Stopped".into()));
                }
                // The kept part starts here; the tile reaches MARGIN further each way,
                // with the photo's edge pixels repeated past its border.
                let (kx, ky) = (col * step, row * step);
                let (x0, y0) = (kx as i64 - MARGIN as i64, ky as i64 - MARGIN as i64);
                let mut pixels = vec![0f32; 3 * plane];
                for ty in 0..side {
                    let sy = (y0 + ty as i64).clamp(0, h as i64 - 1) as u32;
                    for tx in 0..side {
                        let sx = (x0 + tx as i64).clamp(0, w as i64 - 1) as u32;
                        let p = image.get_pixel(sx, sy).0;
                        let i = ty * side + tx;
                        for c in 0..3 {
                            pixels[c * plane + i] = p[c] as f32 / 255.0;
                        }
                    }
                }
                let input = Array4::from_shape_vec((1, 3, side, side), pixels).map_err(ai_error)?;
                let outputs = session
                    .run(ort::inputs!["image" => TensorRef::from_array_view(&input).map_err(ai_error)?])
                    .map_err(ai_error)?;
                let result = outputs["upscaled_image"]
                    .try_extract_array::<f32>()
                    .map_err(ai_error)?;
                let big = side * FACTOR as usize;
                let values = result.as_slice().ok_or_else(|| {
                    Error::Invalid("The upscale model gave an unexpected result".into())
                })?;
                if values.len() != 3 * big * big {
                    return Err(Error::Invalid(format!(
                        "The upscale model gave {} values",
                        values.len()
                    )));
                }
                let big_plane = big * big;
                let m = (MARGIN * FACTOR) as usize;
                let keep_w = (step.min(w - kx) * FACTOR) as usize;
                let keep_h = (step.min(h - ky) * FACTOR) as usize;
                for y in 0..keep_h {
                    for x in 0..keep_w {
                        let i = (y + m) * big + (x + m);
                        let px = |c: usize| {
                            (values[c * big_plane + i] * 255.0)
                                .round()
                                .clamp(0.0, 255.0) as u8
                        };
                        out.put_pixel(
                            kx * FACTOR + x as u32,
                            ky * FACTOR + y as u32,
                            Rgb([px(0), px(1), px(2)]),
                        );
                    }
                }
                progress((row * cols + col + 1) as usize, total);
            }
        }
        Ok(out)
    }
}

impl Upscaler {
    /// `image` enlarged by `factor` (2 to 4) to exactly that size; None when it's
    /// longer than `MAX_SOURCE` already.
    pub fn enlarge_by(
        &self,
        image: &RgbImage,
        factor: u32,
        cancel: &AtomicBool,
    ) -> Result<Option<RgbImage>> {
        let (w, h) = image.dimensions();
        if w.max(h) > MAX_SOURCE || !(2..=FACTOR).contains(&factor) {
            return Ok(None);
        }
        let big = self.enlarge(image, cancel, &|_, _| {})?;
        if factor == FACTOR {
            return Ok(Some(big));
        }
        Ok(Some(image::imageops::resize(
            &big,
            w * factor,
            h * factor,
            image::imageops::FilterType::Lanczos3,
        )))
    }
}

/// How many tiles enlarging a `w`×`h` photo takes (each about a fifth of a second).
pub fn tiles(w: u32, h: u32) -> usize {
    let step = TILE - 2 * MARGIN;
    (w.div_ceil(step) * h.div_ceil(step)) as usize
}
