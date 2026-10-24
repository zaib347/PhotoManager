//! Search by description: every photo's CLIP embedding, made from its grid thumbnail in
//! the background, kept in the library (`ai_embeddings`) as 512 bytes, and compared with
//! the embedding of what the user typed.
//!
//! Which photos match is decided per search, not by a fixed cut-off: similarities are
//! compared with how the whole library scores for that description, so "dog" and "a
//! red car at night" both find what stands out.

use std::sync::atomic::{AtomicBool, Ordering};

use image::DynamicImage;
use photo_core::db::Db;
use photo_core::library::{self, AssetSource};
use photo_core::{Result, thumbs};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::clip::{Clip, DIM};

/// Names the model behind stored embeddings; changing models re-indexes.
pub const MODEL: &str = "clip-vit-b32-q8";
/// Photos embedded per model run.
const BATCH: usize = 32;
/// Most results a search returns.
pub const MAX_RESULTS: usize = 2000;
/// A result must stand this many standard deviations above the library's average…
const MIN_Z: f32 = 2.2;
/// …and be at least this similar.
const MIN_SCORE: f32 = 0.17;

/// An embedding as stored: 8-bit values and their scale.
pub fn quantize(v: &[f32]) -> (f32, Vec<u8>) {
    let max = v.iter().fold(0f32, |m, x| m.max(x.abs()));
    let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
    let bytes = v
        .iter()
        .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8 as u8)
        .collect();
    (scale, bytes)
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub indexed: u64,
    pub total: u64,
}

/// How many photos have an embedding, out of how many can have one.
pub fn progress(conn: &Connection) -> Result<Progress> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM assets WHERE media_kind != 1 AND status = 0",
        [],
        |r| r.get(0),
    )?;
    let indexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ai_embeddings e JOIN assets a ON a.id = e.asset_id
         WHERE e.model = ?1 AND a.media_kind != 1 AND a.status = 0",
        params![MODEL],
        |r| r.get(0),
    )?;
    Ok(Progress {
        indexed: indexed as u64,
        total: total as u64,
    })
}

enum Image {
    Ready(DynamicImage),
    /// Unreadable: recorded so it isn't tried again.
    Failed,
    /// Drive not connected: another time.
    Later,
}

fn thumbnail(library: &Db, thumbs_db: &Db, id: i64) -> Image {
    if let Ok(Some(jpeg)) = thumbs::get(&thumbs_db.read(), id) {
        return image::load_from_memory(&jpeg).map_or(Image::Failed, Image::Ready);
    }
    match library::asset_source(&library.read(), id) {
        Ok(Some(AssetSource::File {
            path,
            orientation,
            look,
            ..
        })) => {
            let made = thumbs::from_file(&path, orientation, &look)
                .or_else(|_| thumbs::from_os(&path, &look));
            match made {
                Ok(jpeg) => {
                    let image = image::load_from_memory(&jpeg);
                    let _ = thumbs::put(&thumbs_db.write(), id, &jpeg);
                    image.map_or(Image::Failed, Image::Ready)
                }
                Err(_) => Image::Failed,
            }
        }
        Ok(Some(AssetSource::Unavailable { .. })) => Image::Later,
        _ => Image::Failed,
    }
}

/// Embeds up to `limit` photos that have no embedding yet, in batches. `pause()` is
/// checked between batches (scans, battery) and stops the pass when true, as does
/// `cancel`. Calls `report` after each batch. Returns how many photos were done
/// (unreadable ones included: they're recorded as such).
pub fn index_pending(
    library: &Db,
    thumbs_db: &Db,
    clip: &Clip,
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
            let mut stmt = conn.prepare_cached(
                "SELECT a.id FROM assets a
                 WHERE a.media_kind != 1 AND a.status = 0 AND a.id > ?1
                   AND NOT EXISTS (SELECT 1 FROM ai_embeddings e WHERE e.asset_id = a.id AND e.model = ?2)
                 ORDER BY a.id LIMIT ?3",
            )?;
            stmt.query_map(params![after, MODEL, BATCH as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let Some(&last) = batch.last() else { break };
        after = last;
        let images: Vec<(i64, Image)> = std::thread::scope(|s| {
            let handles: Vec<_> = batch
                .iter()
                .map(|&id| s.spawn(move || (id, thumbnail(library, thumbs_db, id))))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("thumbnail thread"))
                .collect()
        });
        let ready: Vec<(i64, DynamicImage)> = images
            .iter()
            .filter_map(|(id, i)| match i {
                Image::Ready(img) => Some((*id, img.clone())),
                _ => None,
            })
            .collect();
        let pictures: Vec<DynamicImage> = ready.iter().map(|(_, i)| i.clone()).collect();
        let vectors = clip.embed_images(&pictures)?;
        let mut conn = library.write();
        let tx = conn.transaction()?;
        {
            let mut put = tx.prepare_cached(
                "INSERT INTO ai_embeddings (asset_id, model, scale, vec) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (asset_id) DO UPDATE SET model = excluded.model, scale = excluded.scale,
                     vec = excluded.vec",
            )?;
            for ((id, _), v) in ready.iter().zip(&vectors) {
                let (scale, bytes) = quantize(v);
                put.execute(params![id, MODEL, scale, bytes])?;
                done += 1;
            }
            for (id, image) in &images {
                if matches!(image, Image::Failed) {
                    put.execute(params![id, MODEL, 0.0, Vec::<u8>::new()])?;
                    done += 1;
                }
            }
        }
        tx.commit()?;
        drop(conn);
        report();
    }
    Ok(done)
}

