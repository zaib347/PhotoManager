//! Camera RAW files, shown through the JPEG preview the camera embedded in them
//! (usually 1600 px to full size). Decoding the sensor data itself would need a RAW
//! developer; the preview is what other photo browsers show too, and reading it is
//! fast and works on every platform.
//!
//! - TIFF-based (CR2, NEF, ARW, DNG, ORF, PEF, SRW, RW2): the largest baseline or
//!   progressive JPEG in any image directory, sub-directories included.
//! - CR3: the `PRVW` preview box. RAF: the JPEG offset in Fuji's header.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use image::DynamicImage;

use crate::{Error, Result};

const TIFF_RAWS: &[&str] = &["cr2", "nef", "arw", "dng", "orf", "pef", "srw", "rw2"];

/// Biggest preview read; anything larger is not a preview.
const MAX_PREVIEW: u64 = 64 << 20;

pub fn is_raw(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    TIFF_RAWS.contains(&ext.as_str()) || ext == "cr3" || ext == "raf"
}

/// Opens a photo: RAW files through their embedded preview, everything else with the
/// `image` crate.
pub fn open(path: &Path) -> Result<DynamicImage> {
    if is_raw(path) {
        let jpeg = preview(path)?;
        return Ok(image::load_from_memory_with_format(
            &jpeg,
            image::ImageFormat::Jpeg,
        )?);
    }
    Ok(image::open(path)?)
}

/// The largest JPEG preview in a RAW file.
pub fn preview(path: &Path) -> Result<Vec<u8>> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut file = File::open(path)?;
    let found = match ext.as_str() {
        "cr3" => cr3_preview(&mut file)?,
        "raf" => raf_preview(&mut file)?,
        _ => tiff_preview(&mut file)?,
    };
    let (offset, len) =
        found.ok_or_else(|| Error::Invalid("No preview in this RAW file".into()))?;
    read_jpeg(&mut file, offset, len)
}

