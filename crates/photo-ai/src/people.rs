//! People: faces found in every photo in the background, grouped by likeness into
//! people the user can name, merge and correct.
//!
//! Each face joins the person whose average face it resembles most, if close enough,
//! or starts a new one; after each pass, people who turn out to look alike are merged.
//! Two faces in one photo never join the same person, two named people are never
//! merged, and a face the user takes out of a person ("not this person") is never
//! grouped again. Everything stays in the library (`ai_people`, `ai_faces`), and each
//! face's portrait in the thumbnail cache.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use image::RgbImage;
use photo_core::db::Db;
use photo_core::{Result, thumbs};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::Serialize;

use crate::faces::{DIM, Face, Faces, portrait};
use crate::picture::{Picture, picture};
use crate::search::{Progress, quantize};

/// Names the models behind stored faces; changing models finds faces again.
pub const MODEL: &str = "yunet-2023mar+sface-2021dec";
/// Photos read per batch.
const BATCH: usize = 8;
/// A face joins a person whose average face is at least this similar, and two
/// people this similar are merged. SFace's own threshold for "same person" between
/// two single faces is 0.363; an average face is a steadier likeness, so this can be
/// stricter, and a wrong merge is worse than a person split in two.
pub const JOIN: f32 = 0.45;
/// Faces smaller than this (pixels across, in the photo as read) are too small to
/// recognise reliably.
const MIN_FACE: f32 = 40.0;
/// At most this many faces are kept per photo (the biggest crowds).
const MAX_FACES: usize = 20;
/// Photos are read with their long edge about this long, and at most…
const READ_EDGE: u32 = 1600;
/// …this.
const MAX_EDGE: u32 = 2400;
/// People with fewer photos than this aren't listed until named.
pub const MIN_PHOTOS: i64 = 3;
/// Portraits are this many pixels across.
pub const PORTRAIT: u32 = 160;

/// People as groups of face descriptions, for grouping new faces.
#[derive(Default)]
pub struct Groups {
    ids: Vec<i64>,
    /// Sums of each group's faces (DIM values each).
    sums: Vec<f32>,
    norms: Vec<f32>,
    counts: Vec<u32>,
    named: Vec<bool>,
    alive: Vec<bool>,
    slots: HashMap<i64, usize>,
    /// Groups changed since the last `merges`.
    touched: HashSet<usize>,
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl Groups {
    /// Everyone in the library.
    pub fn load(conn: &Connection) -> Result<Groups> {
        let mut groups = Groups::default();
        let mut stmt = conn.prepare("SELECT id, name IS NOT NULL FROM ai_people")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            groups.create(row.get(0)?, row.get(1)?);
        }
        let mut stmt = conn.prepare(
            "SELECT person_id, scale, vec FROM ai_faces WHERE person_id IS NOT NULL AND length(vec) = ?1",
        )?;
        let mut rows = stmt.query(params![DIM as i64])?;
        while let Some(row) = rows.next()? {
            let (id, scale): (i64, f32) = (row.get(0)?, row.get(1)?);
            let bytes: Vec<u8> = row.get(2)?;
            if let Some(&slot) = groups.slots.get(&id) {
                let v: Vec<f32> = bytes.iter().map(|&b| b as i8 as f32 * scale).collect();
                groups.add(slot, &v);
            }
        }
        groups.touched.clear();
        Ok(groups)
    }

    /// A new, empty group for person `id`.
    pub fn create(&mut self, id: i64, named: bool) -> usize {
        let slot = self.ids.len();
        self.ids.push(id);
        self.sums.extend(std::iter::repeat_n(0.0, DIM));
        self.norms.push(0.0);
        self.counts.push(0);
        self.named.push(named);
        self.alive.push(true);
        self.slots.insert(id, slot);
        slot
    }

