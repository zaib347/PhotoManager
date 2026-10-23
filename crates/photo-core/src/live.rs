//! Live Photos: an iPhone still plus a ~3 s clip saved side by side with the same
//! name (`IMG_1234.HEIC` + `IMG_1234.MOV`). The clip is linked to the photo
//! (`assets.paired_with`) and leaves the timeline; the photo becomes `media_kind = 2`.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::Result;

/// Live Photo clips are about 3 s; anything longer is a real video that happens to
/// share a name.
const MAX_CLIP_MS: i64 = 4_500;
const PHOTO_EXTS: &[&str] = &["heic", "heif", "jpg", "jpeg"];

pub const KIND_LIVE: u8 = 2;

/// Splits `dir/IMG_1234.MOV` into (`dir/IMG_1234.`, `mov`).
fn split_ext(rel: &str) -> Option<(&str, String)> {
    let dot = rel.rfind('.')?;
    if rel[dot..].contains('/') {
        return None;
    }
    Some((&rel[..=dot], rel[dot + 1..].to_ascii_lowercase()))
}

/// Pairs unpaired short clips under `root_id` with a same-named photo in the same
/// folder, and releases clips whose photo is gone. Returns the number of new pairs.
pub fn pair(conn: &mut Connection, root_id: i64) -> Result<usize> {
    let tx = conn.transaction()?;

    // A clip whose photo was deleted becomes an ordinary video again.
    tx.execute(
        "UPDATE assets SET paired_with = NULL
         WHERE paired_with IN (SELECT id FROM assets WHERE status = 1)",
        [],
    )?;
    tx.execute(
        "UPDATE assets SET media_kind = 0
         WHERE media_kind = 2 AND NOT EXISTS (SELECT 1 FROM assets c WHERE c.paired_with = assets.id)",
        [],
    )?;

    let clips: Vec<(i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT a.id, f.rel_path FROM files f JOIN assets a ON a.id = f.asset_id
             WHERE f.root_id = ?1 AND f.missing_since IS NULL AND a.media_kind = 1
               AND a.paired_with IS NULL AND a.status = 0
               AND a.duration_ms IS NOT NULL AND a.duration_ms <= ?2",
        )?;
        stmt.query_map(params![root_id, MAX_CLIP_MS], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };

    let mut paired = 0;
    {
        // Range scan on the (root_id, rel_path) index: every file starting with "dir/IMG_1234."
        let mut photos = tx.prepare(
            "SELECT f.rel_path, a.id FROM files f JOIN assets a ON a.id = f.asset_id
             WHERE f.root_id = ?1 AND f.rel_path >= ?2 AND f.rel_path < ?3
               AND f.missing_since IS NULL AND a.media_kind IN (0, 2) AND a.status = 0",
        )?;
        let mut link = tx.prepare("UPDATE assets SET paired_with = ?2 WHERE id = ?1")?;
        let mut mark = tx.prepare("UPDATE assets SET media_kind = 2 WHERE id = ?1")?;
        let mut claimed: HashMap<i64, i64> = HashMap::new();

        for (clip_id, rel) in clips {
            let Some((stem, _)) = split_ext(&rel) else {
                continue;
            };
            // '.' + 1 is '/', so this bounds names that start with exactly `stem`.
            let upper = format!("{}/", &stem[..stem.len() - 1]);
            let candidates: Vec<(String, i64)> = photos
                .query_map(params![root_id, stem, upper], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let photo = candidates.into_iter().find(|(path, _)| {
                split_ext(path)
                    .is_some_and(|(s, ext)| s == stem && PHOTO_EXTS.contains(&ext.as_str()))
            });
            if let Some((_, photo_id)) = photo
                && !claimed.contains_key(&photo_id)
            {
                claimed.insert(photo_id, clip_id);
                link.execute(params![clip_id, photo_id])?;
                mark.execute(params![photo_id])?;
                paired += 1;
            }
        }
    }
    tx.commit()?;
    Ok(paired)
}

/// The clip of a Live Photo, if it has one.
pub fn clip_of(conn: &Connection, photo_id: i64) -> Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .prepare_cached("SELECT id FROM assets WHERE paired_with = ?1 LIMIT 1")?
        .query_row(params![photo_id], |r| r.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::split_ext;

    #[test]
    fn splits_extensions() {
        assert_eq!(
            split_ext("a/IMG_1.MOV"),
            Some(("a/IMG_1.", "mov".to_owned()))
        );
        assert_eq!(split_ext("IMG_1.heic"), Some(("IMG_1.", "heic".to_owned())));
        assert_eq!(split_ext("dir.v2/noext"), None);
    }
}
