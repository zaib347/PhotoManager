//! Text in photos: read with the operating system's own OCR, so screenshots, receipts,
//! documents and signs can be found by the words in them. Nothing is downloaded:
//! Windows 10 and 11 read the languages installed in Windows (`Windows.Media.Ocr`);
//! macOS will use Apple's Vision framework when the Mac version is built. The text is
//! kept in a full-text index in the library (`ai_text`, by asset id).

use std::sync::atomic::{AtomicBool, Ordering};

use image::RgbImage;
use photo_core::Result;
use photo_core::db::Db;
use rusqlite::{Connection, OptionalExtension, params};

use crate::picture::{Picture, picture};
use crate::search::Progress;

/// Names the engine behind stored text; changing it reads photos again.
pub const ENGINE: &str = "windows-ocr";
/// Photos read per batch.
const BATCH: usize = 8;
/// Photos are read with their long edge about this long, and at most…
const READ_EDGE: u32 = 2000;
/// …this (the OCR engine has its own limit too).
const MAX_EDGE: u32 = 2600;
/// Most text kept for one photo.
const MAX_CHARS: usize = 20_000;
/// Most photos a text search returns.
const MAX_RESULTS: i64 = 2000;

/// The languages this computer can read text in, by name; empty if none.
pub fn languages() -> Vec<String> {
    os::languages()
}

/// Why text can't be read on this computer, if it can't.
pub fn unavailable() -> Option<String> {
    os::Reader::new().err().map(|e| e.to_string())
}

/// How many photos have been read for text, out of how many.
pub fn progress(conn: &Connection) -> Result<Progress> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM assets WHERE media_kind != 1 AND status = 0",
        [],
        |r| r.get(0),
    )?;
    let indexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ai_text_scans s JOIN assets a ON a.id = s.asset_id
         WHERE s.engine = ?1 AND a.media_kind != 1 AND a.status = 0",
        params![ENGINE],
        |r| r.get(0),
    )?;
    Ok(Progress {
        indexed: indexed as u64,
        total: total as u64,
    })
}

/// Reads the text in up to `limit` photos not read yet, in batches. `pause()` is
/// checked between batches (scans, battery) and stops the pass when true, as does
/// `cancel`. Calls `report` after each batch. Returns how many photos were done.
pub fn index_pending(
    library: &Db,
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
                   AND NOT EXISTS (SELECT 1 FROM ai_text_scans s WHERE s.asset_id = a.id AND s.engine = ?2)
                 ORDER BY a.id LIMIT ?3",
            )?;
            stmt.query_map(params![after, ENGINE, BATCH as i64], |r| r.get(0))?
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
        // The OCR engine lives on a thread of its own (its own COM apartment).
        let texts: Vec<(i64, Option<String>)> = std::thread::scope(|s| {
            s.spawn(|| -> Result<Vec<(i64, Option<String>)>> {
                let reader = os::Reader::new()?;
                let mut out = Vec::new();
                for (id, picture) in &pictures {
                    match picture {
                        Picture::Ready(image) => out.push((*id, reader.read(image).ok())),
                        Picture::Failed => out.push((*id, None)),
                        Picture::Later => {}
                    }
                }
                Ok(out)
            })
            .join()
            .expect("text reading thread")
        })?;
        let mut conn = library.write();
        let tx = conn.transaction()?;
        for (id, text) in &texts {
            let text = text.as_deref().map(|t| {
                let t = t.trim();
                match t.char_indices().nth(MAX_CHARS) {
                    Some((cut, _)) => &t[..cut],
                    None => t,
                }
            });
            tx.execute("DELETE FROM ai_text WHERE rowid = ?1", params![id])?;
            if let Some(t) = text.filter(|t| !t.is_empty()) {
                tx.execute(
                    "INSERT INTO ai_text (rowid, text) VALUES (?1, ?2)",
                    params![id, t],
                )?;
            }
            tx.execute(
                "INSERT INTO ai_text_scans (asset_id, engine, chars) VALUES (?1, ?2, ?3)
                 ON CONFLICT (asset_id) DO UPDATE SET engine = excluded.engine, chars = excluded.chars",
                params![id, ENGINE, text.map_or(-1, |t| t.chars().count() as i64)],
            )?;
            done += 1;
        }
        tx.commit()?;
        drop(conn);
        report();
    }
    Ok(done)
}