    pub fn add(&mut self, slot: usize, v: &[f32]) {
        let sum = &mut self.sums[slot * DIM..(slot + 1) * DIM];
        sum.iter_mut().zip(v).for_each(|(s, x)| *s += x);
        self.norms[slot] = dot(sum, sum).sqrt();
        self.counts[slot] += 1;
        self.touched.insert(slot);
    }

    pub fn id(&self, slot: usize) -> i64 {
        self.ids[slot]
    }

    /// The group most like `v` and how alike (cosine to its average face), leaving
    /// out `except`.
    pub fn closest(&self, v: &[f32], except: &HashSet<usize>) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32)> = None;
        for slot in 0..self.ids.len() {
            if !self.alive[slot] || self.counts[slot] == 0 || except.contains(&slot) {
                continue;
            }
            let sim = dot(v, &self.sums[slot * DIM..(slot + 1) * DIM]) / self.norms[slot];
            if best.is_none_or(|(_, b)| sim > b) {
                best = Some((slot, sim));
            }
        }
        best
    }

    fn similarity(&self, a: usize, b: usize) -> f32 {
        let (sa, sb) = (
            &self.sums[a * DIM..(a + 1) * DIM],
            &self.sums[b * DIM..(b + 1) * DIM],
        );
        dot(sa, sb) / (self.norms[a] * self.norms[b]).max(f32::EPSILON)
    }

    /// Merges groups that turned out to look alike, most alike first, and returns
    /// them as (kept person, merged person). Only groups changed since the last call
    /// are compared (with everyone).
    pub fn merges(&mut self) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        for _ in 0..8 {
            let touched: Vec<usize> = self.touched.drain().collect();
            let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
            for &a in &touched {
                if !self.alive[a] || self.counts[a] == 0 {
                    continue;
                }
                for b in 0..self.ids.len() {
                    if b == a
                        || !self.alive[b]
                        || self.counts[b] == 0
                        || (self.named[a] && self.named[b])
                        || (touched.contains(&b) && b < a)
                    {
                        continue;
                    }
                    let sim = self.similarity(a, b);
                    if sim >= JOIN {
                        pairs.push((sim, a, b));
                    }
                }
            }
            if pairs.is_empty() {
                break;
            }
            pairs.sort_by(|x, y| y.0.total_cmp(&x.0));
            let mut changed = HashSet::new();
            for (_, a, b) in pairs {
                // Each group merges once per round; the next round sees the result.
                if changed.contains(&a) || changed.contains(&b) || !self.alive[a] || !self.alive[b]
                {
                    continue;
                }
                let (keep, gone) =
                    if self.named[b] || (!self.named[a] && self.counts[b] > self.counts[a]) {
                        (b, a)
                    } else {
                        (a, b)
                    };
                let gone_sum: Vec<f32> = self.sums[gone * DIM..(gone + 1) * DIM].to_vec();
                let sum = &mut self.sums[keep * DIM..(keep + 1) * DIM];
                sum.iter_mut().zip(&gone_sum).for_each(|(s, x)| *s += x);
                self.norms[keep] = dot(sum, sum).sqrt();
                self.counts[keep] += self.counts[gone];
                self.alive[gone] = false;
                self.slots.remove(&self.ids[gone]);
                changed.insert(keep);
                changed.insert(gone);
                self.touched.insert(keep);
                out.push((self.ids[keep], self.ids[gone]));
            }
        }
        self.touched.clear();
        out
    }
}

/// Assigns each face of one photo to a person: the closest group if alike enough
/// (one face per person per photo), else a new person made by `create`.
pub fn assign(
    groups: &mut Groups,
    faces: &[Vec<f32>],
    create: &mut dyn FnMut() -> Result<i64>,
) -> Result<Vec<i64>> {
    // The surest matches choose first.
    let mut order: Vec<(usize, Option<(usize, f32)>)> = faces
        .iter()
        .enumerate()
        .map(|(i, v)| (i, groups.closest(v, &HashSet::new())))
        .collect();
    order.sort_by(|a, b| {
        let score = |x: &Option<(usize, f32)>| x.map_or(f32::MIN, |m| m.1);
        score(&b.1).total_cmp(&score(&a.1))
    });
    let mut used = HashSet::new();
    let mut people = vec![0; faces.len()];
    for (i, _) in order {
        let slot = match groups.closest(&faces[i], &used) {
            Some((slot, sim)) if sim >= JOIN => slot,
            _ => groups.create(create()?, false),
        };
        groups.add(slot, &faces[i]);
        used.insert(slot);
        people[i] = groups.id(slot);
    }
    Ok(people)
}

