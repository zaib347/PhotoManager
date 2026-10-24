//! Export: writes copies of photos, optionally resized, converted or compressed, with
//! their metadata carried over (capture date, camera, GPS unless removed, colour
//! profile). Originals are never touched, and an existing file is never overwritten:
//! a name already taken in the destination gets " (2)".
//!
//! Rotation is applied to the pixels, so the copy's EXIF has no orientation tag.
//! MakerNotes are left out: they hold offsets into the original's EXIF that a
//! rewritten block would break.

use std::collections::HashSet;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use exif::{Context, Exif, In, Tag, Value};
use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, RgbImage, RgbaImage};
use img_parts::{Bytes, ImageEXIF, ImageICC};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::edit::Look;
use crate::library::{self, AssetSource};
use crate::watermark::{self, Mark, Watermark};
use crate::{Error, Result};

/// Exports running at once: each holds a full-size image or two in memory.
const MAX_THREADS: usize = 4;
/// Errors kept in a report; the rest are only counted.
const MAX_ERRORS: usize = 20;

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    /// JPEG, PNG and WebP stay as they are; everything else (HEIC, TIFF…) becomes JPEG.
    #[default]
    Original,
    Jpeg,
    Webp,
    Png,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub format: Format,
    /// 1–100, for JPEG and WebP.
    pub quality: u8,
    /// Shrinks so the longer edge is at most this; never enlarges.
    pub max_edge: Option<u32>,
    /// Leaves GPS out of the copy (for sharing).
    pub remove_location: bool,
    /// Drawn on every copy when set.
    pub watermark: Option<Watermark>,
    pub folder: PathBuf,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            format: Format::Original,
            quality: 85,
            max_edge: None,
            remove_location: false,
            watermark: None,
            folder: PathBuf::new(),
        }
    }
}

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub exported: usize,
    pub failed: usize,
    /// Videos (not converted), items with no file, cancelled ones.
    pub skipped: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub folder: String,
    pub errors: Vec<String>,
}

/// The format a source ends up in.
fn target_format(format: Format, source: &Path) -> Format {
    if format != Format::Original {
        return format;
    }
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => Format::Png,
        "webp" => Format::Webp,
        _ => Format::Jpeg,
    }
}

fn extension(format: Format) -> &'static str {
    match format {
        Format::Png => "png",
        Format::Webp => "webp",
        _ => "jpg",
    }
}

/// A decoded original, upright, with what it carried besides pixels.
struct Decoded {
    image: DynamicImage,
    icc: Option<Vec<u8>>,
    exif: Option<Exif>,
}

fn decode(source: &Path, bytes: &[u8]) -> Result<Decoded> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()
        .or_else(|| crate::raw::exif(source).into_iter().next());
    // A RAW file is exported from the JPEG preview the camera embedded in it.
    let preview = if crate::raw::is_raw(source) {
        Some(crate::raw::preview(source)?)
    } else {
        None
    };
    let ours = ImageReader::new(Cursor::new(preview.as_deref().unwrap_or(bytes)))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_decoder().ok());
    match ours {
        Some(mut decoder) => {
            let icc = decoder.icc_profile().ok().flatten();
            let mut image = DynamicImage::from_decoder(decoder)?;
            let orientation = exif
                .as_ref()
                .and_then(|e| e.get_field(Tag::Orientation, In::PRIMARY))
                .and_then(|f| f.value.get_uint(0))
                .and_then(|o| Orientation::from_exif(o as u8));
            if let Some(o) = orientation {
                image.apply_orientation(o);
            }
            Ok(Decoded { image, icc, exif })
        }
        // The OS decoder (HEIC) hands over pixels already turned the right way up.
        None => Ok(Decoded {
            image: crate::os_thumb::full_image(source)?,
            icc: None,
            exif,
        }),
    }
}

/// Shrinks so the longer edge is `max_edge` (Lanczos3), keeping alpha if there is any.
fn resize(image: DynamicImage, max_edge: u32) -> Result<DynamicImage> {
    let (w, h) = (image.width(), image.height());
    let long = w.max(h);
    if long <= max_edge || max_edge == 0 {
        return Ok(image);
    }
    let scale = max_edge as f64 / long as f64;
    let (tw, th) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let alpha = image.color().has_alpha();
    let (pixels, kind) = if alpha {
        (image.to_rgba8().into_raw(), PixelType::U8x4)
    } else {
        (image.to_rgb8().into_raw(), PixelType::U8x3)
    };
    let invalid = |e: &dyn std::fmt::Display| Error::Invalid(format!("Resize failed: {e}"));
    let src = ImageRef::new(w, h, &pixels, kind).map_err(|e| invalid(&e))?;
    let mut dst = Image::new(tw, th, kind);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new()
        .resize(&src, &mut dst, &options)
        .map_err(|e| invalid(&e))?;
    let out = dst.into_vec();
    Ok(if alpha {
        DynamicImage::ImageRgba8(RgbaImage::from_raw(tw, th, out).expect("sized buffer"))
    } else {
        DynamicImage::ImageRgb8(RgbImage::from_raw(tw, th, out).expect("sized buffer"))
    })
}

