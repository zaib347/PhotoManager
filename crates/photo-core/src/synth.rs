//! Synthetic library generator for scale testing without real photos.
//!
//! Timestamps are clustered into events (bursts, outings, multi-day trips) and
//! skewed towards recent years, roughly like a real phone-camera library.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::Result;
use crate::timeline::{self, DAY_MS};

const START_MS: i64 = 1_199_145_600_000; // 2008-01-01
const BATCH: usize = 50_000;
const CAMERAS: &[(&str, &str)] = &[
    ("Apple", "iPhone 15 Pro"),
    ("Apple", "iPhone 11"),
    ("samsung", "SM-S918B"),
    ("Canon", "EOS 80D"),
    ("SONY", "ILCE-7M3"),
];
/// Most outings are around home (Lahore); trips go to one of the others.
const HOME: (f64, f64) = (31.52, 74.36);
const AWAY: &[(f64, f64)] = &[
    (33.69, 73.05),  // Islamabad
    (35.92, 74.31),  // Gilgit
    (25.20, 55.27),  // Dubai
    (41.01, 28.98),  // Istanbul
    (51.51, -0.13),  // London
    (40.71, -74.01), // New York
];

/// SplitMix64: tiny, fast and deterministic, which is all a test generator needs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

pub fn generate(conn: &mut Connection, count: usize, seed: u64) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let span = (now - START_MS) as f64;
    let mut rng = Rng(seed);
    let mut written = 0;

    while written < count {
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO assets (media_kind, taken_at, taken_src, tz_offset_min, local_day,
                     width, height, orientation, duration_ms, camera_make, camera_model, created_at,
                     lat, lon)
                 VALUES (?1, ?2, 0, 0, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            let batch_end = (written + BATCH).min(count);
            while written < batch_end {
                // sqrt skews event starts towards recent years.
                let event_start = START_MS + (span * rng.unit().sqrt()) as i64;
                let size = (1.0 + -rng.unit().max(1e-9).ln() * 30.0).min(3000.0) as usize;
                let size = size.min(batch_end - written);
                let event_len = if rng.below(10) == 0 {
                    (2 + rng.below(9)) as i64 * DAY_MS // multi-day trip
                } else {
                    (size as i64 * 120_000).min(10 * 3_600_000)
                };
                let (make, model) = CAMERAS[rng.below(CAMERAS.len() as u64) as usize];
                // A quarter of events have no GPS (old cameras, location turned off).
                let spot = (rng.below(4) != 0).then(|| {
                    if event_len > DAY_MS || rng.below(10) == 0 {
                        AWAY[rng.below(AWAY.len() as u64) as usize]
                    } else {
                        (
                            HOME.0 + (rng.unit() - 0.5) * 0.2,
                            HOME.1 + (rng.unit() - 0.5) * 0.2,
                        )
                    }
                });

                for _ in 0..size {
                    let taken_at = (event_start + (rng.unit() * event_len as f64) as i64).min(now);
                    let is_video = rng.below(100) < 8;
                    let (w, h) = if rng.below(4) == 0 {
                        (3024, 4032)
                    } else {
                        (4032, 3024)
                    };
                    let duration = is_video.then(|| 3_000 + rng.below(87_000) as i64);
                    let gps = spot.map(|(lat, lon)| {
                        (
                            lat + (rng.unit() - 0.5) * 0.02,
                            lon + (rng.unit() - 0.5) * 0.02,
                        )
                    });
                    insert.execute(params![
                        is_video as u8,
                        taken_at,
                        timeline::local_day(taken_at, 0),
                        w,
                        h,
                        duration,
                        make,
                        model,
                        now,
                        gps.map(|g| g.0),
                        gps.map(|g| g.1),
                    ])?;
                }
                written += size;
            }
        }
        tx.commit()?;
    }

    timeline::rebuild_day_buckets(conn)
}

/// Removes synthetic assets (those with no files). Real library content is untouched.
pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM assets WHERE NOT EXISTS (SELECT 1 FROM files f WHERE f.asset_id = assets.id)",
        [],
    )?;
    timeline::rebuild_day_buckets(conn)
}