/// How many photos have been looked at for faces, out of how many.
pub fn progress(conn: &Connection) -> Result<Progress> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM assets WHERE media_kind != 1 AND status = 0",
        [],
        |r| r.get(0),
    )?;
    let indexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ai_face_scans s JOIN assets a ON a.id = s.asset_id
         WHERE s.model = ?1 AND a.media_kind != 1 AND a.status = 0",
        params![MODEL],
        |r| r.get(0),
    )?;
    Ok(Progress {
        indexed: indexed as u64,
        total: total as u64,
    })
}

/// What looking at one photo found: its size as read and its faces; None when it
/// couldn't be read.
type Looked = (i64, Option<((u32, u32), Vec<Found>)>);

/// A face found in a photo, ready to store.
struct Found {
    face: Face,
    vec: Vec<f32>,
    portrait: Vec<u8>,
}

fn find(faces: &Faces, image: &RgbImage) -> Result<Vec<Found>> {
    let mut found = Vec::new();
    for face in faces.detect(image)? {
        if face.w.min(face.h) < MIN_FACE {
            continue;
        }
        let vec = faces.describe(image, &face)?;
        let portrait = thumbs::encode_jpeg(&portrait(image, &face, PORTRAIT))?;
        found.push(Found {
            face,
            vec,
            portrait,
        });
        if found.len() == MAX_FACES {
            break;
        }
    }
    Ok(found)
}

