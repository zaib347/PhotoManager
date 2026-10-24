//! Faces: found with YuNet, then described with SFace (128 numbers, the same for one
//! person's face across photos), so photos of one person can be grouped. Both follow
//! OpenCV's own use of the models (`FaceDetectorYN`, `FaceRecognizerSF`): the same
//! input scaling, output decoding and face alignment.

use std::sync::Mutex;

use image::{RgbImage, imageops::FilterType};
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use photo_core::Result;

use crate::models::{Pack, Store};
use crate::runtime::{Runtime, ai_error};

/// Numbers in a face's description.
pub const DIM: usize = 128;
/// The detector's input: photos are scaled so their long edge fits.
const INPUT: u32 = 640;
/// Detections less sure than this are dropped (OpenCV's default is 0.9).
const MIN_SCORE: f32 = 0.85;
/// Overlapping detections of one face: the best is kept.
const NMS_IOU: f32 = 0.3;
/// The recogniser's input: the face turned upright and scaled so the eyes, nose and
/// mouth land on `TEMPLATE`.
const ALIGNED: u32 = 112;
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// A face in a photo, in that photo's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Eyes (the one on the left of the photo first), nose tip, mouth corners.
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

impl Face {
    /// How good a likeness it makes, 0 to 1, for picking a person's cover: detected
    /// surely, big enough to be sharp and facing the camera.
    pub fn quality(&self) -> f32 {
        let [left, right, nose, ..] = self.landmarks;
        let eyes = ((right[0] - left[0]).powi(2) + (right[1] - left[1]).powi(2))
            .sqrt()
            .max(1.0);
        let middle = (left[0] + right[0]) / 2.0;
        // 0 facing the camera, 1 turned to the side.
        let turned = ((nose[0] - middle).abs() / eyes * 2.0).min(1.0);
        let size = (self.w.min(self.h) / ALIGNED as f32).min(1.0);
        self.score * size * (1.0 - turned)
    }

    fn iou(&self, other: &Face) -> f32 {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        let overlap = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
        let union = self.w * self.h + other.w * other.h - overlap;
        if union > 0.0 { overlap / union } else { 0.0 }
    }
}

pub struct Faces {
    detector: Mutex<Session>,
    recognizer: Mutex<Session>,
}

impl Faces {
    /// Loads the face models (checking their files first).
    pub fn load(store: &Store, threads: usize) -> Result<Faces> {
        store.verify(Pack::People)?;
        let runtime = Runtime::get(store)?;
        Ok(Faces {
            detector: Mutex::new(
                runtime.session(&store.path(Pack::People, "detector.onnx"), threads)?,
            ),
            recognizer: Mutex::new(
                runtime.session(&store.path(Pack::People, "recognizer.onnx"), threads)?,
            ),
        })
    }

    /// The faces in a photo, surest first.
    pub fn detect(&self, image: &RgbImage) -> Result<Vec<Face>> {
        let (w, h) = image.dimensions();
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        // Scaled to fit, the rest left black (bottom and right).
        let scale = INPUT as f32 / w.max(h) as f32;
        let (sw, sh) = (
            ((w as f32 * scale).round() as u32).clamp(1, INPUT),
            ((h as f32 * scale).round() as u32).clamp(1, INPUT),
        );
        let small = image::imageops::resize(image, sw, sh, FilterType::Triangle);
        let side = INPUT as usize;
        let plane = side * side;
        let mut pixels = vec![0f32; 3 * plane];
        for (x, y, p) in small.enumerate_pixels() {
            let i = y as usize * side + x as usize;
            // Blue, green, red, 0–255.
            pixels[i] = p[2] as f32;
            pixels[plane + i] = p[1] as f32;
            pixels[2 * plane + i] = p[0] as f32;
        }
        let input = Array4::from_shape_vec((1, 3, side, side), pixels).map_err(ai_error)?;
        let mut session = self.detector.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session
            .run(ort::inputs!["input" => TensorRef::from_array_view(&input).map_err(ai_error)?])
            .map_err(ai_error)?;
        let mut found = Vec::new();
        for stride in [8u32, 16, 32] {
            let get = |name: &str| -> Result<Vec<f32>> {
                Ok(outputs[format!("{name}_{stride}").as_str()]
                    .try_extract_array::<f32>()
                    .map_err(ai_error)?
                    .iter()
                    .copied()
                    .collect())
            };
            let (cls, obj, bbox, kps) = (get("cls")?, get("obj")?, get("bbox")?, get("kps")?);
            let cols = (INPUT / stride) as usize;
            let s = stride as f32;
            let cells = cls
                .len()
                .min(obj.len())
                .min(bbox.len() / 4)
                .min(kps.len() / 10);
            for i in 0..cells {
                let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
                if score < MIN_SCORE {
                    continue;
                }
                let (c, r) = ((i % cols) as f32, (i / cols) as f32);
                let cx = (c + bbox[i * 4]) * s;
                let cy = (r + bbox[i * 4 + 1]) * s;
                let bw = bbox[i * 4 + 2].exp() * s;
                let bh = bbox[i * 4 + 3].exp() * s;
                let mut landmarks = [[0f32; 2]; 5];
                for (n, l) in landmarks.iter_mut().enumerate() {
                    *l = [
                        (kps[i * 10 + 2 * n] + c) * s / scale,
                        (kps[i * 10 + 2 * n + 1] + r) * s / scale,
                    ];
                }
                found.push(Face {
                    x: (cx - bw / 2.0) / scale,
                    y: (cy - bh / 2.0) / scale,
                    w: bw / scale,
                    h: bh / scale,
                    landmarks,
                    score,
                });
            }
        }
        Ok(keep_best(found))
    }

