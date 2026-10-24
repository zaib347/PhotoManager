//! Media type detection and metadata extraction (EXIF, video atoms, filenames).
//!
//! Time convention: when a capture time has no known UTC offset (most EXIF), it is
//! stored as a "floating" local time: `taken_at` holds the wall-clock time as if it
//! were UTC and `tz_offset_min` is 0. `local_day` is then still the right calendar day.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use chrono::{Local, NaiveDate, TimeZone};
use exif::{Exif, In, Tag, Value};

use crate::video;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MediaKind {
    Photo = 0,
    Video = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TimeSource {
    Exif = 0,
    Video = 1,
    Filename = 2,
    Mtime = 3,
}

const PHOTO_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "heic", "heif", "avif", "tif", "tiff", "dng", "cr2",
    "cr3", "nef", "arw", "orf", "rw2", "raf", "srw", "pef",
];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "3gp", "avi", "mkv", "mts", "m2ts", "webm",
];
const ISO_BMFF_VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "3gp"];

/// MIME type for formats a webview can show natively. `None` means the file has to be
/// decoded in Rust (photos) or opened in the system player (videos).
pub fn web_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        // QuickTime files are usually H.264/HEVC in an MP4-compatible container.
        "mp4" | "m4v" | "mov" => "video/mp4",
        "webm" => "video/webm",
        "3gp" => "video/3gpp",
        _ => return None,
    })
}

pub fn kind_of(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if PHOTO_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Photo)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

#[derive(Debug, Default)]
pub struct Metadata {
    pub taken_at: i64,
    pub taken_src: Option<TimeSource>,
    pub tz_offset_min: i32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u8>,
    pub duration_ms: Option<i64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// JPEG thumbnail embedded in EXIF, if present (typically ~160px).
    pub embedded_thumb: Option<Vec<u8>>,
}

impl Metadata {
    pub fn local_day(&self) -> i64 {
        crate::timeline::local_day(self.taken_at, self.tz_offset_min)
    }
}

/// Extracts what can be read cheaply. Never fails: missing metadata falls back to
/// the filename and finally the file's modification time.
pub fn extract(path: &Path, kind: MediaKind, mtime_ms: i64) -> Metadata {
    let mut meta = Metadata::default();
    match kind {
        MediaKind::Photo => {
            match read_exif(path) {
                Some(exif) => apply_exif(&exif, &mut meta),
                // CR3 and RAF keep their EXIF where the reader can't look.
                None => {
                    for exif in crate::raw::exif(path) {
                        apply_exif(&exif, &mut meta);
                    }
                }
            }
            if meta.width.is_none()
                && let Ok((w, h)) = image::image_dimensions(path)
            {
                meta.width = Some(w);
                meta.height = Some(h);
            }
        }
        MediaKind::Video => {
            let is_bmff = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ISO_BMFF_VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()));
            if let Some(v) = is_bmff.then(|| video::read(path).ok()).flatten() {
                if let Some(created) = v.created_at {
                    // Container times are UTC; show them in this machine's zone.
                    meta.tz_offset_min = local_offset_min(created);
                    meta.taken_at = created;
                    meta.taken_src = Some(TimeSource::Video);
                }
                meta.duration_ms = v.duration_ms;
                meta.width = v.width;
                meta.height = v.height;
                meta.lat = v.lat;
                meta.lon = v.lon;
            } else if let Some(v) = crate::os_thumb::video_props(path) {
                // AVI, MKV, MTS…: what the OS's own handlers read from the file.
                if let Some(recorded) = v.recorded_ms {
                    meta.tz_offset_min = local_offset_min(recorded);
                    meta.taken_at = recorded;
                    meta.taken_src = Some(TimeSource::Video);
                }
                meta.duration_ms = v.duration_ms;
                meta.width = v.width;
                meta.height = v.height;
            }
        }
    }

    if meta.taken_src.is_none() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(t) = date_from_filename(name) {
            meta.taken_at = t;
            meta.taken_src = Some(TimeSource::Filename);
        } else if let Some(t) = epoch_from_filename(name) {
            meta.taken_at = t;
            meta.tz_offset_min = local_offset_min(t);
            meta.taken_src = Some(TimeSource::Filename);
        } else {
            meta.taken_at = mtime_ms;
            meta.tz_offset_min = local_offset_min(mtime_ms);
            meta.taken_src = Some(TimeSource::Mtime);
        }
    }
    meta
}

fn local_offset_min(utc_ms: i64) -> i32 {
    Local
        .timestamp_millis_opt(utc_ms)
        .single()
        .map_or(0, |t| t.offset().local_minus_utc() / 60)
}