/// People, and the faces waiting to be grouped among them.
#[derive(Default)]
pub struct People {
    /// Loaded when indexing starts; dropped whenever the user changes people, so the
    /// next batch starts from what's stored.
    groups: Mutex<Option<Groups>>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub hidden: bool,
    /// Visible photos they're in.
    pub photos: i64,
    /// The face shown for them.
    pub face_id: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl People {
    /// Call while holding the library's write lock, so indexing can't use the old
    /// groups in between.
    fn changed(&self) {
        *self.groups.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Finds the faces in up to `limit` photos not looked at yet, in batches, and
    /// groups them. `pause()` is checked between batches (scans, battery) and stops the
    /// pass when true, as does `cancel`. Calls `report` after each batch. Returns how
    /// many photos were looked at.
    #[allow(clippy::too_many_arguments)]
    pub fn index_pending(
        &self,
        library: &Db,
        thumbs_db: &Db,
        faces: &Faces,
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
                       AND NOT EXISTS (SELECT 1 FROM ai_face_scans s WHERE s.asset_id = a.id AND s.model = ?2)
                     ORDER BY a.id LIMIT ?3",
                )?;
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
            // (photo, its size, faces) for the readable ones; None if unreadable.
            let mut results: Vec<Looked> = Vec::new();
            for (id, picture) in pictures {
                match picture {
                    Picture::Ready(image) => {
                        results.push((id, Some((image.dimensions(), find(faces, &image)?))))
                    }
                    Picture::Failed => results.push((id, None)),
                    Picture::Later => {}
                }
            }
            let mut portraits = Vec::new();
            {
                let mut conn = library.write();
                let mut slot = self.groups.lock().unwrap_or_else(|e| e.into_inner());
                let stored = (|| -> Result<usize> {
                    let tx = conn.transaction()?;
                    if slot.is_none() {
                        *slot = Some(Groups::load(&tx)?);
                    }
                    let groups = slot.as_mut().expect("loaded");
                    let mut stored = 0;
                    for (id, result) in &results {
                        tx.execute("DELETE FROM ai_faces WHERE asset_id = ?1", params![id])?;
                        let count = result.as_ref().map_or(-1, |(_, f)| f.len() as i64);
                        tx.execute(
                            "INSERT INTO ai_face_scans (asset_id, model, faces) VALUES (?1, ?2, ?3)
                             ON CONFLICT (asset_id) DO UPDATE SET model = excluded.model, faces = excluded.faces",
                            params![id, MODEL, count],
                        )?;
                        stored += 1;
                        let Some(((w, h), found)) = result else {
                            continue;
                        };
                        let vecs: Vec<Vec<f32>> = found.iter().map(|f| f.vec.clone()).collect();
                        let people = assign(groups, &vecs, &mut || {
                            tx.execute(
                                "INSERT INTO ai_people (created_at) VALUES (?1)",
                                params![now()],
                            )?;
                            Ok(tx.last_insert_rowid())
                        })?;
                        let (w, h) = (*w as f32, *h as f32);
                        for (f, person) in found.iter().zip(people) {
                            let (scale, bytes) = quantize(&f.vec);
                            tx.execute(
                                "INSERT INTO ai_faces (asset_id, x, y, w, h, quality, scale, vec, person_id)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                                params![
                                    id,
                                    f.face.x / w,
                                    f.face.y / h,
                                    f.face.w / w,
                                    f.face.h / h,
                                    f.face.quality(),
                                    scale,
                                    bytes,
                                    person
                                ],
                            )?;
                            portraits.push((tx.last_insert_rowid(), f.portrait.clone()));
                        }
                    }
                    tx.commit()?;
                    Ok(stored)
                })();
                match stored {
                    Ok(n) => done += n,
                    Err(e) => {
                        // The groups took faces that weren't stored: start again from the library.
                        *slot = None;
                        return Err(e);
                    }
                }
            }
            {
                let conn = thumbs_db.write();
                for (face_id, jpeg) in &portraits {
                    thumbs::put_face(&conn, *face_id, jpeg)?;
                }
            }
            report();
        }
        if done > 0 {
            self.tidy(library)?;
        }
        Ok(done)
    }

    /// Merges people who turned out to look alike and drops empty unnamed ones.
    pub fn tidy(&self, library: &Db) -> Result<()> {
        let mut conn = library.write();
        let mut slot = self.groups.lock().unwrap_or_else(|e| e.into_inner());
        let result = (|| -> Result<()> {
            let tx = conn.transaction()?;
            if slot.is_none() {
                *slot = Some(Groups::load(&tx)?);
            }
            let groups = slot.as_mut().expect("loaded");
            for (keep, gone) in groups.merges() {
                merge_rows(&tx, keep, gone)?;
            }
            let emptied = tx.execute(
                "DELETE FROM ai_people WHERE name IS NULL
                   AND NOT EXISTS (SELECT 1 FROM ai_faces f WHERE f.person_id = ai_people.id)",
                [],
            )?;
            tx.commit()?;
            // People whose photos changed or went: start again from the library.
            if emptied > 0 {
                *slot = None;
            }
            Ok(())
        })();
        if result.is_err() {
            *slot = None;
        }
        result
    }

    /// Everyone with at least `MIN_PHOTOS` visible photos, and everyone named; named
    /// first, then by number of photos.
    pub fn list(&self, conn: &Connection, hidden: bool) -> Result<Vec<Person>> {
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.hidden, COUNT(DISTINCT f.asset_id) AS photos,
                    (SELECT f2.id FROM ai_faces f2 JOIN assets a2 ON a2.id = f2.asset_id
                     WHERE f2.person_id = p.id AND a2.status = 0
                     ORDER BY f2.id = p.cover_face DESC, f2.quality DESC LIMIT 1)
             FROM ai_people p
             JOIN ai_faces f ON f.person_id = p.id
             JOIN assets a ON a.id = f.asset_id AND a.status = 0
             LEFT JOIN user_meta u ON u.asset_id = a.id
             WHERE COALESCE(u.hidden, 0) = 0 AND p.hidden = ?1
             GROUP BY p.id
             HAVING photos >= ?2 OR p.name IS NOT NULL
             ORDER BY p.name IS NULL, photos DESC, p.id",
        )?;
        let rows = stmt.query_map(params![hidden, MIN_PHOTOS], |r| {
            Ok(Person {
                id: r.get(0)?,
                name: r.get(1)?,
                hidden: r.get(2)?,
                photos: r.get(3)?,
                face_id: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get(&self, conn: &Connection, id: i64) -> Result<Option<Person>> {
        Ok(self
            .list(conn, false)?
            .into_iter()
            .chain(self.list(conn, true)?)
            .find(|p| p.id == id))
    }

    /// Names a person (empty takes the name away).
    pub fn rename(&self, library: &Db, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        let conn = library.write();
        conn.execute(
            "UPDATE ai_people SET name = ?2 WHERE id = ?1",
            params![id, (!name.is_empty()).then_some(name)],
        )?;
        self.changed();
        Ok(())
    }

    /// Merges `others` into `into`, keeping a name if `into` has none.
    pub fn merge(&self, library: &Db, into: i64, others: &[i64]) -> Result<()> {
        let mut conn = library.write();
        let tx = conn.transaction()?;
        for &other in others {
            if other != into {
                merge_rows(&tx, into, other)?;
            }
        }
        tx.commit()?;
        self.changed();
        Ok(())
    }

    pub fn set_hidden(&self, library: &Db, id: i64, hidden: bool) -> Result<()> {
        library.write().execute(
            "UPDATE ai_people SET hidden = ?2 WHERE id = ?1",
            params![id, hidden],
        )?;
        Ok(())
    }

    /// Takes these photos out of a person: their faces of that person are never
    /// grouped again. Returns how many faces.
    pub fn remove_photos(&self, library: &Db, id: i64, asset_ids: &[i64]) -> Result<usize> {
        let mut conn = library.write();
        let tx = conn.transaction()?;
        let mut n = 0;
        for chunk in asset_ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "UPDATE ai_faces SET person_id = NULL, rejected = 1
                 WHERE person_id = {id} AND asset_id IN ({marks})"
            );
            n += tx.execute(&sql, params_from_iter(chunk))?;
        }
        tx.execute(
            "UPDATE ai_people SET cover_face = NULL WHERE id = ?1
               AND cover_face NOT IN (SELECT id FROM ai_faces WHERE person_id = ?1)",
            params![id],
        )?;
        tx.commit()?;
        self.changed();
        Ok(n)
    }

    /// Uses this photo's face of the person as their picture.
    pub fn set_cover(&self, library: &Db, id: i64, asset_id: i64) -> Result<()> {
        let conn = library.write();
        let face: Option<i64> = conn
            .query_row(
                "SELECT id FROM ai_faces WHERE person_id = ?1 AND asset_id = ?2
                 ORDER BY quality DESC LIMIT 1",
                params![id, asset_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(face) = face {
            conn.execute(
                "UPDATE ai_people SET cover_face = ?2 WHERE id = ?1",
                params![id, face],
            )?;
        }
        Ok(())
    }

    /// The named people in a photo.
    pub fn in_photo(&self, conn: &Connection, asset_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT p.id, p.name FROM ai_faces f JOIN ai_people p ON p.id = f.person_id
             WHERE f.asset_id = ?1 AND p.name IS NOT NULL ORDER BY f.x",
        )?;
        let rows = stmt.query_map(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Forgets every face and person (the feature is being removed).
    pub fn clear(&self, library: &Db, thumbs_db: &Db) -> Result<()> {
        {
            let mut conn = library.write();
            let tx = conn.transaction()?;
            tx.execute_batch(
                "DELETE FROM ai_faces; DELETE FROM ai_face_scans; DELETE FROM ai_people;",
            )?;
            tx.commit()?;
            self.changed();
        }
        thumbs::clear_faces(&thumbs_db.write())?;
        Ok(())
    }
}

/// Moves `gone`'s faces to `keep` and deletes `gone`, carrying its name and hidden
/// state over when `keep` has none.
fn merge_rows(conn: &Connection, keep: i64, gone: i64) -> Result<()> {
    conn.execute(
        "UPDATE ai_people SET
             name = COALESCE(name, (SELECT name FROM ai_people WHERE id = ?2)),
             cover_face = COALESCE(cover_face, (SELECT cover_face FROM ai_people WHERE id = ?2))
         WHERE id = ?1",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE ai_faces SET person_id = ?1 WHERE person_id = ?2",
        params![keep, gone],
    )?;
    conn.execute("DELETE FROM ai_people WHERE id = ?1", params![gone])?;
    Ok(())
}

/// A face's portrait made again from its photo (when the cache lost it).
pub fn portrait_from_photo(library: &Db, face_id: i64) -> Result<Option<Vec<u8>>> {
    let face: Option<(i64, f32, f32, f32, f32)> = library
        .read()
        .query_row(
            "SELECT asset_id, x, y, w, h FROM ai_faces WHERE id = ?1",
            params![face_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((asset_id, x, y, w, h)) = face else {
        return Ok(None);
    };
    let Picture::Ready(image) = picture(library, asset_id, READ_EDGE, MAX_EDGE) else {
        return Ok(None);
    };
    let (iw, ih) = (image.width() as f32, image.height() as f32);
    let face = Face {
        x: x * iw,
        y: y * ih,
        w: w * iw,
        h: h * ih,
        landmarks: [[0.0; 2]; 5],
        score: 0.0,
    };
    Ok(Some(thumbs::encode_jpeg(&portrait(
        &image, &face, PORTRAIT,
    ))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector mostly along `axis`, nudged by `noise` along another.
    fn face(axis: usize, noise: f32, other: usize) -> Vec<f32> {
        let mut v = vec![0f32; DIM];
        v[axis] = 1.0;
        v[other] += noise;
        let n = dot(&v, &v).sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    #[test]
    fn faces_alike_join_one_person_and_others_start_their_own() {
        let mut groups = Groups::default();
        let mut next = 100;
        let mut create = || {
            next += 1;
            Ok(next)
        };
        let a1 = assign(&mut groups, &[face(0, 0.2, 5)], &mut create).unwrap();
        let a2 = assign(&mut groups, &[face(0, 0.3, 6)], &mut create).unwrap();
        let b = assign(&mut groups, &[face(1, 0.2, 5)], &mut create).unwrap();
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        // Two faces alike in one photo are still two people.
        let both = assign(
            &mut groups,
            &[face(0, 0.1, 7), face(0, 0.2, 8)],
            &mut create,
        )
        .unwrap();
        assert_ne!(both[0], both[1]);
        assert!(both.contains(&a1[0]));
    }

    #[test]
    fn people_who_look_alike_are_merged_but_named_ones_are_kept_apart() {
        let mut groups = Groups::default();
        // Two groups of the same person (made apart, as if before more faces came).
        let a = groups.create(1, false);
        groups.add(a, &face(0, 0.2, 5));
        let b = groups.create(2, false);
        groups.add(b, &face(0, 0.25, 6));
        groups.add(b, &face(0, 0.1, 7));
        let c = groups.create(3, false);
        groups.add(c, &face(9, 0.1, 7));
        let merged = groups.merges();
        assert_eq!(merged, vec![(2, 1)], "the bigger group stays");
        assert!(groups.merges().is_empty());

        let mut groups = Groups::default();
        for (id, noise) in [(1, 0.2), (2, 0.25)] {
            let s = groups.create(id, true);
            groups.add(s, &face(0, noise, 5));
        }
        assert!(groups.merges().is_empty(), "two named people stay apart");
    }
}
