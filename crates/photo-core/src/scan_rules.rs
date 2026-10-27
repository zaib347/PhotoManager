//! Which new files a scan adds: rules for the whole library (small images, file types,
//! short videos) and folders left out of each library folder.
//!
//! Rules only decide about files the library doesn't have yet. Files already in it stay,
//! and files in an excluded folder keep their rows instead of turning up as missing.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::media::MediaKind;
use crate::{Error, Result, library};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Rules {
    /// Photos smaller than this many bytes are skipped (0: off).
    pub min_bytes: u64,
    /// Photos whose longest side is shorter than this many pixels are skipped (0: off).
    pub min_side: u32,
    pub png: bool,
    pub gif: bool,
    pub raw: bool,
    pub videos: bool,
    /// Videos shorter than this are skipped (0: off).
    pub min_video_ms: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            min_bytes: 0,
            min_side: 0,
            png: true,
            gif: true,
            raw: true,
            videos: true,
            min_video_ms: 0,
        }
    }
}

impl Rules {
    /// Whether a new file is skipped, from what the folder walk knows (type and size).
    pub fn skips_file(&self, path: &Path, kind: MediaKind, size: u64) -> bool {
        match kind {
            MediaKind::Video => !self.videos,
            MediaKind::Photo => {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                (ext == "png" && !self.png)
                    || (ext == "gif" && !self.gif)
                    || (!self.raw && crate::raw::is_raw(path))
                    || size < self.min_bytes
            }
        }
    }

    /// Whether a new file is skipped, once its pixel size or length has been read.
    /// Unknown sizes and lengths never skip.
    pub fn skips_media(
        &self,
        kind: MediaKind,
        width: Option<u32>,
        height: Option<u32>,
        duration_ms: Option<i64>,
    ) -> bool {
        match kind {
            MediaKind::Photo => match (width, height) {
                (Some(w), Some(h)) => w.max(h) < self.min_side,
                _ => false,
            },
            MediaKind::Video => duration_ms.is_some_and(|d| d < self.min_video_ms as i64),
        }
    }
}

const KEY: &str = "scan_rules";

pub fn rules(conn: &Connection) -> Result<Rules> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default())
}

pub fn set_rules(conn: &Connection, rules: &Rules) -> Result<()> {
    let json = serde_json::to_string(rules).map_err(|e| Error::Invalid(e.to_string()))?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![KEY, json],
    )?;
    Ok(())
}

/// Folders left out of a library folder, relative to it ("a/b").
pub fn excluded(conn: &Connection, root_id: i64) -> Result<Vec<String>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT excluded_globs FROM roots WHERE id = ?1",
            params![root_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default())
}

/// The same, as full paths, while the folder's drive is connected.
pub fn excluded_paths(conn: &Connection, root_id: i64) -> Result<Vec<PathBuf>> {
    let Some(root) = library::root_path(conn, root_id)? else {
        return Ok(Vec::new());
    };
    Ok(excluded(conn, root_id)?
        .iter()
        .map(|rel| rel.split('/').fold(root.clone(), |p, part| p.join(part)))
        .collect())
}

/// Replaces the folders left out of a library folder. Each must be inside it.
pub fn set_excluded(conn: &Connection, root_id: i64, folders: &[PathBuf]) -> Result<()> {
    let root = library::root_path(conn, root_id)?
        .ok_or_else(|| Error::Invalid("This folder's drive is not connected".into()))?;
    let mut rels = Vec::new();
    for folder in folders {
        let rel = relative(&root, folder).ok_or_else(|| {
            Error::Invalid(format!(
                "{} isn't inside {}",
                folder.display(),
                root.display()
            ))
        })?;
        if !rels.contains(&rel) {
            rels.push(rel);
        }
    }
    rels.sort();
    let json = serde_json::to_string(&rels).map_err(|e| Error::Invalid(e.to_string()))?;
    conn.execute(
        "UPDATE roots SET excluded_globs = ?2 WHERE id = ?1",
        params![root_id, json],
    )?;
    Ok(())
}

/// `folder` relative to `root` with "/" between parts; `None` unless strictly inside.
/// Windows paths compare without regard to case.
fn relative(root: &Path, folder: &Path) -> Option<String> {
    let parts = |p: &Path| -> Option<Vec<String>> {
        p.components()
            .map(|c| c.as_os_str().to_str().map(str::to_owned))
            .collect()
    };
    let (root, folder) = (parts(root)?, parts(folder)?);
    let same = |a: &String, b: &String| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if folder.len() <= root.len() || !root.iter().zip(&folder).all(|(a, b)| same(a, b)) {
        return None;
    }
    Some(folder[root.len()..].join("/"))
}

/// Whether `rel` (a path relative to the library folder) is one of `excluded` or inside one.
pub fn is_excluded(excluded: &[String], rel: &str) -> bool {
    excluded.iter().any(|x| {
        rel.len() >= x.len()
            && rel[..x.len()].eq_ignore_ascii_case(x)
            && (rel.len() == x.len() || rel.as_bytes()[x.len()] == b'/')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_and_unwanted_files_are_skipped() {
        let rules = Rules {
            min_bytes: 50_000,
            min_side: 400,
            gif: false,
            min_video_ms: 3000,
            ..Default::default()
        };
        let photo = MediaKind::Photo;
        assert!(rules.skips_file(Path::new("a.jpg"), photo, 49_999));
        assert!(!rules.skips_file(Path::new("a.jpg"), photo, 50_000));
        assert!(rules.skips_file(Path::new("a.GIF"), photo, 900_000));
        assert!(!rules.skips_file(Path::new("a.mp4"), MediaKind::Video, 10));
        assert!(rules.skips_media(photo, Some(399), Some(300), None));
        assert!(!rules.skips_media(photo, Some(300), Some(400), None));
        assert!(!rules.skips_media(photo, None, None, None));
        assert!(rules.skips_media(MediaKind::Video, None, None, Some(2999)));
        assert!(!rules.skips_media(MediaKind::Video, None, None, None));

        let off = Rules::default();
        assert!(!off.skips_file(Path::new("a.gif"), photo, 1));
        assert!(!off.skips_media(photo, Some(1), Some(1), Some(1)));
    }

    #[test]
    fn excluded_folders_cover_what_is_inside() {
        let excluded = vec!["WhatsApp/Stickers".to_owned()];
        assert!(is_excluded(&excluded, "WhatsApp/Stickers"));
        assert!(is_excluded(&excluded, "whatsapp/stickers/a.webp"));
        assert!(!is_excluded(&excluded, "WhatsApp/Stickers2/a.webp"));
        assert!(!is_excluded(&excluded, "WhatsApp/a.jpg"));
    }

    #[test]
    fn folders_are_made_relative_to_the_library_folder() {
        let root = Path::new("/photos/2024");
        assert_eq!(
            relative(root, Path::new("/photos/2024/trip/raw")).as_deref(),
            Some("trip/raw")
        );
        assert_eq!(relative(root, Path::new("/photos/2024")), None);
        assert_eq!(relative(root, Path::new("/photos/2023/x")), None);
    }
}
