//! Similar photos: near-duplicates and bursts.
//!
//! Each photo gets a 64-bit perceptual hash (DCT "pHash") and a sharpness score, both
//! from its grid thumbnail, which is tiny to decode. Two kinds of group come out:
//! - similar shots: taken within `WINDOW` of each other and close in hash (bursts,
//!   retakes of the same scene);
//! - near copies: very close in hash at any time apart (resized or re-saved copies,
//!   e.g. a WhatsApp export of a camera photo).
//!
//! Near copies across the whole library are found by splitting hashes into four
//! 16-bit chunks: two hashes within `COPY_DIST` (< 4) bits share at least one chunk
//! exactly, so only photos sharing a chunk are compared.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use image::imageops::FilterType;
use image::{DynamicImage, GrayImage};
use rayon::prelude::*;
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::db::Db;
use crate::library::{self, AssetSource};
use crate::{Result, now_ms, thumbs};

/// Bumped when the hash changes, so every photo is hashed again.
pub const VERSION: i64 = 1;
/// Photos hashed per write transaction.
const BATCH: usize = 256;
/// Similar shots must be taken this close together...
const WINDOW_MS: i64 = 3 * 60_000;
/// ...among the next this many photos...
const NEIGHBOURS: usize = 30;
/// ...and differ in at most this many of 64 bits.
const SIMILAR_DIST: u32 = 10;
/// Near copies, at any time apart. Must stay below 4 for the chunk index.
const COPY_DIST: u32 = 3;
/// Chunk buckets bigger than this are near-blank images that all look alike.
const MAX_BUCKET: usize = 200;
/// Grey levels of spread below which an image has too little detail to compare.
const MIN_DETAIL: f32 = 6.0;

/// Hash and sharpness of an image, or `None` when it has too little detail to tell
/// apart from others (black frames, blank walls, pocket shots).
pub fn fingerprint(img: &DynamicImage) -> Option<(u64, f64)> {
    let luma = trim_borders(&img.to_luma8());
    let small = image::imageops::resize(&luma, 32, 32, FilterType::Triangle);
    let px: Vec<f32> = small.pixels().map(|p| p.0[0] as f32).collect();
    let mean = px.iter().sum::<f32>() / px.len() as f32;
    let spread = (px.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / px.len() as f32).sqrt();
    if spread < MIN_DETAIL {
        return None;
    }
    Some((dct_hash(&px), sharpness(&luma)))
}

/// Crops dark bands from the edges (letterboxed EXIF thumbnails), at most a quarter
/// of each side.
fn trim_borders(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let dark_row = |y: u32| (0..w).all(|x| img.get_pixel(x, y).0[0] < 24);
    let dark_col = |x: u32| (0..h).all(|y| img.get_pixel(x, y).0[0] < 24);
    let (mut top, mut bottom, mut left, mut right) = (0, h, 0, w);
    while top < h / 4 && dark_row(top) {
        top += 1;
    }
    while bottom > h - h / 4 && dark_row(bottom - 1) {
        bottom -= 1;
    }
    while left < w / 4 && dark_col(left) {
        left += 1;
    }
    while right > w - w / 4 && dark_col(right - 1) {
        right -= 1;
    }
    image::imageops::crop_imm(img, left, top, right - left, bottom - top).to_image()
}

/// Top-left 8×8 of the 32×32 DCT-II, one bit per coefficient above the median.
fn dct_hash(px: &[f32]) -> u64 {
    const N: usize = 32;
    const K: usize = 8;
    let cos: Vec<f32> = (0..K * N)
        .map(|i| {
            let (u, x) = (i / N, i % N);
            (std::f32::consts::PI * (2 * x + 1) as f32 * u as f32 / (2 * N) as f32).cos()
        })
        .collect();
    // Rows first (32×8), then columns (8×8).
    let mut rows = [0f32; N * K];
    for y in 0..N {
        for u in 0..K {
            rows[y * K + u] = (0..N).map(|x| px[y * N + x] * cos[u * N + x]).sum();
        }
    }
    let mut coef = [0f32; K * K];
    for v in 0..K {
        for u in 0..K {
            coef[v * K + u] = (0..N).map(|y| rows[y * K + u] * cos[v * N + y]).sum();
        }
    }
    let mut sorted = coef;
    sorted.sort_by(f32::total_cmp);
    let median = (sorted[31] + sorted[32]) / 2.0;
    coef.iter()
        .enumerate()
        .fold(0u64, |h, (i, &c)| if c > median { h | 1 << i } else { h })
}