/// Transparent pixels over white, for formats without alpha.
fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])])
    })
}

fn encode(image: &DynamicImage, format: Format, quality: u8) -> Result<Vec<u8>> {
    let quality = quality.clamp(1, 100);
    let mut out = Vec::new();
    match format {
        Format::Png => {
            let (w, h) = (image.width(), image.height());
            if image.color().has_alpha() {
                let px = image.to_rgba8();
                PngEncoder::new(&mut out).write_image(
                    &px,
                    w,
                    h,
                    image::ExtendedColorType::Rgba8,
                )?;
            } else {
                let px = image.to_rgb8();
                PngEncoder::new(&mut out).write_image(&px, w, h, image::ExtendedColorType::Rgb8)?;
            }
        }
        Format::Webp => {
            let (w, h) = (image.width(), image.height());
            let encoded = if image.color().has_alpha() {
                let px = image.to_rgba8();
                webp::Encoder::from_rgba(&px, w, h)
                    .encode(quality as f32)
                    .to_vec()
            } else {
                let px = image.to_rgb8();
                webp::Encoder::from_rgb(&px, w, h)
                    .encode(quality as f32)
                    .to_vec()
            };
            out = encoded;
        }
        Format::Jpeg | Format::Original => {
            out = crate::jpeg::encode(&flatten(image), quality, crate::jpeg::Purpose::Keep)?;
        }
    }
    Ok(out)
}

/// The original's EXIF rebuilt for the copy: no orientation (already applied), no
/// MakerNote or thumbnail, and no GPS if asked.
fn copy_exif(exif: &Exif, remove_location: bool) -> Option<Vec<u8>> {
    let mut writer = exif::experimental::Writer::new();
    let mut any = false;
    for field in exif.fields() {
        let skip = field.ifd_num != In::PRIMARY
            || matches!(field.tag, Tag::Orientation | Tag::MakerNote)
            || (remove_location && field.tag.context() == Context::Gps)
            || matches!(field.value, Value::Unknown(..));
        if !skip {
            writer.push_field(field);
            any = true;
        }
    }
    if !any {
        return None;
    }
    let mut buf = Cursor::new(Vec::new());
    writer.write(&mut buf, exif.little_endian()).ok()?;
    Some(buf.into_inner())
}

/// Puts EXIF and the colour profile into an encoded file.
fn attach(
    encoded: Vec<u8>,
    format: Format,
    exif: Option<Vec<u8>>,
    icc: Option<Vec<u8>>,
) -> Vec<u8> {
    fn set<T: ImageEXIF + ImageICC>(img: &mut T, exif: Option<Vec<u8>>, icc: Option<Vec<u8>>) {
        if exif.is_some() {
            img.set_exif(exif.map(Bytes::from));
        }
        if icc.is_some() {
            img.set_icc_profile(icc.map(Bytes::from));
        }
    }
    let bytes = Bytes::from(encoded.clone());
    let mut out = Vec::with_capacity(encoded.len() + 64 * 1024);
    let written = match format {
        Format::Png => img_parts::png::Png::from_bytes(bytes).ok().map(|mut img| {
            set(&mut img, exif, icc);
            img.encoder().write_to(&mut out)
        }),
        Format::Webp => img_parts::webp::WebP::from_bytes(bytes)
            .ok()
            .map(|mut img| {
                set(&mut img, exif, icc);
                img.encoder().write_to(&mut out)
            }),
        Format::Jpeg | Format::Original => {
            img_parts::jpeg::Jpeg::from_bytes(bytes)
                .ok()
                .map(|mut img| {
                    set(&mut img, exif, icc);
                    img.encoder().write_to(&mut out)
                })
        }
    };
    match written {
        Some(Ok(_)) => out,
        // Metadata is a nicety; the pixels are what was asked for.
        _ => encoded,
    }
}

/// Loads the watermark of `options`, if it has one with something to draw.
pub fn prepare_mark(options: &Options) -> Result<Option<Mark>> {
    match &options.watermark {
        Some(w) => watermark::prepare(w),
        None => Ok(None),
    }
}

