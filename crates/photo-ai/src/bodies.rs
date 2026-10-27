//! People seen whole, not just their faces: YOLOX-S (Apache 2.0, Megvii), from the
//! OpenCV model zoo, finds people (its class 0 of 80) anywhere in a photo. People runs
//! it on photos where no face was found, so photos of someone seen from behind, far
//! away or with their face hidden stay out of "No people". It says only that someone
//! is there, never who.
//!
//! Input and output follow OpenCV's own use of the model: RGB 0–255, scaled to fit
//! 640 × 640 with the rest grey (114) at the bottom and right; out come 8,400 candidate
//! boxes (one per cell of the 8, 16 and 32 pixel grids), each with an objectness and 80
//! class scores, already squashed to 0–1.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{RgbImage, imageops::FilterType};
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use photo_core::db::Db;
use photo_core::{Error, Result};
use rusqlite::{Connection, params};

use crate::models::{Pack, Store};
use crate::picture::{Picture, picture};
use crate::runtime::{Runtime, ai_error};
use crate::search::Progress;

/// The model's file in its pack.
pub const MODEL_FILE: &str = "persons.onnx";
/// The model's input side.
const INPUT: u32 = 640;
/// The grids the model predicts on, finest first.
const STRIDES: [u32; 3] = [8, 16, 32];
/// Numbers per candidate: box (4), objectness, 80 class scores.
const ROW: usize = 85;
/// A person must score at least this (objectness × "person"). Measured 2026-10-27 on
/// 137 family photos and 200 random images: real people with no face found scored 0.62
/// and up; the one thing mistaken for a person (a festival poster) scored 0.57.
pub const MIN_SCORE: f32 = 0.6;
/// Overlapping detections of one person: the surest is kept.
const NMS_IOU: f32 = 0.45;

/// A person in a photo, in that photo's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
}

impl Body {
    fn iou(&self, other: &Body) -> f32 {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        let overlap = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
        let union = self.w * self.h + other.w * other.h - overlap;
        if union > 0.0 { overlap / union } else { 0.0 }
    }
}

pub struct Bodies {
    session: Mutex<Session>,
    input: String,
    output: String,
}

impl Bodies {
    /// Loads the installed model (checking its file first).
    pub fn load(store: &Store, threads: usize) -> Result<Bodies> {
        store.verify(Pack::Bodies)?;
        Bodies::open(
            Runtime::get(store)?,
            &store.path(Pack::Bodies, MODEL_FILE),
            threads,
        )
    }

    /// Loads a model file directly (development and tests).
    pub fn open(runtime: &Runtime, model: &Path, threads: usize) -> Result<Bodies> {
        let session = runtime.session(model, threads)?;
        let name = |outlets: &[ort::value::Outlet]| -> Result<String> {
            outlets
                .first()
                .map(|o| o.name().to_owned())
                .ok_or_else(|| Error::Invalid("The person model has no inputs or outputs".into()))
        };
        Ok(Bodies {
            input: name(session.inputs())?,
            output: name(session.outputs())?,
            session: Mutex::new(session),
        })
    }

    /// The people in a photo scoring at least `min_score`, surest first.
    pub fn detect(&self, image: &RgbImage, min_score: f32) -> Result<Vec<Body>> {
        let (w, h) = image.dimensions();
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        let scale = INPUT as f32 / w.max(h) as f32;
        let (sw, sh) = (
            ((w as f32 * scale) as u32).clamp(1, INPUT),
            ((h as f32 * scale) as u32).clamp(1, INPUT),
        );
        let small = image::imageops::resize(image, sw, sh, FilterType::Triangle);
        let side = INPUT as usize;
        let plane = side * side;
        let mut pixels = vec![114f32; 3 * plane];
        for (x, y, p) in small.enumerate_pixels() {
            let i = y as usize * side + x as usize;
            for c in 0..3 {
                pixels[c * plane + i] = p[c] as f32;
            }
        }
        let input = Array4::from_shape_vec((1, 3, side, side), pixels).map_err(ai_error)?;
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session
            .run(ort::inputs![self.input.as_str() => TensorRef::from_array_view(&input).map_err(ai_error)?])
            .map_err(ai_error)?;
        let values = outputs[self.output.as_str()]
            .try_extract_array::<f32>()
            .map_err(ai_error)?;
        let values = values
            .as_slice()
            .ok_or_else(|| Error::Invalid("The person model gave an unexpected layout".into()))?;
        decode(values, scale, min_score)
    }
}