fn read_exif(path: &Path) -> Option<Exif> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    exif::Reader::new().read_from_container(&mut reader).ok()
}

fn ascii(exif: &Exif, tag: Tag) -> Option<String> {
    ascii_in(exif, tag, In::PRIMARY)
}

/// Some phone firmware (notably MediaTek-based) writes capture date and camera into
/// the thumbnail IFD instead of the primary one, so fall back to it.
fn ascii_any(exif: &Exif, tag: Tag) -> Option<String> {
    ascii_in(exif, tag, In::PRIMARY).or_else(|| ascii_in(exif, tag, In::THUMBNAIL))
}

fn ascii_in(exif: &Exif, tag: Tag, ifd: In) -> Option<String> {
    match &exif.get_field(tag, ifd)?.value {
        Value::Ascii(v) => {
            let s = String::from_utf8_lossy(v.first()?);
            let s = s.trim_matches(char::from(0)).trim();
            (!s.is_empty()).then(|| s.to_owned())
        }
        _ => None,
    }
}

fn uint(exif: &Exif, tag: Tag) -> Option<u32> {
    exif.get_field(tag, In::PRIMARY)?.value.get_uint(0)
}

fn apply_exif(exif: &Exif, meta: &mut Metadata) {
    for (time_tag, offset_tag) in [
        (Tag::DateTimeOriginal, Tag::OffsetTimeOriginal),
        (Tag::DateTimeDigitized, Tag::OffsetTimeDigitized),
        (Tag::DateTime, Tag::OffsetTime),
    ] {
        let Some(text) = ascii_any(exif, time_tag) else {
            continue;
        };
        let Ok(mut dt) = exif::DateTime::from_ascii(text.as_bytes()) else {
            continue;
        };
        if let Some(offset) = ascii_any(exif, offset_tag) {
            let _ = dt.parse_offset(offset.as_bytes());
        }
        let Some(wall) = NaiveDate::from_ymd_opt(dt.year as i32, dt.month as u32, dt.day as u32)
            .and_then(|d| d.and_hms_opt(dt.hour as u32, dt.minute as u32, dt.second as u32))
        else {
            continue;
        };
        let wall_ms = wall.and_utc().timestamp_millis();
        let offset = dt.offset.unwrap_or(0) as i32;
        meta.taken_at = wall_ms - offset as i64 * 60_000;
        meta.tz_offset_min = offset;
        meta.taken_src = Some(TimeSource::Exif);
        break;
    }

    meta.width = uint(exif, Tag::PixelXDimension).or_else(|| uint(exif, Tag::ImageWidth));
    meta.height = uint(exif, Tag::PixelYDimension).or_else(|| uint(exif, Tag::ImageLength));
    meta.orientation = uint(exif, Tag::Orientation)
        .map(|o| o as u8)
        .filter(|o| (1..=8).contains(o));
    meta.camera_make = ascii_any(exif, Tag::Make);
    meta.camera_model = ascii_any(exif, Tag::Model);

    let coord = |value_tag, ref_tag, negative: &str| -> Option<f64> {
        let Value::Rational(parts) = &exif.get_field(value_tag, In::PRIMARY)?.value else {
            return None;
        };
        let mut deg = 0.0;
        for (part, scale) in parts.iter().zip([1.0, 60.0, 3600.0]) {
            deg += part.to_f64() / scale;
        }
        let sign = if ascii(exif, ref_tag).as_deref() == Some(negative) {
            -1.0
        } else {
            1.0
        };
        deg.is_finite().then_some(deg * sign)
    };
    let lat = coord(Tag::GPSLatitude, Tag::GPSLatitudeRef, "S");
    let lon = coord(Tag::GPSLongitude, Tag::GPSLongitudeRef, "W");
    // 0,0 is almost always a camera writing empty GPS fields.
    if let (Some(lat), Some(lon)) = (lat, lon)
        && (lat, lon) != (0.0, 0.0)
        && lat.abs() <= 90.0
        && lon.abs() <= 180.0
    {
        meta.lat = Some(lat);
        meta.lon = Some(lon);
    }

    let thumb = (|| {
        let offset = exif
            .get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL)?
            .value
            .get_uint(0)?;
        let len = exif
            .get_field(Tag::JPEGInterchangeFormatLength, In::THUMBNAIL)?
            .value
            .get_uint(0)?;
        exif.buf()
            .get(offset as usize..(offset as usize + len as usize))
            .map(<[u8]>::to_vec)
    })();
    meta.embedded_thumb = thumb.filter(|t| t.starts_with(&[0xFF, 0xD8]));
}