/// Converts one file, with its look (turns and edit) and `mark` drawn on it. Returns
/// the new file's bytes and extension.
pub fn convert(
    source: &Path,
    look: &Look,
    options: &Options,
    mark: Option<&Mark>,
) -> Result<(Vec<u8>, &'static str)> {
    let bytes = std::fs::read(source)?;
    let decoded = decode(source, &bytes)?;
    let image = look.apply(decoded.image);
    let mut image = match options.max_edge {
        Some(edge) => resize(image, edge)?,
        None => image,
    };
    if let Some(mark) = mark {
        mark.apply(&mut image);
    }
    let format = target_format(options.format, source);
    let encoded = encode(&image, format, options.quality)?;
    let exif = decoded
        .exif
        .as_ref()
        .and_then(|e| copy_exif(e, options.remove_location));
    Ok((
        attach(encoded, format, exif, decoded.icc),
        extension(format),
    ))
}

/// A photo upright and shrunk, kept while watermark or edit settings change.
pub struct PreviewBase(DynamicImage);

impl PreviewBase {
    pub fn image(&self) -> &DynamicImage {
        &self.0
    }
}

/// The photo upright and shrunk to `max_edge`, for previewing a watermark on.
pub fn preview_base(source: &Path, look: &Look, max_edge: u32) -> Result<PreviewBase> {
    let bytes = std::fs::read(source)?;
    let image = look.apply(decode(source, &bytes)?.image);
    Ok(PreviewBase(resize(image, max_edge)?))
}

/// `base` with `watermark` drawn on it, as a JPEG.
pub fn preview(base: &PreviewBase, watermark: &Watermark) -> Result<Vec<u8>> {
    let mut image = base.0.clone();
    if let Some(mark) = watermark::prepare(watermark)? {
        mark.apply(&mut image);
    }
    crate::jpeg::encode(&image.to_rgb8(), 85, crate::jpeg::Purpose::Show)
}

