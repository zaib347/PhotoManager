//! Per-asset details and user-authored data (favorite, rating, hidden).

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::collections::{self, Named};
use crate::{Error, Result, timeline};

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
    pub favorite: bool,
    pub rating: Option<u8>,
    pub hidden: bool,
    pub files: Vec<FileInfo>,
    pub albums: Vec<Named>,
    pub tags: Vec<Named>,
}

pub fn details(conn: &Connection, id: i64) -> Result<Option<AssetDetails>> {
    let asset = conn
        .query_row(
            "SELECT a.media_kind, a.taken_at, a.taken_src, a.tz_offset_min, a.local_day, a.width,
                    a.height, a.orientation, a.duration_ms, a.camera_make, a.camera_model, a.lat,
                    a.lon, COALESCE(u.favorite, 0), u.rating, COALESCE(u.hidden, 0)
             FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id WHERE a.id = ?1",
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
                    favorite: r.get(13)?,
                    rating: r.get(14)?,
                    hidden: r.get(15)?,
                    files: Vec::new(),
                    albums: Vec::new(),
                    tags: Vec::new(),
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
pub fn set_hidden(conn: &mut Connection, ids: &[i64], hidden: bool) -> Result<()> {
    set_meta(conn, "hidden", ids, Some(hidden as i64))?;
    timeline::rebuild_day_buckets(conn)
}