/// Finds a capture date in names like `IMG_20190312_101530.jpg`,
/// `2019-03-12 10.15.30.jpg` or `PXL_20240101_235959123.mp4`. Returns floating ms.
pub fn date_from_filename(name: &str) -> Option<i64> {
    let b = name.as_bytes();
    (0..b.len()).find_map(|i| {
        if i > 0 && b[i - 1].is_ascii_digit() {
            return None;
        }
        let (date, mut at) = parse_date(b, i)?;
        let mut time = (0, 0, 0);
        if matches!(b.get(at), Some(b'_' | b'-' | b' ' | b'T')) {
            at += 1;
        }
        if let Some((t, _)) = parse_time(b, at) {
            time = t;
        }
        date.and_hms_opt(time.0, time.1, time.2)
            .map(|dt| dt.and_utc().timestamp_millis())
    })
}

/// Unix-epoch filenames such as `1759554341584.jpg` (ms) or `1759554341.jpg` (s),
/// written by WhatsApp, Telegram and many Android apps. Returns true UTC ms.
pub fn epoch_from_filename(name: &str) -> Option<i64> {
    const MIN_MS: i64 = 1_104_537_600_000; // 2005-01-01
    let stem = name.split('.').next()?;
    let digits = stem.trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits = &digits[..digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len())];
    let value: i64 = digits.parse().ok()?;
    let ms = match digits.len() {
        13 => value,
        10 => value * 1000,
        _ => return None,
    };
    let max_ms = crate::now_ms() + crate::timeline::DAY_MS;
    (MIN_MS..=max_ms).contains(&ms).then_some(ms)
}

fn digits(b: &[u8], at: usize, n: usize) -> Option<u32> {
    let s = b.get(at..at + n)?;
    s.iter()
        .all(u8::is_ascii_digit)
        .then(|| s.iter().fold(0, |acc, d| acc * 10 + (d - b'0') as u32))
}

fn skip_sep(b: &[u8], at: usize) -> usize {
    if matches!(b.get(at), Some(b'-' | b'_' | b'.' | b':')) {
        at + 1
    } else {
        at
    }
}

fn parse_date(b: &[u8], at: usize) -> Option<(NaiveDate, usize)> {
    let year = digits(b, at, 4)?;
    if !(1990..=2100).contains(&year) {
        return None;
    }
    let mut i = skip_sep(b, at + 4);
    let month = digits(b, i, 2)?;
    i = skip_sep(b, i + 2);
    let day = digits(b, i, 2)?;
    Some((NaiveDate::from_ymd_opt(year as i32, month, day)?, i + 2))
}

fn parse_time(b: &[u8], at: usize) -> Option<((u32, u32, u32), usize)> {
    let h = digits(b, at, 2)?;
    let mut i = skip_sep(b, at + 2);
    let m = digits(b, i, 2)?;
    i = skip_sep(b, i + 2);
    let s = digits(b, i, 2)?;
    (h < 24 && m < 60 && s < 60).then_some(((h, m, s), i + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn filename_dates() {
        assert_eq!(
            date_from_filename("IMG_20190312_101530.jpg"),
            Some(ms(2019, 3, 12, 10, 15, 30))
        );
        assert_eq!(
            date_from_filename("2019-03-12 10.15.30.jpg"),
            Some(ms(2019, 3, 12, 10, 15, 30))
        );
        assert_eq!(
            date_from_filename("PXL_20240101_235959123.mp4"),
            Some(ms(2024, 1, 1, 23, 59, 59))
        );
        assert_eq!(
            date_from_filename("Screenshot_2021-07-04.png"),
            Some(ms(2021, 7, 4, 0, 0, 0))
        );
        assert_eq!(date_from_filename("DSC_0042.JPG"), None);
        assert_eq!(
            date_from_filename("IMG_20191340_000000.jpg"),
            None,
            "invalid month"
        );
        assert_eq!(
            date_from_filename("12345201903120.jpg"),
            None,
            "inside a longer number"
        );
    }

    #[test]
    fn epoch_filenames() {
        assert_eq!(
            epoch_from_filename("1759554341584.jpg"),
            Some(1_759_554_341_584)
        );
        assert_eq!(
            epoch_from_filename("IMG-1759554341.jpg"),
            Some(1_759_554_341_000)
        );
        assert_eq!(
            epoch_from_filename("0000000000001.jpg"),
            None,
            "before 2005"
        );
        assert_eq!(
            epoch_from_filename("9999999999999.jpg"),
            None,
            "in the future"
        );
        assert_eq!(epoch_from_filename("12345.jpg"), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a/B.JPG")), Some(MediaKind::Photo));
        assert_eq!(kind_of(Path::new("clip.MOV")), Some(MediaKind::Video));
        assert_eq!(kind_of(Path::new("notes.txt")), None);
    }
}
