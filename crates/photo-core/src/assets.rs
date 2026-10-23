//! Per-asset details and user-authored data (favorite, rating, hidden).

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::collections::{self, Named};
use crate::places::{PLACE_COLUMNS, Place, place_row};
use crate::{Error, Result};

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub modified_at: i64,
    /// The file's drive is connected and the file exists right now.
    pub online: bool,
    /// A complete scan found the file gone.
    pub missing: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AssetDetails {
    pub id: i64,
    pub media_kind: u8,
    pub taken_at: i64,
    pub taken_src: u8,
    pub tz_offset_min: i32,
    pub local_day: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u8>,
    pub duration_ms: Option<i64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// The town nearest the GPS position, once places have been looked up.
    pub place: Option<Place>,
    pub favorite: bool,
    pub rating: Option<u8>,
    pub hidden: bool,
    pub files: Vec<FileInfo>,
    pub albums: Vec<Named>,
    pub tags: Vec<Named>,
    /// The clip of a Live Photo.
    pub live_clip_id: Option<i64>,
    pub event: Option<crate::events::EventSummary>,
    pub trip: Option<crate::trips::TripSummary>,
}

pub fn details(conn: &Connection, id: i64) -> Result<Option<AssetDetails>> {
    let asset = conn
        .query_row(
            &format!(
                "SELECT a.media_kind, a.taken_at, a.taken_src, a.tz_offset_min, a.local_day, a.width,
                        a.height, a.orientation, a.duration_ms, a.camera_make, a.camera_model, a.lat,
                        a.lon, COALESCE(u.favorite, 0), u.rating, COALESCE(u.hidden, 0), {PLACE_COLUMNS}
                 FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id
                 LEFT JOIN places p ON p.id = a.place_id WHERE a.id = ?1"
            ),
            params![id],
            |r| {
                Ok(AssetDetails {
                    id,
                    media_kind: r.get(0)?,
                    taken_at: r.get(1)?,
                    taken_src: r.get(2)?,
                    tz_offset_min: r.get(3)?,
                    local_day: r.get(4)?,
                    width: r.get(5)?,
                    height: r.get(6)?,
                    orientation: r.get(7)?,
                    duration_ms: r.get(8)?,
                    camera_make: r.get(9)?,
                    camera_model: r.get(10)?,
                    lat: r.get(11)?,
                    lon: r.get(12)?,
                    place: place_row(r, 16)?,
                    favorite: r.get(13)?,
                    rating: r.get(14)?,
                    hidden: r.get(15)?,
                    files: Vec::new(),
                    albums: Vec::new(),
                    tags: Vec::new(),
                    live_clip_id: None,
                    event: None,
                    trip: None,
                })
            },
        )
        .optional()?;
    let Some(mut asset) = asset else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT f.id, v.last_mount_path, r.rel_path, f.rel_path, f.size, f.mtime,
                f.missing_since IS NOT NULL, v.status = 'online'
         FROM files f JOIN roots r ON r.id = f.root_id JOIN volumes v ON v.id = r.volume_id
         WHERE f.asset_id = ?1 ORDER BY f.missing_since IS NOT NULL, f.id",
    )?;
    let rows = stmt.query_map(params![id], |r| {
        let mount: Option<String> = r.get(1)?;
        let path = PathBuf::from(mount.unwrap_or_default())
            .join(r.get::<_, String>(2)?)
            .join(r.get::<_, String>(3)?);
        let volume_online: bool = r.get(7)?;
        Ok(FileInfo {
            id: r.get(0)?,
            online: volume_online && path.is_file(),
            path: path.to_string_lossy().into_owned(),
            size: r.get(4)?,
            modified_at: r.get(5)?,
            missing: r.get(6)?,
        })
    })?;
    asset.files = rows.collect::<rusqlite::Result<_>>()?;
    asset.albums = collections::albums_of(conn, id)?;
    asset.tags = collections::tags_of(conn, id)?;
    asset.live_clip_id = crate::live::clip_of(conn, id)?;
    asset.event = crate::events::of_asset(conn, id)?;
    asset.trip = crate::trips::of_day(conn, asset.local_day)?;
    Ok(Some(asset))
}

/// Applies one user_meta column to many assets in a single transaction.
fn set_meta(conn: &mut Connection, column: &str, ids: &[i64], value: Option<i64>) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(&format!(
            "INSERT INTO user_meta (asset_id, {column}) VALUES (?1, ?2)
             ON CONFLICT (asset_id) DO UPDATE SET {column} = excluded.{column}"
        ))?;
        for id in ids {
            stmt.execute(params![id, value])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn set_favorite(conn: &mut Connection, ids: &[i64], favorite: bool) -> Result<()> {
    set_meta(conn, "favorite", ids, Some(favorite as i64))
}

pub fn set_rating(conn: &mut Connection, ids: &[i64], rating: Option<u8>) -> Result<()> {
    if rating.is_some_and(|r| !(1..=5).contains(&r)) {
        return Err(Error::Invalid("Rating must be between 1 and 5".into()));
    }
    set_meta(conn, "rating", ids, rating.map(i64::from))
}

/// Hidden assets leave the main timeline but stay in the library and the Hidden view.
/// Day counts are adjusted only for the days that change, not rebuilt.
pub fn set_hidden(conn: &mut Connection, ids: &[i64], hidden: bool) -> Result<()> {
    let tx = conn.transaction()?;
    // Only timeline-visible assets whose state actually flips move a day count.
    let mut deltas: HashMap<i64, i64> = HashMap::new();
    {
        let mut day_of = tx.prepare(
            "SELECT a.local_day FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id
             WHERE a.id = ?1 AND a.status = 0 AND a.paired_with IS NULL
               AND COALESCE(u.hidden, 0) != ?2",
        )?;
        let mut set = tx.prepare(
            "INSERT INTO user_meta (asset_id, hidden) VALUES (?1, ?2)
             ON CONFLICT (asset_id) DO UPDATE SET hidden = excluded.hidden",
        )?;
        for &id in ids {
            if let Some(day) = day_of
                .query_row(params![id, hidden], |r| r.get::<_, i64>(0))
                .optional()?
            {
                *deltas.entry(day).or_default() += if hidden { -1 } else { 1 };
            }
            set.execute(params![id, hidden])?;
        }
        let mut bump = tx.prepare(
            "INSERT INTO day_buckets (day, count) VALUES (?1, ?2)
             ON CONFLICT (day) DO UPDATE SET count = count + excluded.count",
        )?;
        for (day, delta) in &deltas {
            bump.execute(params![day, delta])?;
        }
    }
    tx.execute("DELETE FROM day_buckets WHERE count <= 0", [])?;
    tx.commit()?;
    Ok(())
}