/// Names the model behind stored results; changing models looks again.
pub const MODEL: &str = "yolox-s-2022nov";
/// Photos read per batch.
const BATCH: usize = 8;
/// Photos are read with their long edge about this long (the model sees 640), and at
/// most…
const READ_EDGE: u32 = 960;
/// …this.
const MAX_EDGE: u32 = 1600;

/// Photos People looked at and found no face in: those this looks at.
const NO_FACE: &str = "FROM ai_face_scans s JOIN assets a ON a.id = s.asset_id
     WHERE s.faces = 0 AND a.status = 0 AND a.media_kind != 1";

/// How many photos with no face have been looked at for people, out of how many.
pub fn progress(conn: &Connection) -> Result<Progress> {
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) {NO_FACE}"), [], |r| r.get(0))?;
    let indexed: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) {NO_FACE}
               AND EXISTS (SELECT 1 FROM ai_body_scans b WHERE b.asset_id = s.asset_id AND b.model = ?1)"
        ),
        params![MODEL],
        |r| r.get(0),
    )?;
    Ok(Progress {
        indexed: indexed as u64,
        total: total as u64,
    })
}

/// Looks for people in up to `limit` photos with no face that haven't been looked at,
/// in batches. `pause()` is checked between batches (folder scans) and stops the pass
/// when true, as does `cancel`. Calls `report` after each batch. Returns how many
/// photos were looked at (unreadable ones included: they're recorded as such).
pub fn index_pending(
    library: &Db,
    bodies: &Bodies,
    limit: usize,
    cancel: &AtomicBool,
    pause: &dyn Fn() -> bool,
    report: &dyn Fn(),
) -> Result<usize> {
    let mut after = i64::MIN;
    let mut done = 0;
    while done < limit && !cancel.load(Ordering::Relaxed) && !pause() {
        let batch: Vec<i64> = {
            let conn = library.read();
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT s.asset_id {NO_FACE} AND s.asset_id > ?1
                   AND NOT EXISTS (SELECT 1 FROM ai_body_scans b WHERE b.asset_id = s.asset_id AND b.model = ?2)
                 ORDER BY s.asset_id LIMIT ?3"
            ))?;
            stmt.query_map(params![after, MODEL, BATCH as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let Some(&last) = batch.last() else { break };
        after = last;
        let pictures: Vec<(i64, Picture)> = std::thread::scope(|s| {
            let handles: Vec<_> = batch
                .iter()
                .map(|&id| s.spawn(move || (id, picture(library, id, READ_EDGE, MAX_EDGE))))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("photo reading thread"))
                .collect()
        });
        // (photo, people found), or -1 if unreadable; drives not connected wait.
        let mut results = Vec::new();
        for (id, p) in pictures {
            match p {
                Picture::Ready(image) => {
                    results.push((id, bodies.detect(&image, MIN_SCORE)?.len() as i64))
                }
                Picture::Failed => results.push((id, -1)),
                Picture::Later => {}
            }
        }
        let mut conn = library.write();
        let tx = conn.transaction()?;
        for (id, count) in &results {
            tx.execute(
                "INSERT INTO ai_body_scans (asset_id, model, bodies) VALUES (?1, ?2, ?3)
                 ON CONFLICT (asset_id) DO UPDATE SET model = excluded.model, bodies = excluded.bodies",
                params![id, MODEL, count],
            )?;
        }
        tx.commit()?;
        drop(conn);
        done += results.len();
        report();
    }
    Ok(done)
}

/// Forgets everything found (the feature was removed).
pub fn clear(library: &Db) -> Result<()> {
    library.write().execute("DELETE FROM ai_body_scans", [])?;
    Ok(())
}

/// How many candidates the model gives for one input.
fn candidates() -> usize {
    STRIDES
        .iter()
        .map(|s| ((INPUT / s) * (INPUT / s)) as usize)
        .sum()
}

