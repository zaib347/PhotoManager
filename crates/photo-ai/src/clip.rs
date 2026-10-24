//! CLIP: images and text in one embedding space, so "beach at sunset" finds photos of
//! one. Embeddings are L2-normalised, so their dot product is the cosine similarity.

use std::sync::Mutex;

use image::{DynamicImage, imageops::FilterType};
use ndarray::{Array2, Array4};
use ort::session::Session;
use ort::value::TensorRef;
use photo_core::{Error, Result};

use crate::models::{Pack, Store};
use crate::runtime::{Runtime, ai_error};
use crate::tokenizer::{CONTEXT, Tokenizer};

pub const DIM: usize = 512;
const SIZE: u32 = 224;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

/// The model and its input and output names (read from the files).
struct Model {
    session: Mutex<Session>,
    inputs: Vec<String>,
    output: String,
}

impl Model {
    fn load(
        runtime: &Runtime,
        path: &std::path::Path,
        wanted_output: &str,
        threads: usize,
    ) -> Result<Model> {
        let session = runtime.session(path, threads)?;
        let inputs = session
            .inputs()
            .iter()
            .map(|i| i.name().to_owned())
            .collect();
        let outputs: Vec<String> = session
            .outputs()
            .iter()
            .map(|o| o.name().to_owned())
            .collect();
        let output = outputs
            .iter()
            .find(|o| o.as_str() == wanted_output)
            .or(outputs.first())
            .cloned()
            .ok_or_else(|| Error::Invalid("The search model has no output".into()))?;
        Ok(Model {
            session: Mutex::new(session),
            inputs,
            output,
        })
    }
}

pub struct Clip {
    vision: Model,
    text: Model,
    tokenizer: Tokenizer,
}

/// CLIP's input: shortest side scaled to 224 (bicubic), centre 224×224, normalised,
/// channels first.
pub fn preprocess(image: &DynamicImage, out: &mut [f32]) {
    let (w, h) = (image.width().max(1), image.height().max(1));
    let scale = SIZE as f32 / w.min(h) as f32;
    let (nw, nh) = (
        ((w as f32 * scale).round() as u32).max(SIZE),
        ((h as f32 * scale).round() as u32).max(SIZE),
    );
    let resized = image.resize_exact(nw, nh, FilterType::CatmullRom).to_rgb8();
    let (x0, y0) = ((nw - SIZE) / 2, (nh - SIZE) / 2);
    let plane = (SIZE * SIZE) as usize;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = resized.get_pixel(x0 + x, y0 + y).0;
            let i = (y * SIZE + x) as usize;
            for c in 0..3 {
                out[c * plane + i] = (p[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
    }
}

fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

impl Clip {
    /// Loads the search model (checking its files first).
    pub fn load(store: &Store, threads: usize) -> Result<Clip> {
        store.verify(Pack::Search)?;
        let runtime = Runtime::get(store)?;
        let vision = Model::load(
            runtime,
            &store.path(Pack::Search, "vision.onnx"),
            "image_embeds",
            threads,
        )?;
        let text = Model::load(
            runtime,
            &store.path(Pack::Search, "text.onnx"),
            "text_embeds",
            threads,
        )?;
        let tokenizer = Tokenizer::new(
            &std::fs::read_to_string(store.path(Pack::Search, "vocab.json"))?,
            &std::fs::read_to_string(store.path(Pack::Search, "merges.txt"))?,
        )?;
        Ok(Clip {
            vision,
            text,
            tokenizer,
        })
    }

    /// One embedding per image. Each image runs on its own: the 8-bit model scales its
    /// values over the whole input, so a batch would make one photo's embedding depend on
    /// the others in it.
    pub fn embed_images(&self, images: &[DynamicImage]) -> Result<Vec<Vec<f32>>> {
        let name = self
            .vision
            .inputs
            .first()
            .cloned()
            .unwrap_or_else(|| "pixel_values".into());
        let plane = (3 * SIZE * SIZE) as usize;
        let mut out = Vec::with_capacity(images.len());
        let mut session = self
            .vision
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for image in images {
            let mut pixels = vec![0f32; plane];
            preprocess(image, &mut pixels);
            let input = Array4::from_shape_vec((1, 3, SIZE as usize, SIZE as usize), pixels)
                .map_err(ai_error)?;
            let outputs = session
                .run(ort::inputs![name.as_str() => TensorRef::from_array_view(&input).map_err(ai_error)?])
                .map_err(ai_error)?;
            out.extend(rows(
                outputs[self.vision.output.as_str()]
                    .try_extract_array::<f32>()
                    .map_err(ai_error)?,
                1,
            )?);
        }
        Ok(out)
    }

    /// The embedding of a description.
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let (ids, real) = self.tokenizer.encode(text);
        let ids = Array2::from_shape_vec((1, CONTEXT), ids).map_err(ai_error)?;
        let mask = Array2::from_shape_fn((1, CONTEXT), |(_, i)| i64::from(i < real));
        let mut session = self.text.session.lock().unwrap_or_else(|e| e.into_inner());
        let mut inputs: Vec<(std::borrow::Cow<str>, ort::session::SessionInputValue)> = Vec::new();
        for name in &self.text.inputs {
            let tensor = if name.contains("mask") {
                TensorRef::from_array_view(&mask)
            } else {
                TensorRef::from_array_view(&ids)
            }
            .map_err(ai_error)?;
            inputs.push((name.clone().into(), tensor.into()));
        }
        let outputs = session.run(inputs).map_err(ai_error)?;
        let mut v = rows(
            outputs[self.text.output.as_str()]
                .try_extract_array::<f32>()
                .map_err(ai_error)?,
            1,
        )?;
        Ok(v.remove(0))
    }
}

/// `n` normalised rows of an `[n, DIM]` output.
fn rows(array: ndarray::ArrayViewD<'_, f32>, n: usize) -> Result<Vec<Vec<f32>>> {
    let flat: Vec<f32> = array.iter().copied().collect();
    if flat.len() != n * DIM {
        return Err(Error::Invalid(format!(
            "The search model gave {} values for {n} items",
            flat.len()
        )));
    }
    Ok(flat
        .chunks(DIM)
        .map(|c| {
            let mut v = c.to_vec();
            normalize(&mut v);
            v
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn preprocessing_crops_the_centre_and_normalises() {
        // Left half black, right half white, wider than tall: the centre crop keeps both.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(448, 224, |x, _| {
            if x < 224 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        }));
        let mut out = vec![0f32; 3 * 224 * 224];
        preprocess(&img, &mut out);
        let red_left = out[100 * 224 + 2];
        let red_right = out[100 * 224 + 221];
        assert!(
            (red_left - (0.0 - MEAN[0]) / STD[0]).abs() < 0.05,
            "{red_left}"
        );
        assert!(
            (red_right - (1.0 - MEAN[0]) / STD[0]).abs() < 0.05,
            "{red_right}"
        );
    }
}