    /// The face's description, normalised (the dot product of two is their cosine
    /// similarity).
    pub fn describe(&self, image: &RgbImage, face: &Face) -> Result<Vec<f32>> {
        let aligned = align(image, &face.landmarks);
        let side = ALIGNED as usize;
        let plane = side * side;
        let mut pixels = vec![0f32; 3 * plane];
        for (x, y, p) in aligned.enumerate_pixels() {
            let i = y as usize * side + x as usize;
            // Red, green, blue, 0–255.
            for c in 0..3 {
                pixels[c * plane + i] = p[c] as f32;
            }
        }
        let input = Array4::from_shape_vec((1, 3, side, side), pixels).map_err(ai_error)?;
        let mut session = self.recognizer.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session
            .run(ort::inputs!["data" => TensorRef::from_array_view(&input).map_err(ai_error)?])
            .map_err(ai_error)?;
        let mut v: Vec<f32> = outputs["fc1"]
            .try_extract_array::<f32>()
            .map_err(ai_error)?
            .iter()
            .copied()
            .collect();
        if v.len() != DIM {
            return Err(photo_core::Error::Invalid(format!(
                "The face model gave {} values",
                v.len()
            )));
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        Ok(v)
    }
}

/// Drops detections overlapping a surer one.
fn keep_best(mut faces: Vec<Face>) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for f in faces {
        if kept.iter().all(|k| k.iou(&f) < NMS_IOU) {
            kept.push(f);
        }
    }
    kept
}

/// The similarity transform (scale, turn and shift) that best maps `from` onto `to`,
/// as (a, b, tx, ty): x' = a·x − b·y + tx, y' = b·x + a·y + ty.
fn similarity(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> (f32, f32, f32, f32) {
    let mean = |p: &[[f32; 2]; 5]| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        (sx / 5.0, sy / 5.0)
    };
    let ((fx0, fy0), (tx0, ty0)) = (mean(from), mean(to));
    let (mut num_a, mut num_b, mut den) = (0f32, 0f32, 0f32);
    for (f, t) in from.iter().zip(to) {
        let (fx, fy) = (f[0] - fx0, f[1] - fy0);
        let (tx, ty) = (t[0] - tx0, t[1] - ty0);
        num_a += fx * tx + fy * ty;
        num_b += fx * ty - fy * tx;
        den += fx * fx + fy * fy;
    }
    if den <= f32::EPSILON {
        return (1.0, 0.0, tx0 - fx0, ty0 - fy0);
    }
    let (a, b) = (num_a / den, num_b / den);
    (a, b, tx0 - (a * fx0 - b * fy0), ty0 - (b * fx0 + a * fy0))
}