/// The people among the model's candidates, in photo pixels (`scale`: input pixels per
/// photo pixel), overlapping ones merged, surest first.
fn decode(values: &[f32], scale: f32, min_score: f32) -> Result<Vec<Body>> {
    if values.len() != candidates() * ROW {
        return Err(Error::Invalid(format!(
            "The person model gave {} values",
            values.len()
        )));
    }
    let mut found = Vec::new();
    let mut rows = values.chunks_exact(ROW);
    for stride in STRIDES {
        let cells = INPUT / stride;
        let s = stride as f32;
        for gy in 0..cells {
            for gx in 0..cells {
                let v = rows.next().expect("counted above");
                let score = v[4] * v[5];
                if score < min_score {
                    continue;
                }
                let (cx, cy) = ((v[0] + gx as f32) * s, (v[1] + gy as f32) * s);
                let (bw, bh) = (v[2].exp() * s, v[3].exp() * s);
                found.push(Body {
                    x: (cx - bw / 2.0) / scale,
                    y: (cy - bh / 2.0) / scale,
                    w: bw / scale,
                    h: bh / scale,
                    score,
                });
            }
        }
    }
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Body> = Vec::new();
    for b in found {
        if kept.iter().all(|k| k.iou(&b) < NMS_IOU) {
            kept.push(b);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Candidates with nothing in them, and the index of the cell at (`gx`, `gy`) on the
    /// `stride` grid.
    fn empty() -> Vec<f32> {
        vec![0.0; candidates() * ROW]
    }

    fn cell(stride: u32, gx: u32, gy: u32) -> usize {
        let mut i = 0;
        for s in STRIDES {
            if s == stride {
                return i + (gy * (INPUT / s) + gx) as usize;
            }
            i += ((INPUT / s) * (INPUT / s)) as usize;
        }
        unreachable!()
    }

    fn put(values: &mut [f32], at: usize, row: [f32; 6]) {
        values[at * ROW..at * ROW + 6].copy_from_slice(&row);
    }

    #[test]
    fn people_are_read_from_their_cells_and_scaled_to_the_photo() {
        let mut values = empty();
        // A person centred in cell (10, 4) of the 16 px grid, 2^1 × 2^2 cells big.
        put(&mut values, cell(16, 10, 4), [0.5, 0.5, 1.0, 2.0, 0.9, 0.9]);
        // Something else (a high score for class 1, not people).
        let at = cell(8, 3, 3);
        put(&mut values, at, [0.5, 0.5, 0.0, 0.0, 0.95, 0.01]);
        values[at * ROW + 6] = 0.99;
        // Photo twice the input size: scale 0.5.
        let found = decode(&values, 0.5, MIN_SCORE).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        let b = &found[0];
        let (cx, cy) = (10.5 * 16.0, 4.5 * 16.0);
        let (w, h) = (1f32.exp() * 16.0, 2f32.exp() * 16.0);
        assert!((b.x - (cx - w / 2.0) * 2.0).abs() < 1e-3);
        assert!((b.y - (cy - h / 2.0) * 2.0).abs() < 1e-3);
        assert!((b.w - w * 2.0).abs() < 1e-3 && (b.h - h * 2.0).abs() < 1e-3);
        assert!((b.score - 0.81).abs() < 1e-6);
    }

    #[test]
    fn overlapping_detections_keep_the_surest_and_weak_ones_are_dropped() {
        let mut values = empty();
        put(&mut values, cell(32, 5, 5), [0.5, 0.5, 1.0, 1.0, 0.9, 0.9]);
        put(
            &mut values,
            cell(32, 5, 5) + 1,
            [0.0, 0.5, 1.0, 1.0, 0.8, 0.8],
        );
        put(
            &mut values,
            cell(32, 15, 15),
            [0.5, 0.5, 0.5, 0.5, 0.8, 0.8],
        );
        put(&mut values, cell(8, 70, 70), [0.5, 0.5, 0.0, 0.0, 0.6, 0.5]);
        // The second overlaps the first and goes; the far one stays; the last is too weak.
        let found = decode(&values, 1.0, MIN_SCORE).unwrap();
        let scores: Vec<f32> = found.iter().map(|b| (b.score * 100.0).round()).collect();
        assert_eq!(scores, vec![81.0, 64.0]);
    }

    #[test]
    fn a_wrong_sized_output_is_an_error() {
        assert!(decode(&[0.0; 85], 1.0, MIN_SCORE).is_err());
    }
}