/// The full-text query for what the user typed: every word, each also matching longer
/// words it starts ("pass" finds "password").
fn match_query(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.to_lowercase()))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Photos whose text has every word of `query`, best first.
pub fn search(conn: &Connection, query: &str) -> Result<Vec<i64>> {
    let Some(q) = match_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT t.rowid FROM ai_text t JOIN assets a ON a.id = t.rowid
         WHERE ai_text MATCH ?1 AND a.status = 0 ORDER BY t.rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![q, MAX_RESULTS], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The text read in a photo.
pub fn text_of(conn: &Connection, asset_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT text FROM ai_text WHERE rowid = ?1",
            params![asset_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Forgets all text read (the feature is being removed).
pub fn clear(library: &Db) -> Result<()> {
    let mut conn = library.write();
    let tx = conn.transaction()?;
    tx.execute_batch("DELETE FROM ai_text; DELETE FROM ai_text_scans;")?;
    tx.commit()?;
    Ok(())
}

/// The text in an image, line by line (a development aid and for tests).
pub fn read_image(image: &RgbImage) -> Result<String> {
    os::Reader::new()?.read(image)
}

#[cfg(windows)]
mod os {
    use image::RgbImage;
    use photo_core::{Error, Result};
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    fn os_error(e: windows::core::Error) -> Error {
        Error::Invalid(format!("Windows couldn't read the text: {e}"))
    }

    pub struct Reader {
        engine: OcrEngine,
        max: u32,
    }

    impl Reader {
        /// An engine for the languages of the user's Windows profile. Call on a thread
        /// that hasn't joined a single-threaded COM apartment.
        pub fn new() -> Result<Reader> {
            // SAFETY: joins this thread to the multithreaded apartment; harmless if it
            // already is in one (the result says so).
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
                Error::Invalid(
                    "Windows can't read text in any of your languages. Add a language with \
                     its optical character recognition (OCR) part in Settings → Time & \
                     language → Language & region, then try again."
                        .into(),
                )
            })?;
            let max = OcrEngine::MaxImageDimension().unwrap_or(4096).max(256);
            Ok(Reader { engine, max })
        }

        pub fn read(&self, image: &RgbImage) -> Result<String> {
            let (w, h) = image.dimensions();
            let scaled;
            let image = if w.max(h) > self.max {
                let s = self.max as f32 / w.max(h) as f32;
                scaled = image::imageops::resize(
                    image,
                    ((w as f32 * s) as u32).max(1),
                    ((h as f32 * s) as u32).max(1),
                    image::imageops::FilterType::Triangle,
                );
                &scaled
            } else {
                image
            };
            let (w, h) = image.dimensions();
            let mut bgra = Vec::with_capacity((w * h * 4) as usize);
            for p in image.pixels() {
                bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
            let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).map_err(os_error)?;
            let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
                &buffer,
                BitmapPixelFormat::Bgra8,
                w as i32,
                h as i32,
            )
            .map_err(os_error)?;
            let result = self
                .engine
                .RecognizeAsync(&bitmap)
                .map_err(os_error)?
                .join()
                .map_err(os_error)?;
            let lines = result.Lines().map_err(os_error)?;
            let mut text = String::new();
            for i in 0..lines.Size().map_err(os_error)? {
                let line = lines.GetAt(i).map_err(os_error)?.Text().map_err(os_error)?;
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&line.to_string_lossy());
            }
            Ok(text)
        }
    }

    pub fn languages() -> Vec<String> {
        let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else {
            return Vec::new();
        };
        (0..list.Size().unwrap_or(0))
            .filter_map(|i| list.GetAt(i).ok()?.DisplayName().ok())
            .map(|n| n.to_string_lossy())
            .collect()
    }
}

#[cfg(not(windows))]
mod os {
    use image::RgbImage;
    use photo_core::{Error, Result};

    pub struct Reader;

    impl Reader {
        pub fn new() -> Result<Reader> {
            Err(Error::Invalid(
                "Reading text in photos isn't available on this system yet".into(),
            ))
        }

        pub fn read(&self, _: &RgbImage) -> Result<String> {
            Err(Error::Invalid("No text reader".into()))
        }
    }

    pub fn languages() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_words_become_a_prefix_query() {
        assert_eq!(
            match_query("Wi-Fi password!").as_deref(),
            Some("\"wi\"* \"fi\"* \"password\"*")
        );
        assert_eq!(match_query("  \"; --"), None);
        // Quotes can't break out of a term.
        assert_eq!(match_query("a\"b").as_deref(), Some("\"a\"* \"b\"*"));
    }
}