/// The pixel at (x, y), blended from its four neighbours; black outside the image.
fn bilinear(image: &RgbImage, x: f32, y: f32) -> [u8; 3] {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (dx, dy) = (x - x0 as f32, y - y0 as f32);
    let at = |px: i64, py: i64| -> [f32; 3] {
        if px < 0 || py < 0 || px >= w || py >= h {
            [0.0; 3]
        } else {
            let p = image.get_pixel(px as u32, py as u32).0;
            [p[0] as f32, p[1] as f32, p[2] as f32]
        }
    };
    let (p00, p10, p01, p11) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    let mut out = [0u8; 3];
    for c in 0..3 {
        let top = p00[c] * (1.0 - dx) + p10[c] * dx;
        let bottom = p01[c] * (1.0 - dx) + p11[c] * dx;
        out[c] = (top * (1.0 - dy) + bottom * dy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// The face turned upright and scaled onto the recogniser's template.
pub fn align(image: &RgbImage, landmarks: &[[f32; 2]; 5]) -> RgbImage {
    let (a, b, tx, ty) = similarity(landmarks, &TEMPLATE);
    let det = (a * a + b * b).max(f32::EPSILON);
    RgbImage::from_fn(ALIGNED, ALIGNED, |u, v| {
        // Back from the template to the photo.
        let (du, dv) = (u as f32 - tx, v as f32 - ty);
        let x = (a * du + b * dv) / det;
        let y = (-b * du + a * dv) / det;
        image::Rgb(bilinear(image, x, y))
    })
}

/// A square picture of the face with some room around it, `size` pixels across.
pub fn portrait(image: &RgbImage, face: &Face, size: u32) -> RgbImage {
    let (w, h) = (image.width() as f32, image.height() as f32);
    let side = (face.w.max(face.h) * 1.6).min(w).min(h).max(1.0);
    let cx = face.x + face.w / 2.0;
    let cy = face.y + face.h / 2.0;
    let x0 = (cx - side / 2.0).clamp(0.0, (w - side).max(0.0));
    let y0 = (cy - side / 2.0).clamp(0.0, (h - side).max(0.0));
    let crop = image::imageops::crop_imm(
        image,
        x0 as u32,
        y0 as u32,
        (side as u32).max(1),
        (side as u32).max(1),
    )
    .to_image();
    image::imageops::resize(&crop, size, size, FilterType::CatmullRom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_recovers_a_known_transform() {
        // Template points scaled by 3, turned 20° and shifted.
        let (s, t) = (3.0f32, 20f32.to_radians());
        let (a, b) = (s * t.cos(), s * t.sin());
        let moved = TEMPLATE.map(|[x, y]| [a * x - b * y + 40.0, b * x + a * y - 7.0]);
        let (ra, rb, tx, ty) = similarity(&TEMPLATE, &moved);
        assert!((ra - a).abs() < 1e-3 && (rb - b).abs() < 1e-3, "{ra} {rb}");
        assert!(
            (tx - 40.0).abs() < 1e-2 && (ty + 7.0).abs() < 1e-2,
            "{tx} {ty}"
        );
    }

    #[test]
    fn alignment_puts_the_eyes_on_the_template() {
        // A photo with two bright dots where the eyes are, at twice the template's
        // size and shifted: aligned, they land on the template's eyes.
        let mut img = RgbImage::new(400, 400);
        let eyes = [
            [100.0 + 2.0 * TEMPLATE[0][0], 50.0 + 2.0 * TEMPLATE[0][1]],
            [100.0 + 2.0 * TEMPLATE[1][0], 50.0 + 2.0 * TEMPLATE[1][1]],
        ];
        for [x, y] in eyes {
            for dy in -2..=2 {
                for dx in -2..=2 {
                    img.put_pixel(
                        (x as i32 + dx) as u32,
                        (y as i32 + dy) as u32,
                        image::Rgb([255, 255, 255]),
                    );
                }
            }
        }
        let landmarks = TEMPLATE.map(|[x, y]| [100.0 + 2.0 * x, 50.0 + 2.0 * y]);
        let aligned = align(&img, &landmarks);
        for [x, y] in [TEMPLATE[0], TEMPLATE[1]] {
            assert!(aligned.get_pixel(x.round() as u32, y.round() as u32)[0] > 200);
        }
        assert_eq!(aligned.get_pixel(56, 20)[0], 0);
    }

    #[test]
    fn overlapping_detections_keep_the_surest() {
        let face = |x: f32, score: f32| Face {
            x,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            landmarks: [[0.0; 2]; 5],
            score,
        };
        let kept = keep_best(vec![face(0.0, 0.9), face(10.0, 0.95), face(300.0, 0.88)]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.95);
        assert_eq!(kept[1].x, 300.0);
    }
}