/// Writes `data` as `<stem>.<ext>` in `folder`, or `<stem> (2).<ext>` and so on if
/// the name is taken; never replaces a file. `taken` holds names claimed by this
/// export (lower case) so parallel workers don't race for one.
fn write_new(
    folder: &Path,
    stem: &str,
    ext: &str,
    data: &[u8],
    taken: &Mutex<HashSet<String>>,
) -> Result<PathBuf> {
    for n in 1.. {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        if !taken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_lowercase())
        {
            continue;
        }
        let path = folder.join(&name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(data)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("some name is free")
}

/// Exports the photos of `asset_ids` into `options.folder`. Calls `progress(done,
/// total)` as it goes; stops early when `cancel` is set.
pub fn run(
    db: &Db,
    asset_ids: &[i64],
    options: &Options,
    cancel: &AtomicBool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Report> {
    if options.folder.as_os_str().is_empty() {
        return Err(Error::Invalid("Choose a folder to export to".into()));
    }
    let mark = prepare_mark(options)?;
    std::fs::create_dir_all(&options.folder)?;
    let mut report = Report {
        folder: options.folder.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut sources = Vec::new();
    for &id in asset_ids {
        match library::asset_source(&db.read(), id)? {
            Some(AssetSource::File {
                path, look, kind, ..
            }) if kind != 1 => sources.push((path, look)),
            Some(AssetSource::Unavailable { .. }) => {
                report.failed += 1;
                if report.errors.len() < MAX_ERRORS {
                    report
                        .errors
                        .push(format!("Item {id}: its drive isn't connected"));
                }
            }
            _ => report.skipped += 1, // videos, items without files
        }
    }

    let total = sources.len();
    let done = AtomicUsize::new(0);
    let taken = Mutex::new(HashSet::new());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(MAX_THREADS.min(std::thread::available_parallelism().map_or(2, |n| n.get())))
        .build()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    // (bytes in, bytes out) or what went wrong; None if cancelled first.
    let results: Vec<Option<std::result::Result<(u64, u64), String>>> = pool.install(|| {
        sources
            .par_iter()
            .map(|(source, look)| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let result = convert(source, look, options, mark.as_ref())
                    .and_then(|(data, ext)| {
                        let stem = source
                            .file_stem()
                            .map_or("photo".into(), |s| s.to_string_lossy().into_owned());
                        write_new(&options.folder, &stem, ext, &data, &taken)?;
                        let size_in = std::fs::metadata(source).map_or(0, |m| m.len());
                        Ok((size_in, data.len() as u64))
                    })
                    .map_err(|e| format!("{}: {e}", source.display()));
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                Some(result)
            })
            .collect()
    });
    for result in results {
        match result {
            None => report.skipped += 1,
            Some(Ok((size_in, size_out))) => {
                report.exported += 1;
                report.bytes_in += size_in;
                report.bytes_out += size_out;
            }
            Some(Err(why)) => {
                report.failed += 1;
                if report.errors.len() < MAX_ERRORS {
                    report.errors.push(why);
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use image::codecs::jpeg::JpegEncoder;

    /// A 600×400 JPEG with EXIF: a capture date, GPS and orientation 6 (turn right).
    fn photo_with_exif(dir: &Path) -> PathBuf {
        use exif::{Field, Rational};
        let img = RgbImage::from_fn(600, 400, |x, y| Rgb([(x / 3) as u8, (y / 2) as u8, 90]));
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&img)
            .unwrap();

        let fields = [
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"2023:06:14 12:30:00".to_vec()]),
            },
            Field {
                tag: Tag::Orientation,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![6]),
            },
            Field {
                tag: Tag::GPSLatitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![
                    Rational { num: 31, denom: 1 },
                    Rational { num: 31, denom: 1 },
                    Rational { num: 0, denom: 1 },
                ]),
            },
        ];
        let mut writer = exif::experimental::Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut raw = Cursor::new(Vec::new());
        writer.write(&mut raw, false).unwrap();
        let mut parts = img_parts::jpeg::Jpeg::from_bytes(jpeg.into()).unwrap();
        parts.set_exif(Some(raw.into_inner().into()));
        let path = dir.join("IMG_0001.jpg");
        let mut out = std::fs::File::create(&path).unwrap();
        parts.encoder().write_to(&mut out).unwrap();
        path
    }

    fn exif_of(bytes: &[u8]) -> Exif {
        exif::Reader::new()
            .read_from_container(&mut Cursor::new(bytes))
            .expect("exported file has EXIF")
    }

    #[test]
    fn resizes_turns_upright_and_keeps_the_date() {
        let dir = tempfile::tempdir().unwrap();
        let source = photo_with_exif(dir.path());
        let options = Options {
            max_edge: Some(300),
            ..Default::default()
        };
        let (data, ext) = convert(&source, &Look::default(), &options, None).unwrap();
        assert_eq!(ext, "jpg");
        let img = image::load_from_memory(&data).unwrap();
        // 600×400 turned on its side, then the long edge cut to 300.
        assert_eq!((img.width(), img.height()), (200, 300));
        let exif = exif_of(&data);
        assert!(exif.get_field(Tag::DateTimeOriginal, In::PRIMARY).is_some());
        assert!(
            exif.get_field(Tag::Orientation, In::PRIMARY).is_none(),
            "already applied"
        );
        assert!(exif.get_field(Tag::GPSLatitude, In::PRIMARY).is_some());
    }

    #[test]
    fn converts_and_removes_location_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let source = photo_with_exif(dir.path());
        for (format, ext) in [(Format::Webp, "webp"), (Format::Png, "png")] {
            let options = Options {
                format,
                remove_location: true,
                ..Default::default()
            };
            let (data, got) = convert(&source, &Look::default(), &options, None).unwrap();
            assert_eq!(got, ext);
            assert_eq!(image::guess_format(&data).unwrap().extensions_str()[0], ext);
            let exif = exif_of(&data);
            assert!(
                exif.get_field(Tag::DateTimeOriginal, In::PRIMARY).is_some(),
                "{ext}"
            );
            assert!(
                exif.get_field(Tag::GPSLatitude, In::PRIMARY).is_none(),
                "{ext}"
            );
        }
    }

    #[test]
    fn lower_quality_is_smaller_and_names_never_clash() {
        let dir = tempfile::tempdir().unwrap();
        let source = photo_with_exif(dir.path());
        let size = |quality| {
            let options = Options {
                quality,
                ..Default::default()
            };
            convert(&source, &Look::default(), &options, None)
                .unwrap()
                .0
                .len()
        };
        assert!(size(40) < size(95));

        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("IMG_0001.jpg"), b"already here").unwrap();
        let taken = Mutex::new(HashSet::new());
        let first = write_new(&out, "IMG_0001", "jpg", b"a", &taken).unwrap();
        let second = write_new(&out, "IMG_0001", "jpg", b"b", &taken).unwrap();
        assert!(first.ends_with("IMG_0001 (2).jpg"), "{first:?}");
        assert!(second.ends_with("IMG_0001 (3).jpg"), "{second:?}");
        assert_eq!(
            std::fs::read(out.join("IMG_0001.jpg")).unwrap(),
            b"already here"
        );
    }
}