/// Every embedding, for searching in memory (512 bytes a photo).
pub struct Index {
    ids: Vec<i64>,
    scales: Vec<f32>,
    data: Vec<i8>,
}

impl Index {
    pub fn load(conn: &Connection) -> Result<Index> {
        let mut stmt = conn.prepare(
            "SELECT e.asset_id, e.scale, e.vec FROM ai_embeddings e JOIN assets a ON a.id = e.asset_id
             WHERE e.model = ?1 AND length(e.vec) = ?2 AND a.status = 0",
        )?;
        let mut index = Index {
            ids: Vec::new(),
            scales: Vec::new(),
            data: Vec::new(),
        };
        let mut rows = stmt.query(params![MODEL, DIM as i64])?;
        while let Some(row) = rows.next()? {
            let bytes: Vec<u8> = row.get(2)?;
            index.ids.push(row.get(0)?);
            index.scales.push(row.get(1)?);
            index.data.extend(bytes.iter().map(|&b| b as i8));
        }
        Ok(index)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Photos matching `query` (a text embedding), best first, with their similarity.
    pub fn search(&self, query: &[f32]) -> Vec<(i64, f32)> {
        if self.ids.is_empty() || query.len() != DIM {
            return Vec::new();
        }
        let scores: Vec<f32> = self
            .data
            .chunks(DIM)
            .zip(&self.scales)
            .map(|(v, s)| {
                v.iter()
                    .zip(query)
                    .map(|(&a, &b)| a as f32 * b)
                    .sum::<f32>()
                    * s
            })
            .collect();
        let n = scores.len() as f32;
        let mean = scores.iter().sum::<f32>() / n;
        let std = (scores.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n)
            .sqrt()
            .max(1e-4);
        let mut hits: Vec<(i64, f32)> = self
            .ids
            .iter()
            .zip(&scores)
            .filter(|&(_, &s)| s >= MIN_SCORE && (n < 20.0 || (s - mean) / std >= MIN_Z))
            .map(|(&id, &s)| (id, s))
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(MAX_RESULTS);
        hits
    }
}

/// Things the library can be browsed by, without typing: (name, description searched).
pub const SMART_ALBUMS: &[(&str, &str)] = &[
    ("Beaches", "a photo of a beach"),
    ("Mountains", "a photo of mountains"),
    ("Snow", "a photo of snow"),
    ("Sunsets", "a photo of a sunset"),
    ("Food", "a photo of food on a plate"),
    ("Pets", "a photo of a pet dog or cat"),
    ("Flowers", "a photo of flowers"),
    ("Cars", "a photo of a car"),
    ("Babies", "a photo of a baby"),
    ("Parties", "a photo of a party or celebration"),
    ("Cities", "a photo of a city skyline"),
    ("Documents", "a photo of a document or receipt"),
    ("Screenshots", "a screenshot of a phone or computer screen"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_vectors_keep_their_direction() {
        let v: Vec<f32> = (0..DIM).map(|i| ((i as f32) * 0.37).sin()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let v: Vec<f32> = v.iter().map(|x| x / norm).collect();
        let (scale, bytes) = quantize(&v);
        let back: Vec<f32> = bytes.iter().map(|&b| b as i8 as f32 * scale).collect();
        let dot: f32 = v.iter().zip(&back).map(|(a, b)| a * b).sum();
        assert!(dot > 0.999, "{dot}");
    }

    #[test]
    fn search_keeps_what_stands_out() {
        // 100 photos facing one way, three facing the query.
        let query: Vec<f32> = (0..DIM).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let mut index = Index {
            ids: Vec::new(),
            scales: Vec::new(),
            data: Vec::new(),
        };
        for id in 0..103i64 {
            let mut v = vec![0f32; DIM];
            if id >= 100 {
                v[0] = 0.3;
                v[1] = 0.95;
            } else {
                v[0] = 0.15 + (id % 7) as f32 * 0.005;
                v[2] = 0.98;
            }
            let (scale, bytes) = quantize(&v);
            index.ids.push(id);
            index.scales.push(scale);
            index.data.extend(bytes.iter().map(|&b| b as i8));
        }
        let hits = index.search(&query);
        let ids: Vec<i64> = hits.iter().map(|h| h.0).collect();
        assert_eq!(ids.len(), 3, "{hits:?}");
        assert!(ids.iter().all(|&i| i >= 100));
    }
}