/// Variance of the Laplacian at a fixed 128×128 size: higher is sharper. Comparable
/// between shots of the same scene, not across different scenes.
fn sharpness(luma: &GrayImage) -> f64 {
    let img = image::imageops::resize(luma, 128, 128, FilterType::Triangle);
    let at = |x: u32, y: u32| img.get_pixel(x, y).0[0] as f64;
    let mut values = Vec::with_capacity(126 * 126);
    for y in 1..127 {
        for x in 1..127 {
            values.push(at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * at(x, y));
        }
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64
}

pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// True when photos are waiting to be hashed.
pub fn needs_hashing(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM assets WHERE phash_v IS NULL AND status = 0)",
        [],
        |r| r.get(0),
    )?)
}

enum Outcome {
    Hashed(Option<(u64, f64)>, Option<Vec<u8>>),
    /// Videos, synthetic items, unreadable files: never compared.
    Skip,
    /// Drive not connected: try again another time.
    Later,
}

/// Hashes photos that don't have a hash yet, decoding their thumbnails (and making
/// a thumbnail from the file where there is none). Stops early when `cancel` is set.
/// Returns how many were processed.
pub fn hash_pending(library: &Db, thumbs_db: &Db, cancel: &AtomicBool) -> Result<usize> {
    let mut after = i64::MIN;
    let mut done = 0;
    while !cancel.load(Ordering::Relaxed) {
        let batch: Vec<(i64, u8, bool)> = {
            let conn = library.read();
            let mut stmt = conn.prepare_cached(
                "SELECT a.id, a.media_kind, EXISTS (SELECT 1 FROM files f WHERE f.asset_id = a.id)
                 FROM assets a WHERE a.phash_v IS NULL AND a.status = 0 AND a.id > ?1
                 ORDER BY a.id LIMIT ?2",
            )?;
            stmt.query_map(params![after, BATCH as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        let Some(&(last, _, _)) = batch.last() else {
            break;
        };
        after = last;
        let outcomes: Vec<(i64, Outcome)> = batch
            .into_par_iter()
            .map(|(id, kind, has_files)| {
                let outcome = if kind == 1 || !has_files {
                    Outcome::Skip
                } else {
                    hash_one(library, thumbs_db, id).unwrap_or(Outcome::Skip)
                };
                (id, outcome)
            })
            .collect();

        let mut new_thumbs = Vec::new();
        {
            let mut conn = library.write();
            let tx = conn.transaction()?;
            {
                let mut write = tx.prepare_cached(
                    "UPDATE assets SET phash = ?2, blur = ?3, phash_v = ?4 WHERE id = ?1",
                )?;
                for (id, outcome) in outcomes {
                    match outcome {
                        Outcome::Hashed(print, thumb) => {
                            let (hash, sharp) = print.unzip();
                            write.execute(params![id, hash.map(|h| h as i64), sharp, VERSION])?;
                            new_thumbs.extend(thumb.map(|t| (id, t)));
                        }
                        Outcome::Skip => {
                            write.execute(params![id, None::<i64>, None::<f64>, VERSION])?;
                        }
                        Outcome::Later => {}
                    }
                    done += 1;
                }
            }
            tx.commit()?;
        }
        if !new_thumbs.is_empty() {
            let mut store = thumbs_db.write();
            let tx = store.transaction()?;
            for (id, data) in new_thumbs {
                thumbs::put(&tx, id, &data)?;
            }
            tx.commit()?;
        }
    }
    Ok(done)
}

fn hash_one(library: &Db, thumbs_db: &Db, id: i64) -> Result<Outcome> {
    if let Some(jpeg) = thumbs::get(&thumbs_db.read(), id)? {
        let img = image::load_from_memory(&jpeg)?;
        return Ok(Outcome::Hashed(fingerprint(&img), None));
    }
    let source = library::asset_source(&library.read(), id)?;
    match source {
        Some(AssetSource::File {
            path, orientation, ..
        }) => {
            let jpeg = thumbs::from_file(&path, orientation).or_else(|_| thumbs::from_os(&path))?;
            let img = image::load_from_memory(&jpeg)?;
            Ok(Outcome::Hashed(fingerprint(&img), Some(jpeg)))
        }
        Some(AssetSource::Unavailable { .. }) => Ok(Outcome::Later),
        Some(AssetSource::NoFiles { .. }) | None => Ok(Outcome::Skip),
    }
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn find(&mut self, i: usize) -> usize {
        let mut root = i;
        while self.0[root] != root {
            root = self.0[root];
        }
        let mut i = i;
        while self.0[i] != root {
            let next = self.0[i];
            self.0[i] = root;
            i = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// Indices of `prints` (sorted by time) that belong together.
fn group_indices(prints: &[(i64, u64)]) -> Vec<Vec<usize>> {
    let n = prints.len();
    let mut uf = UnionFind((0..n).collect());
    for i in 0..n {
        for j in i + 1..(i + 1 + NEIGHBOURS).min(n) {
            if prints[j].0 - prints[i].0 > WINDOW_MS {
                break;
            }
            if distance(prints[i].1, prints[j].1) <= SIMILAR_DIST {
                uf.union(i, j);
            }
        }
    }
    for chunk in 0..4 {
        let mut buckets: HashMap<u16, Vec<usize>> = HashMap::new();
        for (i, &(_, hash)) in prints.iter().enumerate() {
            buckets
                .entry((hash >> (chunk * 16)) as u16)
                .or_default()
                .push(i);
        }
        for members in buckets.values() {
            if members.len() < 2 || members.len() > MAX_BUCKET {
                continue;
            }
            for (k, &a) in members.iter().enumerate() {
                for &b in &members[k + 1..] {
                    if distance(prints[a].1, prints[b].1) <= COPY_DIST {
                        uf.union(a, b);
                    }
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = uf.find(i);
        groups.entry(root).or_default().push(i);
    }
    groups.into_values().filter(|g| g.len() > 1).collect()
}

/// Regroups every hashed photo. Groups whose photos the user all kept are left out.
/// Reading and grouping happen on a reader; the writer is held only to store groups.
pub fn detect(db: &Db) -> Result<()> {
    let (mut rows, dismissed): (Vec<(i64, i64, u64)>, HashSet<i64>) = {
        let conn = db.read();
        let mut stmt = conn.prepare(
            "SELECT id, taken_at + tz_offset_min * 60000, phash FROM assets
             WHERE phash IS NOT NULL AND status = 0 AND paired_with IS NULL",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare("SELECT asset_id FROM similar_dismissed")?;
        let dismissed = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        (rows, dismissed)
    };
    rows.sort_unstable_by_key(|r| (r.1, r.0));
    let prints: Vec<(i64, u64)> = rows.iter().map(|r| (r.1, r.2)).collect();
    // (group id, kind, first wall time, member ids)
    let mut found: Vec<(i64, &str, i64, Vec<i64>)> = Vec::new();
    for indices in group_indices(&prints) {
        let ids: Vec<i64> = indices.iter().map(|&i| rows[i].0).collect();
        if ids.iter().all(|id| dismissed.contains(id)) {
            continue;
        }
        let first = indices.iter().map(|&i| rows[i].1).min().unwrap_or(0);
        let last = indices.iter().map(|&i| rows[i].1).max().unwrap_or(0);
        let kind = if last - first > WINDOW_MS {
            "copies"
        } else {
            "similar"
        };
        let id = *ids.iter().min().expect("non-empty group");
        found.push((id, kind, first, ids));
    }

    let mut conn = db.write();
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM similar_members", [])?;
    tx.execute("DELETE FROM similar_groups", [])?;
    {
        let mut group = tx.prepare(
            "INSERT INTO similar_groups (id, kind, first_wall, count) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut member =
            tx.prepare("INSERT INTO similar_members (asset_id, group_id) VALUES (?1, ?2)")?;
        for (id, kind, first, ids) in &found {
            group.execute(params![id, kind, first, ids.len() as i64])?;
            for asset in ids {
                member.execute(params![asset, id])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub groups: i64,
    pub photos: i64,
    /// Photos still waiting for their hash (groups may be incomplete until then).
    pub pending: i64,
}

pub fn overview(conn: &Connection) -> Result<Overview> {
    Ok(conn.query_row(
        "SELECT (SELECT count(*) FROM similar_groups), (SELECT count(*) FROM similar_members),
                (SELECT count(*) FROM assets WHERE phash_v IS NULL AND status = 0)",
        [],
        |r| {
            Ok(Overview {
                groups: r.get(0)?,
                photos: r.get(1)?,
                pending: r.get(2)?,
            })
        },
    )?)
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub asset_id: i64,
    pub media_kind: u8,
    pub taken_at: i64,
    pub tz_offset_min: i32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Size of its largest file.
    pub bytes: i64,
    pub favorite: bool,
    pub rating: Option<u8>,
    pub sharpness: Option<f64>,
    /// The one suggested to keep.
    pub best: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: i64,
    /// "similar" (taken close together) or "copies" (the same picture saved again).
    pub kind: String,
    pub members: Vec<Member>,
}

/// Which member to keep: favorites and ratings first; then for similar shots the
/// sharpest, for copies the biggest.
fn best_index(kind: &str, members: &[Member]) -> usize {
    let pixels = |m: &Member| m.width.unwrap_or(0) as u64 * m.height.unwrap_or(0) as u64;
    let key = |m: &Member| {
        let sharp = m.sharpness.unwrap_or(0.0);
        let (first, second) = if kind == "copies" {
            (pixels(m) as f64, m.bytes as f64)
        } else {
            (sharp, pixels(m) as f64)
        };
        (m.favorite, m.rating.unwrap_or(0), first, second)
    };
    (0..members.len())
        .max_by(|&a, &b| {
            let (ka, kb) = (key(&members[a]), key(&members[b]));
            (ka.0, ka.1)
                .cmp(&(kb.0, kb.1))
                .then(ka.2.total_cmp(&kb.2))
                .then(ka.3.total_cmp(&kb.3))
                .then(b.cmp(&a)) // earlier one on a full tie
        })
        .unwrap_or(0)
}

/// A page of groups, newest first, members in time order.
pub fn groups(conn: &Connection, offset: usize, limit: usize) -> Result<Vec<Group>> {
    let page: Vec<(i64, String)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, kind FROM similar_groups ORDER BY first_wall DESC, id LIMIT ?1 OFFSET ?2",
        )?;
        stmt.query_map(params![limit as i64, offset as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.media_kind, a.taken_at, a.tz_offset_min, a.width, a.height,
                COALESCE((SELECT max(size) FROM files f WHERE f.asset_id = a.id AND f.missing_since IS NULL), 0),
                COALESCE(u.favorite, 0), u.rating, a.blur
         FROM similar_members m JOIN assets a ON a.id = m.asset_id
         LEFT JOIN user_meta u ON u.asset_id = a.id
         WHERE m.group_id = ?1 AND a.status = 0
         ORDER BY a.taken_at + a.tz_offset_min * 60000, a.id",
    )?;
    let mut out = Vec::with_capacity(page.len());
    for (id, kind) in page {
        let mut members: Vec<Member> = stmt
            .query_map(params![id], |r| {
                Ok(Member {
                    asset_id: r.get(0)?,
                    media_kind: r.get(1)?,
                    taken_at: r.get(2)?,
                    tz_offset_min: r.get(3)?,
                    width: r.get(4)?,
                    height: r.get(5)?,
                    bytes: r.get(6)?,
                    favorite: r.get(7)?,
                    rating: r.get(8)?,
                    sharpness: r.get(9)?,
                    best: false,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        if members.len() < 2 {
            continue; // some were removed since the last detection
        }
        let best = best_index(&kind, &members);
        members[best].best = true;
        out.push(Group { id, kind, members });
    }
    Ok(out)
}

/// Every present file of these photos, for removing them.
pub fn files_of_assets(conn: &Connection, asset_ids: &[i64]) -> Result<Vec<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM files WHERE asset_id = ?1 AND missing_since IS NULL")?;
    let mut out = Vec::new();
    for &id in asset_ids {
        for file in stmt.query_map(params![id], |r| r.get::<_, i64>(0))? {
            out.push(file?);
        }
    }
    Ok(out)
}

/// The user looked at these and keeps them all: their group stops showing (until a
/// new similar photo joins it).
pub fn dismiss(db: &Db, asset_ids: &[i64]) -> Result<()> {
    {
        let mut conn = db.write();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO similar_dismissed (asset_id, at) VALUES (?1, ?2)",
            )?;
            let now = now_ms();
            for id in asset_ids {
                stmt.execute(params![id, now])?;
            }
        }
        tx.commit()?;
    }
    detect(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A scene of soft blobs; `seed` moves them around.
    fn scene(seed: u32, w: u32, h: u32) -> RgbImage {
        let blobs: Vec<(f32, f32, f32)> = (0..6)
            .map(|i| {
                let k = (seed * 7 + i * 13) as f32;
                (
                    (k * 0.37).sin().abs(),
                    (k * 0.73).cos().abs(),
                    40.0 + (k % 5.0) * 30.0,
                )
            })
            .collect();
        RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let v: f32 = blobs
                .iter()
                .map(|&(bx, by, b)| b * (-((fx - bx).powi(2) + (fy - by).powi(2)) * 12.0).exp())
                .sum();
            let v = (60.0 + v).min(255.0) as u8;
            Rgb([v, v / 2 + 40, 255 - v / 2])
        })
    }

    fn print(img: RgbImage) -> (u64, f64) {
        fingerprint(&DynamicImage::ImageRgb8(img)).expect("detailed enough")
    }

    #[test]
    fn copies_and_retakes_hash_close_and_other_scenes_far() {
        let (base, base_sharp) = print(scene(1, 256, 192));
        // A smaller re-save.
        let resized = image::imageops::resize(&scene(1, 256, 192), 160, 120, FilterType::Triangle);
        assert!(distance(base, print(resized).0) <= COPY_DIST);
        // Letterboxed, as many EXIF thumbnails are.
        let mut boxed = RgbImage::from_pixel(256, 232, Rgb([0, 0, 0]));
        image::imageops::overlay(&mut boxed, &scene(1, 256, 192), 0, 20);
        assert!(
            distance(base, print(boxed).0) <= COPY_DIST,
            "black bands are trimmed"
        );
        // Brighter, as a retake might be.
        let mut brighter = scene(1, 256, 192);
        brighter
            .pixels_mut()
            .for_each(|p| p.0 = p.0.map(|c| c.saturating_add(20)));
        assert!(distance(base, print(brighter).0) <= SIMILAR_DIST);
        // Something else entirely.
        for seed in 2..6 {
            assert!(
                distance(base, print(scene(seed, 256, 192)).0) > SIMILAR_DIST,
                "seed {seed}"
            );
        }
        assert!(base_sharp > 0.0);
    }

    #[test]
    fn blur_lowers_sharpness() {
        // Hard-edged squares on a gradient.
        let crisp = RgbImage::from_fn(256, 192, |x, y| {
            let on = (x / 32 + y / 32) % 2 == 0;
            let v = if on { 220 } else { (x / 2) as u8 };
            Rgb([v, v, v])
        });
        let blurred = image::imageops::blur(&crisp, 3.0);
        assert!(print(blurred).1 < print(crisp).1 / 2.0);
    }

    #[test]
    fn blank_frames_are_not_compared() {
        let black = RgbImage::from_pixel(64, 64, Rgb([3, 3, 3]));
        assert!(fingerprint(&DynamicImage::ImageRgb8(black)).is_none());
    }

    #[test]
    fn groups_bursts_by_time_and_copies_at_any_time() {
        const MIN: i64 = 60_000;
        let a: u64 = 0xF0F0_1234_ABCD_5555;
        let b: u64 = 0x0F0F_9876_1357_AAAA; // far from a
        let prints = [
            (0, a),
            (MIN, a ^ 0b1111_1111), // 8 bits off, a minute later: same burst
            // An hour later, 4 bits from the burst's second shot: too far for a copy.
            (60 * MIN, a ^ 0b1111_1111 ^ (0b1111 << 20)),
            (90 * MIN, b),
            (400 * 24 * 60 * MIN, b ^ 0b11), // a year later, 2 bits off: a copy
            (500 * 24 * 60 * MIN, a ^ (0b1111_1111 << 40)), // far in time and bits
        ];
        let mut groups: Vec<Vec<usize>> = group_indices(&prints);
        groups.iter_mut().for_each(|g| g.sort_unstable());
        groups.sort();
        assert_eq!(groups, [vec![0, 1], vec![3, 4]]);
    }
}