/// EXIF for RAW formats the EXIF reader can't open as a container: the TIFF blocks
/// in a CR3's `CMT1`–`CMT4` boxes (camera, capture details, maker notes, GPS), or
/// the EXIF in a RAF's preview JPEG. Empty for TIFF-based RAWs, which it reads itself.
pub fn exif(path: &Path) -> Vec<exif::Exif> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let reader = exif::Reader::new();
    match ext.as_str() {
        "raf" => preview(path)
            .ok()
            .and_then(|jpeg| {
                reader
                    .read_from_container(&mut std::io::Cursor::new(jpeg))
                    .ok()
            })
            .into_iter()
            .collect(),
        "cr3" => {
            let Ok(head) = read_head(path, 4 << 20) else {
                return Vec::new();
            };
            [b"CMT1", b"CMT2", b"CMT4"]
                .iter()
                .filter_map(|name| {
                    let at = head.windows(4).position(|w| w == *name)?;
                    let start = at.checked_sub(4)?;
                    let size = u32::from_be_bytes(head.get(start..at)?.try_into().ok()?);
                    let block = head.get(at + 4..start + size as usize)?;
                    reader.read_raw(block.to_vec()).ok()
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn read_head(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = vec![0u8; len.min(max) as usize];
    file.read_exact(&mut head)?;
    Ok(head)
}

fn read_jpeg(file: &mut File, offset: u64, len: u64) -> Result<Vec<u8>> {
    if !(4..=MAX_PREVIEW).contains(&len) {
        return Err(Error::Invalid("No usable preview in this RAW file".into()));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut data = vec![0; len as usize];
    file.read_exact(&mut data)?;
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(Error::Invalid("The RAW preview isn't a JPEG".into()));
    }
    Ok(data)
}

/// A TIFF file read through `Seek`, in its byte order.
struct Tiff<'a> {
    file: &'a mut File,
    little: bool,
    len: u64,
}

impl Tiff<'_> {
    fn bytes<const N: usize>(&mut self, at: u64) -> Option<[u8; N]> {
        if at + N as u64 > self.len {
            return None;
        }
        let mut buf = [0u8; N];
        self.file.seek(SeekFrom::Start(at)).ok()?;
        self.file.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    fn u16(&mut self, at: u64) -> Option<u16> {
        let b = self.bytes::<2>(at)?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&mut self, at: u64) -> Option<u32> {
        let b = self.bytes::<4>(at)?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// A SHORT or LONG entry's first value, or every LONG value for SubIFDs.
    fn values(&mut self, entry: u64) -> Vec<u64> {
        let (Some(kind), Some(count)) = (self.u16(entry + 2), self.u32(entry + 4)) else {
            return Vec::new();
        };
        let size = match kind {
            3 => 2,
            4 | 13 => 4,
            _ => return Vec::new(),
        };
        let count = count.min(64) as u64;
        let at = if size * count <= 4 {
            entry + 8
        } else {
            match self.u32(entry + 8) {
                Some(o) => o as u64,
                None => return Vec::new(),
            }
        };
        (0..count)
            .filter_map(|i| {
                if size == 2 {
                    self.u16(at + i * 2).map(u64::from)
                } else {
                    self.u32(at + i * 4).map(u64::from)
                }
            })
            .collect()
    }
}

/// Whether the JPEG at `offset` is one the `image` crate decodes (baseline or
/// progressive), not the lossless JPEG some RAWs store sensor data in.
fn is_viewable_jpeg(tiff: &mut Tiff, offset: u64) -> bool {
    let mut at = offset;
    if tiff.bytes::<2>(at) != Some([0xFF, 0xD8]) {
        return false;
    }
    at += 2;
    // Walk the segments up to the frame header.
    for _ in 0..64 {
        let Some([0xFF, marker]) = tiff.bytes::<2>(at) else {
            return false;
        };
        match marker {
            0xC0..=0xC2 => return true,
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return false,
            0xDA | 0xD9 => return false,
            _ => {}
        }
        let Some(len) = tiff.bytes::<2>(at + 2).map(u16::from_be_bytes) else {
            return false;
        };
        at += 2 + len as u64;
    }
    false
}

fn tiff_preview(file: &mut File) -> Result<Option<(u64, u64)>> {
    let len = file.metadata()?.len();
    let mut head = [0u8; 8];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let little = match &head[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Ok(None),
    };
    let mut tiff = Tiff { file, little, len };
    let first = if little {
        u32::from_le_bytes([head[4], head[5], head[6], head[7]])
    } else {
        u32::from_be_bytes([head[4], head[5], head[6], head[7]])
    } as u64;

    let mut best: Option<(u64, u64)> = None;
    let mut queue = vec![first];
    let mut seen = std::collections::HashSet::new();
    while let Some(ifd) = queue.pop() {
        if ifd == 0 || ifd >= len || !seen.insert(ifd) || seen.len() > 32 {
            continue;
        }
        let Some(count) = tiff.u16(ifd) else { continue };
        let (mut jpeg_at, mut jpeg_len) = (None, None);
        let (mut strip_at, mut strip_len, mut compression) = (None, None, None);
        for i in 0..count.min(512) as u64 {
            let entry = ifd + 2 + i * 12;
            let Some(tag) = tiff.u16(entry) else { break };
            match tag {
                0x0201 => jpeg_at = tiff.values(entry).first().copied(),
                0x0202 => jpeg_len = tiff.values(entry).first().copied(),
                0x0111 => strip_at = Some(tiff.values(entry)),
                0x0117 => strip_len = Some(tiff.values(entry)),
                0x0103 => compression = tiff.values(entry).first().copied(),
                0x014A => queue.extend(tiff.values(entry)),
                // Panasonic RW2 keeps a full-size JPEG under its own tag.
                0x002E => {
                    if let (Some(4 | 7), Some(n)) = (tiff.u16(entry + 2), tiff.u32(entry + 4)) {
                        jpeg_len = Some(n as u64);
                        jpeg_at = tiff.u32(entry + 8).map(u64::from);
                    }
                }
                _ => {}
            }
        }
        let mut candidates = Vec::new();
        if let (Some(at), Some(n)) = (jpeg_at, jpeg_len) {
            candidates.push((at, n));
        }
        // A single strip of old-style (6) or new-style (7) JPEG.
        if let (Some(6 | 7), Some(at), Some(n)) = (compression, strip_at, strip_len)
            && at.len() == 1
            && n.len() == 1
        {
            candidates.push((at[0], n[0]));
        }
        for (at, n) in candidates {
            let fits = at + n <= len && n > 0;
            if fits && best.is_none_or(|(_, b)| n > b) && is_viewable_jpeg(&mut tiff, at) {
                best = Some((at, n));
            }
        }
        if let Some(next) = tiff.u32(ifd + 2 + count as u64 * 12) {
            queue.push(next as u64);
        }
    }
    Ok(best)
}

/// Canon CR3: a `PRVW` box (inside a `uuid` box near the start) holds a ~1620 px
/// JPEG, its size four bytes before the JPEG.
fn cr3_preview(file: &mut File) -> Result<Option<(u64, u64)>> {
    let len = file.metadata()?.len();
    let mut head = vec![0u8; len.min(4 << 20) as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let Some(at) = head.windows(4).position(|w| w == b"PRVW") else {
        return Ok(None);
    };
    let window = &head[at..head.len().min(at + 64)];
    let Some(soi) = window.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) else {
        return Ok(None);
    };
    let soi = at + soi;
    if soi < 4 {
        return Ok(None);
    }
    let size = u32::from_be_bytes([head[soi - 4], head[soi - 3], head[soi - 2], head[soi - 1]]);
    Ok(Some((soi as u64, size as u64)))
}

/// Fujifilm RAF: the header gives the JPEG's offset and length at bytes 84 and 88.
fn raf_preview(file: &mut File) -> Result<Option<(u64, u64)>> {
    let mut head = [0u8; 92];
    file.seek(SeekFrom::Start(0))?;
    if file.read_exact(&mut head).is_err() || !head.starts_with(b"FUJIFILMCCD-RAW") {
        return Ok(None);
    }
    let at = u32::from_be_bytes([head[84], head[85], head[86], head[87]]);
    let n = u32::from_be_bytes([head[88], head[89], head[90], head[91]]);
    Ok(Some((at as u64, n as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        crate::thumbs::encode_jpeg(&RgbImage::from_pixel(w, h, Rgb([200, 30, 30]))).unwrap()
    }

    /// A little-endian TIFF: IFD0 with a small thumbnail, a SubIFD holding a big
    /// JPEG as a single strip (as NEF and ARW do).
    fn nef_like(small: &[u8], big: &[u8]) -> Vec<u8> {
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        let entry = |tag: u16, kind: u16, value: u32| {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend(kind.to_le_bytes());
            e.extend(1u32.to_le_bytes());
            e.extend(value.to_le_bytes());
            e
        };
        // IFD0 at 8: 3 entries; SubIFD right after; then the JPEGs.
        let ifd0_size = 2 + 3 * 12 + 4;
        let sub_at = 8 + ifd0_size;
        let sub_size = 2 + 3 * 12 + 4;
        let small_at = sub_at + sub_size;
        let big_at = small_at + small.len() as u32;
        out.extend(3u16.to_le_bytes());
        out.extend(entry(0x0201, 4, small_at));
        out.extend(entry(0x0202, 4, small.len() as u32));
        out.extend(entry(0x014A, 4, sub_at));
        out.extend(0u32.to_le_bytes());
        out.extend(3u16.to_le_bytes());
        out.extend(entry(0x0103, 3, 6));
        out.extend(entry(0x0111, 4, big_at));
        out.extend(entry(0x0117, 4, big.len() as u32));
        out.extend(0u32.to_le_bytes());
        out.extend(small);
        out.extend(big);
        out
    }

    #[test]
    fn tiff_raws_give_their_biggest_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("DSC_0001.NEF");
        std::fs::write(&path, nef_like(&jpeg(160, 120), &jpeg(1600, 1200))).unwrap();
        assert!(is_raw(&path));
        let img = open(&path).unwrap();
        assert_eq!((img.width(), img.height()), (1600, 1200));
    }

    #[test]
    fn cr3_and_raf_previews() {
        let dir = tempfile::tempdir().unwrap();
        let preview = jpeg(320, 200);

        let mut cr3 = b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom".to_vec();
        cr3.extend(b"\0\0\0\0PRVW\0\0\0\0\0\x01\x01\x40\0\xc8\0\0");
        cr3.extend((preview.len() as u32).to_be_bytes());
        cr3.extend(&preview);
        let path = dir.path().join("IMG_0001.CR3");
        std::fs::write(&path, &cr3).unwrap();
        assert_eq!(open(&path).unwrap().width(), 320);

        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(84, 0);
        raf.extend(100u32.to_be_bytes());
        raf.extend((preview.len() as u32).to_be_bytes());
        raf.resize(100, 0);
        raf.extend(&preview);
        let path = dir.path().join("DSCF0001.RAF");
        std::fs::write(&path, &raf).unwrap();
        assert_eq!(open(&path).unwrap().height(), 200);
    }

    #[test]
    fn files_without_a_preview_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.cr2");
        std::fs::write(&path, b"II*\0\x08\0\0\0\0\0\0\0\0\0").unwrap();
        assert!(open(&path).is_err());
    }
}
