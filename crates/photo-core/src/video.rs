//! Minimal ISO-BMFF (MP4/MOV/3GP) reader for the metadata the timeline needs:
//! creation time, duration, frame size and GPS. Only the `moov` box is read, so
//! this never touches the media data.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAC_EPOCH_OFFSET_S: i64 = 2_082_844_800; // 1904-01-01 → 1970-01-01
const MAX_MOOV: u64 = 64 << 20;

#[derive(Debug, Default, PartialEq)]
pub struct VideoMeta {
    /// UTC milliseconds. Many cameras write zero here; treat that as unknown.
    pub created_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

pub fn read(path: &Path) -> io::Result<VideoMeta> {
    let mut file = BufReader::new(File::open(path)?);
    let len = file.get_ref().metadata()?.len();
    let mut pos = 0;
    while pos + 8 <= len {
        file.seek(SeekFrom::Start(pos))?;
        let (kind, header, size) = box_header(&mut file, len - pos)?;
        if &kind == b"moov" {
            let body = size - header;
            if body > MAX_MOOV {
                break;
            }
            let mut moov = vec![0; body as usize];
            file.read_exact(&mut moov)?;
            return Ok(parse_moov(&moov));
        }
        pos += size;
    }
    Ok(VideoMeta::default())
}

/// Returns (type, header length, total size). Size 0 means "to end of file".
fn box_header(r: &mut impl Read, remaining: u64) -> io::Result<([u8; 4], u64, u64)> {
    let mut h = [0u8; 8];
    r.read_exact(&mut h)?;
    let kind = [h[4], h[5], h[6], h[7]];
    let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
    let (header, size) = match size {
        0 => (8, remaining),
        1 => {
            let mut big = [0u8; 8];
            r.read_exact(&mut big)?;
            (16, u64::from_be_bytes(big))
        }
        n => (8, n),
    };
    if size < header {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad box size"));
    }
    Ok((kind, header, size))
}

/// Iterates child boxes of an in-memory box body as (type, body).
fn children(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let (kind, header, size) = box_header(&mut &data[..], data.len() as u64).ok()?;
        let size = usize::try_from(size).ok()?.min(data.len());
        let body = data.get(header as usize..size)?;
        data = &data[size..];
        Some((kind, body))
    })
}

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn parse_moov(moov: &[u8]) -> VideoMeta {
    let mut meta = VideoMeta::default();
    for (kind, body) in children(moov) {
        match &kind {
            b"mvhd" => parse_mvhd(body, &mut meta),
            b"trak" if meta.width.is_none() => {
                for (kind, body) in children(body) {
                    if &kind == b"tkhd" {
                        parse_tkhd(body, &mut meta);
                    }
                }
            }
            b"udta" => {
                for (kind, body) in children(body) {
                    if kind == [0xA9, b'x', b'y', b'z'] {
                        // u16 length, u16 language, then an ISO 6709 string.
                        if let Some(text) = body.get(4..).map(String::from_utf8_lossy)
                            && let Some((lat, lon)) = parse_iso6709(&text)
                        {
                            meta.lat = Some(lat);
                            meta.lon = Some(lon);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    meta
}

fn parse_mvhd(b: &[u8], meta: &mut VideoMeta) {
    let parsed = match b.first() {
        Some(0) => (|| Some((be_u32(b, 4)? as u64, be_u32(b, 12)?, be_u32(b, 16)? as u64)))(),
        Some(1) => (|| Some((be_u64(b, 4)?, be_u32(b, 20)?, be_u64(b, 24)?)))(),
        _ => None,
    };
    let Some((created, timescale, duration)) = parsed else {
        return;
    };
    if created > MAC_EPOCH_OFFSET_S as u64 {
        meta.created_at = Some((created as i64 - MAC_EPOCH_OFFSET_S) * 1000);
    }
    if timescale > 0 {
        meta.duration_ms = Some((duration as u128 * 1000 / timescale as u128) as i64);
    }
}

fn parse_tkhd(b: &[u8], meta: &mut VideoMeta) {
    // Offset of the 3x3 matrix after version-dependent times and duration.
    let matrix = match b.first() {
        Some(0) => 4 + 20 + 16,
        Some(1) => 4 + 32 + 16,
        _ => return,
    };
    let (Some(w), Some(h)) = (be_u32(b, matrix + 36), be_u32(b, matrix + 40)) else {
        return;
    };
    let (w, h) = (w >> 16, h >> 16);
    if w == 0 || h == 0 {
        return; // audio track
    }
    // Matrix entries a=0, |b|=1 mean the frame is displayed rotated by 90°.
    let a = be_u32(b, matrix).unwrap_or(0x10000) as i32;
    let rotated = a == 0;
    meta.width = Some(if rotated { h } else { w });
    meta.height = Some(if rotated { w } else { h });
}

/// Parses the leading latitude/longitude pair of an ISO 6709 string such as
/// `+37.3349-122.0090+010.000/`.
fn parse_iso6709(s: &str) -> Option<(f64, f64)> {
    let s = s.trim_matches(char::from(0)).trim();
    let second = s.get(1..)?.find(['+', '-'])? + 1;
    let lat: f64 = s[..second].parse().ok()?;
    let rest = &s[second..];
    let end = rest
        .get(1..)
        .and_then(|r| r.find(['+', '-', '/']))
        .map_or(rest.len(), |i| i + 1);
    let lon: f64 = rest[..end].parse().ok()?;
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_moov_after_mdat() {
        let mut mvhd = vec![0u8; 100];
        mvhd[4..8].copy_from_slice(&((1_700_000_000 + MAC_EPOCH_OFFSET_S) as u32).to_be_bytes());
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&(600u32 * 12).to_be_bytes());

        let mut tkhd = vec![0u8; 84];
        tkhd[40..44].copy_from_slice(&0x10000u32.to_be_bytes()); // matrix a = 1
        tkhd[76..80].copy_from_slice(&(1920u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(1080u32 << 16).to_be_bytes());

        let mut xyz = vec![0, 0, 0, 0];
        xyz.extend_from_slice(b"+31.5204+074.3587/");
        let udta = boxed(&[0xA9, b'x', b'y', b'z'], &xyz);

        let moov = [
            boxed(b"mvhd", &mvhd),
            boxed(b"trak", &boxed(b"tkhd", &tkhd)),
            boxed(b"udta", &udta),
        ]
        .concat();
        let file = [
            boxed(b"ftyp", b"isom"),
            boxed(b"mdat", &[0; 1000]),
            boxed(b"moov", &moov),
        ]
        .concat();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, file).unwrap();

        let meta = read(&path).unwrap();
        assert_eq!(meta.created_at, Some(1_700_000_000_000));
        assert_eq!(meta.duration_ms, Some(12_000));
        assert_eq!((meta.width, meta.height), (Some(1920), Some(1080)));
        assert_eq!((meta.lat, meta.lon), (Some(31.5204), Some(74.3587)));
    }

    #[test]
    fn iso6709_variants() {
        assert_eq!(
            parse_iso6709("+37.3349-122.0090+010.000/"),
            Some((37.3349, -122.009))
        );
        assert_eq!(
            parse_iso6709("-33.8688+151.2093/"),
            Some((-33.8688, 151.2093))
        );
        assert_eq!(parse_iso6709("garbage"), None);
    }
}
