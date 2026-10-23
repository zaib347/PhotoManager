//! Albums and tags. Both are database collections that point at assets: files are
//! never copied or moved, and one asset can be in any number of albums.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{Error, Result, now_ms};

const MAX_NAME: usize = 200;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AlbumInfo {
    pub id: i64,
    pub name: String,
    pub count: i64,
    /// Explicit cover, or the newest item when none is set.
    pub cover_asset_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

/// A name/id pair for showing which albums or tags an asset belongs to.
#[derive(Serialize, Debug, PartialEq)]
pub struct Named {
    pub id: i64,
    pub name: String,
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("Name can't be empty".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(Error::Invalid(format!(
            "Name can be at most {MAX_NAME} characters"
        )));
    }
    Ok(name.to_owned())
}

// ---------- Albums ----------

pub fn list_albums(conn: &Connection) -> Result<Vec<AlbumInfo>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.name, al.created_at, COALESCE(al.updated_at, al.created_at),
                (SELECT count(*) FROM album_assets aa JOIN assets a ON a.id = aa.asset_id
                 WHERE aa.album_id = al.id AND a.status = 0),
                COALESCE(al.cover_asset_id,
                    (SELECT a.id FROM album_assets aa JOIN assets a ON a.id = aa.asset_id
                     WHERE aa.album_id = al.id AND a.status = 0
                     ORDER BY a.taken_at DESC LIMIT 1))
         FROM albums al ORDER BY COALESCE(al.updated_at, al.created_at) DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(AlbumInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            created_at: r.get(2)?,
            updated_at: r.get(3)?,
            count: r.get(4)?,
            cover_asset_id: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn create_album(conn: &mut Connection, name: &str, asset_ids: &[i64]) -> Result<i64> {
    let name = clean_name(name)?;
    let tx = conn.transaction()?;
    let now = now_ms();
    tx.execute(
        "INSERT INTO albums (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
        params![name, now],
    )?;
    let id = tx.last_insert_rowid();
    insert_album_assets(&tx, id, asset_ids)?;
    tx.commit()?;
    Ok(id)
}

pub fn rename_album(conn: &Connection, id: i64, name: &str) -> Result<()> {
    let name = clean_name(name)?;
    conn.execute(
        "UPDATE albums SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, name, now_ms()],
    )?;
    Ok(())
}

pub fn delete_album(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM albums WHERE id = ?1", params![id])?;
    Ok(())
}

fn insert_album_assets(conn: &Connection, album_id: i64, asset_ids: &[i64]) -> Result<usize> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO album_assets (album_id, asset_id) VALUES (?1, ?2)",
    )?;
    let mut added = 0;
    for id in asset_ids {
        added += stmt.execute(params![album_id, id])?;
    }
    Ok(added)
}

/// Returns how many were newly added (items already in the album are skipped).
pub fn add_to_album(conn: &mut Connection, album_id: i64, asset_ids: &[i64]) -> Result<usize> {
    let tx = conn.transaction()?;
    let exists: Option<i64> = tx
        .query_row(
            "SELECT id FROM albums WHERE id = ?1",
            params![album_id],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(Error::Invalid("That album no longer exists".into()));
    }
    let added = insert_album_assets(&tx, album_id, asset_ids)?;
    tx.execute(
        "UPDATE albums SET updated_at = ?2 WHERE id = ?1",
        params![album_id, now_ms()],
    )?;
    tx.commit()?;
    Ok(added)
}

pub fn remove_from_album(conn: &mut Connection, album_id: i64, asset_ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt =
            tx.prepare("DELETE FROM album_assets WHERE album_id = ?1 AND asset_id = ?2")?;
        for id in asset_ids {
            stmt.execute(params![album_id, id])?;
        }
    }
    tx.execute(
        "UPDATE albums SET updated_at = ?2,
             cover_asset_id = CASE WHEN cover_asset_id IN (SELECT asset_id FROM album_assets WHERE album_id = ?1)
                                   THEN cover_asset_id END
         WHERE id = ?1",
        params![album_id, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn set_album_cover(conn: &Connection, album_id: i64, asset_id: i64) -> Result<()> {
    let changed = conn.execute(
        "UPDATE albums SET cover_asset_id = ?2 WHERE id = ?1
           AND EXISTS (SELECT 1 FROM album_assets WHERE album_id = ?1 AND asset_id = ?2)",
        params![album_id, asset_id],
    )?;
    if changed == 0 {
        return Err(Error::Invalid(
            "The cover must be a photo in the album".into(),
        ));
    }
    Ok(())
}

pub fn albums_of(conn: &Connection, asset_id: i64) -> Result<Vec<Named>> {
    let mut stmt = conn.prepare_cached(
        "SELECT al.id, al.name FROM album_assets aa JOIN albums al ON al.id = aa.album_id
         WHERE aa.asset_id = ?1 ORDER BY al.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![asset_id], |r| {
        Ok(Named {
            id: r.get(0)?,
            name: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- Tags ----------

pub fn list_tags(conn: &Connection) -> Result<Vec<TagInfo>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name,
                (SELECT count(*) FROM asset_tags at JOIN assets a ON a.id = at.asset_id
                 WHERE at.tag_id = t.id AND a.status = 0)
         FROM tags t ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            count: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Tags assets with `name`, creating the tag if needed (names are case-insensitive).
pub fn add_tag(conn: &mut Connection, asset_ids: &[i64], name: &str) -> Result<i64> {
    let name = clean_name(name)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
        params![name],
    )?;
    let tag_id: i64 = tx.query_row(
        "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
        params![name],
        |r| r.get(0),
    )?;
    {
        let mut stmt =
            tx.prepare("INSERT OR IGNORE INTO asset_tags (asset_id, tag_id) VALUES (?1, ?2)")?;
        for id in asset_ids {
            stmt.execute(params![id, tag_id])?;
        }
    }
    tx.commit()?;
    Ok(tag_id)
}

/// Untags assets; a tag left with no assets is deleted.
pub fn remove_tag(conn: &mut Connection, asset_ids: &[i64], tag_id: i64) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare("DELETE FROM asset_tags WHERE asset_id = ?1 AND tag_id = ?2")?;
        for id in asset_ids {
            stmt.execute(params![id, tag_id])?;
        }
    }
    tx.execute(
        "DELETE FROM tags WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM asset_tags WHERE tag_id = ?1)",
        params![tag_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn rename_tag(conn: &Connection, tag_id: i64, name: &str) -> Result<()> {
    let name = clean_name(name)?;
    let clash: Option<i64> = conn
        .query_row(
            "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE AND id != ?2",
            params![name, tag_id],
            |r| r.get(0),
        )
        .optional()?;
    if clash.is_some() {
        return Err(Error::Invalid(format!(
            "A tag named \"{name}\" already exists"
        )));
    }
    conn.execute(
        "UPDATE tags SET name = ?2 WHERE id = ?1",
        params![tag_id, name],
    )?;
    Ok(())
}

pub fn delete_tag(conn: &Connection, tag_id: i64) -> Result<()> {
    conn.execute("DELETE FROM tags WHERE id = ?1", params![tag_id])?;
    Ok(())
}

pub fn tags_of(conn: &Connection, asset_id: i64) -> Result<Vec<Named>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id, t.name FROM asset_tags at JOIN tags t ON t.id = at.tag_id
         WHERE at.asset_id = ?1 ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![asset_id], |r| {
        Ok(Named {
            id: r.get(0)?,
            name: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
