//! Memory slideshows and saving them as videos.
//!
//! [`pick`] chooses and orders the photos: near-duplicates are left out (only the best
//! of each similar group plays), a long selection is cut to its best shots, and what's
//! left plays in the order it was taken.
//!
//! [`make`] renders the slideshow as a 1080p, 30 fps H.264 MP4 with Windows' own
//! encoder (Media Foundation; no ffmpeg), with music from the user's file mixed in as
//! AAC: each photo slowly zooms and pans, photos crossfade, and the whole video fades
//! in and out. Photos whose shape differs a lot from the frame (portraits in a
//! landscape video) show whole over a blurred copy of themselves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::RgbImage;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::library::{self, AssetSource};
use crate::{Error, Result};

/// Most photos a slideshow plays; longer selections keep their best shots.
pub const MAX_PHOTOS: usize = 150;
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: u32 = 30;
/// Crossfade between photos, seconds (at most a third of a photo's time).
const TRANSITION: f32 = 1.0;
/// Fade in from black at the start, out at the end, seconds.
const FADE_IN: f32 = 0.6;
const FADE_OUT: f32 = 1.2;
/// Music fades out over the last seconds.
const MUSIC_FADE: f32 = 2.5;
const SAMPLE_RATE: u32 = 48_000;
/// Frames rendered at once (in parallel) before they're encoded.
const BATCH: usize = 24;

/// Photos to play from `ids`: no videos, one shot per group of near-duplicates, at
/// most `max` (the best), in the order they were taken.
pub fn pick(conn: &Connection, ids: &[i64], max: usize) -> Result<Vec<i64>> {
    struct Shot {
        id: i64,
        taken_at: i64,
        score: f64,
        group: Option<i64>,
    }
    let mut stmt = conn.prepare_cached(
        "SELECT a.taken_at, COALESCE(u.favorite, 0), COALESCE(u.rating, 0), a.focus, a.exposure,
                sm.group_id
         FROM assets a LEFT JOIN user_meta u ON u.asset_id = a.id
         LEFT JOIN similar_members sm ON sm.asset_id = a.id
         WHERE a.id = ?1 AND a.media_kind != 1 AND a.status = 0",
    )?;
    let mut shots: Vec<Shot> = Vec::new();
    for &id in ids {
        let row = stmt
            .query_row(params![id], |r| {
                let favorite: bool = r.get(1)?;
                let rating: i64 = r.get(2)?;
                let focus: Option<f64> = r.get(3)?;
                let exposure: Option<f64> = r.get(4)?;
                Ok(Shot {
                    id,
                    taken_at: r.get(0)?,
                    score: favorite as i64 as f64 * 10.0
                        + rating as f64 * 2.0
                        + focus.unwrap_or(0.0).max(0.0).ln_1p()
                        + exposure.unwrap_or(0.5) * 2.0,
                    group: r.get(5)?,
                })
            })
            .optional()?;
        shots.extend(row);
    }
    // One per group of near-duplicates: the best.
    let mut best: HashMap<i64, usize> = HashMap::new();
    let mut keep = vec![true; shots.len()];
    for (i, s) in shots.iter().enumerate() {
        let Some(g) = s.group else { continue };
        match best.get(&g) {
            Some(&j) if shots[j].score >= s.score => keep[i] = false,
            Some(&j) => {
                keep[j] = false;
                best.insert(g, i);
            }
            None => {
                best.insert(g, i);
            }
        }
    }
    let mut kept: Vec<Shot> = shots
        .into_iter()
        .zip(keep)
        .filter_map(|(s, k)| k.then_some(s))
        .collect();
    if kept.len() > max {
        kept.sort_by(|a, b| b.score.total_cmp(&a.score));
        kept.truncate(max);
    }
    kept.sort_by_key(|s| (s.taken_at, s.id));
    Ok(kept.into_iter().map(|s| s.id).collect())
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// How long each photo shows, seconds.
    pub seconds_per_photo: f32,
    /// An audio file to play under the photos (looped or cut to fit).
    pub music: Option<PathBuf>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub path: PathBuf,
    pub photos: usize,
    pub seconds: f32,
    /// Music was asked for but couldn't be read; the video has none.
    pub music_skipped: bool,
}

/// A photo ready to film: big enough to zoom into, and how it's framed.
struct Prepared {
    image: RgbImage,
    /// For photos shown whole: a blurred, darkened copy filling the frame behind them.
    backdrop: Option<RgbImage>,
}

fn invalid(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("Rendering failed: {e}"))
}

/// `src` cropped to (`x`, `y`, `w`, `h`) and scaled to `tw`×`th` (bilinear: fast enough
/// for every frame).
fn crop_resize(
    src: &RgbImage,
    crop: (f64, f64, f64, f64),
    tw: u32,
    th: u32,
    smooth: bool,
) -> Result<RgbImage> {
    let view =
        ImageRef::new(src.width(), src.height(), src.as_raw(), PixelType::U8x3).map_err(invalid)?;
    let mut dst = Image::new(tw, th, PixelType::U8x3);
    let filter = if smooth {
        FilterType::Lanczos3
    } else {
        FilterType::Bilinear
    };
    let options = ResizeOptions::new()
        .crop(crop.0, crop.1, crop.2, crop.3)
        .resize_alg(ResizeAlg::Convolution(filter));
    Resizer::new()
        .resize(&view, &mut dst, &options)
        .map_err(invalid)?;
    Ok(RgbImage::from_raw(tw, th, dst.into_vec()).expect("sized buffer"))
}

/// Photos this much narrower (or wider) than the frame are shown whole.
fn shown_whole(iw: u32, ih: u32) -> bool {
    let aspect = iw as f32 / ih as f32;
    let frame = WIDTH as f32 / HEIGHT as f32;
    aspect < frame * 0.72 || aspect > frame * 1.6
}

fn prepare(source: &Path, look: &crate::edit::Look) -> Result<Prepared> {
    let base = crate::export::preview_base(source, look, 3000)?;
    let image = base.image().to_rgb8();
    let backdrop = if shown_whole(image.width(), image.height()) {
        // Cover crop, shrunk, blurred, darkened, then scaled back up.
        let (x, y, w, h) = cover(image.width() as f64, image.height() as f64, 1.0, 0.5, 0.5);
        let small = crop_resize(&image, (x, y, w, h), WIDTH / 8, HEIGHT / 8, true)?;
        let mut soft = image::imageops::blur(&small, 6.0);
        for p in soft.pixels_mut() {
            for c in &mut p.0 {
                *c = (*c as f32 * 0.55) as u8;
            }
        }
        let (sw, sh) = (soft.width() as f64, soft.height() as f64);
        Some(crop_resize(
            &soft,
            (0.0, 0.0, sw, sh),
            WIDTH,
            HEIGHT,
            false,
        )?)
    } else {
        None
    };
    Ok(Prepared { image, backdrop })
}

/// The part of an `iw`×`ih` photo that fills the frame at `zoom`, its centre at
/// (`u`, `v`) of the room it has to move in (0.5, 0.5 is centred).
fn cover(iw: f64, ih: f64, zoom: f64, u: f64, v: f64) -> (f64, f64, f64, f64) {
    let frame = WIDTH as f64 / HEIGHT as f64;
    let (cw, ch) = if iw / ih > frame {
        (ih * frame, ih)
    } else {
        (iw, iw / frame)
    };
    let (cw, ch) = (cw / zoom, ch / zoom);
    ((iw - cw) * u, (ih - ch) * v, cw, ch)
}

/// How the camera moves over photo `i` as `p` goes from 0 to 1: alternately zooming
/// in and out, drifting a different way each time.
fn motion(i: usize, p: f64) -> (f64, f64, f64) {
    let p = p.clamp(0.0, 1.0);
    let (z0, z1) = if i.is_multiple_of(2) {
        (1.0, 1.12)
    } else {
        (1.12, 1.0)
    };
    let ((u0, v0), (u1, v1)) = match i % 4 {
        0 => ((0.35, 0.5), (0.65, 0.5)),
        1 => ((0.5, 0.35), (0.5, 0.6)),
        2 => ((0.65, 0.45), (0.35, 0.55)),
        _ => ((0.5, 0.5), (0.5, 0.5)),
    };
    (z0 + (z1 - z0) * p, u0 + (u1 - u0) * p, v0 + (v1 - v0) * p)
}

/// Photo `i` at progress `p` through its time on screen.
fn shot(photo: &Prepared, i: usize, p: f64) -> Result<RgbImage> {
    let (iw, ih) = (photo.image.width() as f64, photo.image.height() as f64);
    let (zoom, u, v) = motion(i, p);
    match &photo.backdrop {
        None => crop_resize(
            &photo.image,
            cover(iw, ih, zoom, u, v),
            WIDTH,
            HEIGHT,
            false,
        ),
        Some(backdrop) => {
            // Whole, slowly growing, centred over the backdrop.
            let fit =
                (WIDTH as f64 / iw).min(HEIGHT as f64 / ih) * (0.9 + 0.06 * p.clamp(0.0, 1.0));
            let (fw, fh) = (((iw * fit) as u32).max(1), ((ih * fit) as u32).max(1));
            let front = crop_resize(&photo.image, (0.0, 0.0, iw, ih), fw, fh, false)?;
            let mut frame = backdrop.clone();
            image::imageops::replace(
                &mut frame,
                &front,
                ((WIDTH - fw) / 2) as i64,
                ((HEIGHT - fh) / 2) as i64,
            );
            Ok(frame)
        }
    }
}

fn mix(a: &mut RgbImage, b: &RgbImage, w: f32) {
    let w = (w.clamp(0.0, 1.0) * 256.0) as u32;
    a.par_chunks_mut(WIDTH as usize * 3)
        .zip(b.par_chunks(WIDTH as usize * 3))
        .for_each(|(ra, rb)| {
            for (x, y) in ra.iter_mut().zip(rb) {
                *x = ((*x as u32 * (256 - w) + *y as u32 * w) >> 8) as u8;
            }
        });
}

fn dim(a: &mut RgbImage, k: f32) {
    let k = (k.clamp(0.0, 1.0) * 256.0) as u32;
    if k >= 256 {
        return;
    }
    a.par_chunks_mut(WIDTH as usize * 3).for_each(|row| {
        for x in row {
            *x = ((*x as u32 * k) >> 8) as u8;
        }
    });
}

/// When each photo shows: photo `i` owns `[i * d, (i + 1) * d)` and fades in during
/// the last `t` seconds of the one before.
struct Timing {
    photos: usize,
    per: f32,
    transition: f32,
}

impl Timing {
    fn new(photos: usize, per: f32) -> Timing {
        let per = per.clamp(1.5, 15.0);
        Timing {
            photos,
            per,
            transition: TRANSITION.min(per / 3.0),
        }
    }

    fn seconds(&self) -> f32 {
        self.photos as f32 * self.per
    }

    fn frames(&self) -> usize {
        (self.seconds() * FPS as f32).round() as usize
    }

    /// The photos on screen at `t`: (index, progress, weight of the next one).
    fn at(&self, t: f32) -> (usize, f64, Option<(f64, f32)>) {
        let i = ((t / self.per) as usize).min(self.photos - 1);
        let progress = |i: usize| {
            let start = i as f32 * self.per - if i == 0 { 0.0 } else { self.transition };
            let span = self.per + if i == 0 { 0.0 } else { self.transition };
            ((t - start) / span) as f64
        };
        let fade_from = (i + 1) as f32 * self.per - self.transition;
        let next = (i + 1 < self.photos && t >= fade_from)
            .then(|| (progress(i + 1), (t - fade_from) / self.transition));
        (i, progress(i), next)
    }

    /// Fade from and to black.
    fn brightness(&self, t: f32) -> f32 {
        (t / FADE_IN)
            .min((self.seconds() - t) / FADE_OUT)
            .clamp(0.0, 1.0)
    }
}

/// Renders frame `f`.
fn frame(timing: &Timing, photos: &HashMap<usize, Arc<Prepared>>, f: usize) -> Result<RgbImage> {
    let t = f as f32 / FPS as f32;
    let (i, p, next) = timing.at(t);
    let mut out = shot(&photos[&i], i, p)?;
    if let Some((pn, w)) = next {
        let other = shot(&photos[&(i + 1)], i + 1, pn)?;
        mix(&mut out, &other, w);
    }
    dim(&mut out, timing.brightness(t));
    Ok(out)
}

/// Saves photos `ids` (in this order) as an MP4 at `path`. Calls `progress(done,
/// total)` in frames; stops when `cancel` is set, removing the partial file.
pub fn make(
    db: &Db,
    ids: &[i64],
    options: &Options,
    path: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(usize, usize),
) -> Result<Report> {
    if ids.is_empty() {
        return Err(Error::Invalid(
            "There are no photos to put in the video".into(),
        ));
    }
    let sources: Vec<(PathBuf, crate::edit::Look)> = {
        let conn = db.read();
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(AssetSource::File {
                path, look, kind, ..
            }) = library::asset_source(&conn, id)?
                && kind != 1
            {
                out.push((path, look));
            }
        }
        out
    };
    if sources.is_empty() {
        return Err(Error::Invalid(
            "None of these photos can be read right now".into(),
        ));
    }
    let timing = Timing::new(sources.len(), options.seconds_per_photo);
    let total = timing.frames();
    let music = options
        .music
        .as_deref()
        .map(|m| encoder::decode_audio(m, timing.seconds()));
    let music_skipped = matches!(music, Some(Err(_)));
    let audio = music.and_then(|m| m.ok());

    let result = (|| -> Result<()> {
        let mut writer = encoder::Writer::create(path, WIDTH, HEIGHT, FPS, audio.is_some())?;
        let mut cache: HashMap<usize, Arc<Prepared>> = HashMap::new();
        let mut audio_written = 0usize;
        let mut done = 0;
        while done < total {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Invalid("Stopped".into()));
            }
            let batch: Vec<usize> = (done..(done + BATCH).min(total)).collect();
            // Photos this batch shows, prepared in parallel; others are let go.
            let needed: Vec<usize> = {
                let first = timing
                    .at(*batch.first().expect("batch") as f32 / FPS as f32)
                    .0;
                let last = timing
                    .at(*batch.last().expect("batch") as f32 / FPS as f32)
                    .0;
                (first..=(last + 1).min(timing.photos - 1)).collect()
            };
            cache.retain(|k, _| needed.contains(k));
            let missing: Vec<usize> = needed
                .iter()
                .copied()
                .filter(|k| !cache.contains_key(k))
                .collect();
            let prepared: Vec<(usize, Prepared)> = missing
                .par_iter()
                .map(|&k| Ok((k, prepare(&sources[k].0, &sources[k].1)?)))
                .collect::<Result<_>>()?;
            cache.extend(prepared.into_iter().map(|(k, p)| (k, Arc::new(p))));
            let frames: Vec<RgbImage> = batch
                .par_iter()
                .map(|&f| frame(&timing, &cache, f))
                .collect::<Result<_>>()?;
            for (offset, image) in frames.iter().enumerate() {
                writer.video(image, (done + offset) as u64)?;
            }
            done += frames.len();
            // Audio up to where the video is, so the file interleaves them.
            if let Some(samples) = &audio {
                let until = ((done as u64 * SAMPLE_RATE as u64 / FPS as u64) as usize * 2)
                    .min(samples.len());
                if until > audio_written {
                    writer.audio(&samples[audio_written..until], audio_written / 2)?;
                    audio_written = until;
                }
            }
            progress(done, total);
        }
        if let Some(samples) = &audio
            && audio_written < samples.len()
        {
            writer.audio(&samples[audio_written..], audio_written / 2)?;
        }
        writer.finish()
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(Report {
        path: path.to_owned(),
        photos: sources.len(),
        seconds: timing.seconds(),
        music_skipped,
    })
}

/// Stereo 16-bit samples at [`SAMPLE_RATE`], looped or cut to `seconds`, fading out.
fn fit_music(mut samples: Vec<i16>, rate: u32, channels: u32, seconds: f32) -> Vec<i16> {
    // To stereo.
    if channels == 1 {
        samples = samples.iter().flat_map(|&s| [s, s]).collect();
    } else if channels > 2 {
        samples = samples
            .chunks(channels as usize)
            .flat_map(|c| [c[0], c[1]])
            .collect();
    }
    // To the output rate (linear interpolation).
    if rate != SAMPLE_RATE && rate > 0 {
        let frames = samples.len() / 2;
        let out_frames = (frames as u64 * SAMPLE_RATE as u64 / rate as u64) as usize;
        let mut out = Vec::with_capacity(out_frames * 2);
        for o in 0..out_frames {
            let pos = o as f64 * rate as f64 / SAMPLE_RATE as f64;
            let i = pos as usize;
            let frac = pos - i as f64;
            for c in 0..2 {
                let a = samples[(i * 2 + c).min(samples.len() - 2 + c)] as f64;
                let b = samples[((i + 1) * 2 + c).min(samples.len() - 2 + c)] as f64;
                out.push((a + (b - a) * frac) as i16);
            }
        }
        samples = out;
    }
    let want = (seconds * SAMPLE_RATE as f32) as usize * 2;
    if samples.is_empty() {
        return vec![0; want];
    }
    let mut out = Vec::with_capacity(want);
    while out.len() < want {
        let take = (want - out.len()).min(samples.len());
        out.extend_from_slice(&samples[..take]);
    }
    let fade = (MUSIC_FADE * SAMPLE_RATE as f32) as usize * 2;
    let start = want.saturating_sub(fade);
    for (k, s) in out[start..].iter_mut().enumerate() {
        let gain = 1.0 - k as f32 / fade.max(1) as f32;
        *s = (*s as f32 * gain) as i16;
    }
    out
}

/// BT.709 limited-range NV12 (what H.264 encoders take): a luma plane, then
/// interleaved chroma at half resolution.
fn to_nv12(image: &RgbImage) -> Vec<u8> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut out = vec![0u8; w * h * 3 / 2];
    let (luma, chroma) = out.split_at_mut(w * h);
    let px = image.as_raw();
    luma.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, l) in row.iter_mut().enumerate() {
            let i = (y * w + x) * 3;
            let (r, g, b) = (px[i] as i32, px[i + 1] as i32, px[i + 2] as i32);
            *l = (((47 * r + 157 * g + 16 * b) >> 8) + 16).clamp(0, 255) as u8;
        }
    });
    chroma.par_chunks_mut(w).enumerate().for_each(|(cy, row)| {
        for cx in 0..w / 2 {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((cy * 2 + dy) * w + cx * 2 + dx) * 3;
                r += px[i] as i32;
                g += px[i + 1] as i32;
                b += px[i + 2] as i32;
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            row[cx * 2] = (((-26 * r - 86 * g + 112 * b) >> 8) + 128).clamp(0, 255) as u8;
            row[cx * 2 + 1] = (((112 * r - 102 * g - 10 * b) >> 8) + 128).clamp(0, 255) as u8;
        }
    });
    out
}

#[cfg(windows)]
mod encoder {
    //! Media Foundation: a sink writer for the MP4, a source reader for the music.

    use std::path::Path;

    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::{GUID, HSTRING};

    use super::{SAMPLE_RATE, fit_music, to_nv12};
    use crate::{Error, Result};

    fn mf_error(e: windows::core::Error) -> Error {
        Error::Invalid(format!("Windows couldn't make the video: {}", e.message()))
    }

    /// Starts Media Foundation for this thread (and COM, which it needs).
    fn startup() -> Result<()> {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(mf_error)
    }

    fn media_type(major: &GUID, subtype: &GUID) -> Result<IMFMediaType> {
        unsafe {
            let t = MFCreateMediaType().map_err(mf_error)?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, major).map_err(mf_error)?;
            t.SetGUID(&MF_MT_SUBTYPE, subtype).map_err(mf_error)?;
            Ok(t)
        }
    }

    fn pack(a: u32, b: u32) -> u64 {
        ((a as u64) << 32) | b as u64
    }

    fn set32(t: &IMFMediaType, key: &GUID, v: u32) -> Result<()> {
        unsafe { t.SetUINT32(key, v) }.map_err(mf_error)
    }

    fn video_type(subtype: &GUID, w: u32, h: u32, fps: u32) -> Result<IMFMediaType> {
        let t = media_type(&MFMediaType_Video, subtype)?;
        unsafe {
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))
                .map_err(mf_error)?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))
                .map_err(mf_error)?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
                .map_err(mf_error)?;
        }
        set32(
            &t,
            &MF_MT_INTERLACE_MODE,
            MFVideoInterlace_Progressive.0 as u32,
        )?;
        Ok(t)
    }

    fn audio_type(subtype: &GUID, bytes_per_second: u32) -> Result<IMFMediaType> {
        let t = media_type(&MFMediaType_Audio, subtype)?;
        set32(&t, &MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        set32(&t, &MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        set32(&t, &MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        set32(&t, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_second)?;
        Ok(t)
    }

    fn sample(bytes: &[u8], time: i64, duration: i64) -> Result<IMFSample> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32).map_err(mf_error)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None).map_err(mf_error)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            buffer.Unlock().map_err(mf_error)?;
            buffer
                .SetCurrentLength(bytes.len() as u32)
                .map_err(mf_error)?;
            let s = MFCreateSample().map_err(mf_error)?;
            s.AddBuffer(&buffer).map_err(mf_error)?;
            s.SetSampleTime(time).map_err(mf_error)?;
            s.SetSampleDuration(duration).map_err(mf_error)?;
            Ok(s)
        }
    }

    pub struct Writer {
        sink: IMFSinkWriter,
        video: u32,
        audio: Option<u32>,
        fps: u32,
    }

    impl Writer {
        pub fn create(path: &Path, w: u32, h: u32, fps: u32, with_audio: bool) -> Result<Writer> {
            startup()?;
            unsafe {
                let mut attrs: Option<IMFAttributes> = None;
                MFCreateAttributes(&mut attrs, 2).map_err(mf_error)?;
                let attrs = attrs.expect("created");
                attrs
                    .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                    .map_err(mf_error)?;
                let sink =
                    MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attrs)
                        .map_err(mf_error)?;

                let out = video_type(&MFVideoFormat_H264, w, h, fps)?;
                set32(&out, &MF_MT_AVG_BITRATE, 12_000_000)?;
                // Main profile plays everywhere.
                set32(&out, &MF_MT_MPEG2_PROFILE, 77)?;
                let video = sink.AddStream(&out).map_err(mf_error)?;
                let input = video_type(&MFVideoFormat_NV12, w, h, fps)?;
                set32(&input, &MF_MT_DEFAULT_STRIDE, w)?;
                sink.SetInputMediaType(video, &input, None)
                    .map_err(mf_error)?;

                let audio = if with_audio {
                    let out = audio_type(&MFAudioFormat_AAC, 24_000)?;
                    let index = sink.AddStream(&out).map_err(mf_error)?;
                    let input = audio_type(&MFAudioFormat_PCM, SAMPLE_RATE * 4)?;
                    set32(&input, &MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                    sink.SetInputMediaType(index, &input, None)
                        .map_err(mf_error)?;
                    Some(index)
                } else {
                    None
                };
                sink.BeginWriting().map_err(mf_error)?;
                Ok(Writer {
                    sink,
                    video,
                    audio,
                    fps,
                })
            }
        }

        /// Frame number `n`.
        pub fn video(&mut self, image: &image::RgbImage, n: u64) -> Result<()> {
            let duration = 10_000_000 / self.fps as i64;
            let s = sample(&to_nv12(image), n as i64 * duration, duration)?;
            unsafe { self.sink.WriteSample(self.video, &s) }.map_err(mf_error)
        }

        /// Interleaved stereo samples starting at sample frame `start`.
        pub fn audio(&mut self, samples: &[i16], start: usize) -> Result<()> {
            let Some(index) = self.audio else {
                return Ok(());
            };
            // In pieces of about a tenth of a second.
            for (k, chunk) in samples.chunks(SAMPLE_RATE as usize / 10 * 2).enumerate() {
                let first = start + k * SAMPLE_RATE as usize / 10;
                let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                let time = first as i64 * 10_000_000 / SAMPLE_RATE as i64;
                let duration = (chunk.len() / 2) as i64 * 10_000_000 / SAMPLE_RATE as i64;
                let s = sample(&bytes, time, duration)?;
                unsafe { self.sink.WriteSample(index, &s) }.map_err(mf_error)?;
            }
            Ok(())
        }

        pub fn finish(self) -> Result<()> {
            unsafe { self.sink.Finalize() }.map_err(mf_error)
        }
    }

    /// The audio of `path` as 16-bit stereo at the output rate, fitted to `seconds`.
    pub fn decode_audio(path: &Path, seconds: f32) -> Result<Vec<i16>> {
        startup()?;
        unsafe {
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)
                .map_err(mf_error)?;
            let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(mf_error)?;
            reader.SetStreamSelection(stream, true).map_err(mf_error)?;
            let want = media_type(&MFMediaType_Audio, &MFAudioFormat_PCM)?;
            set32(&want, &MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            reader
                .SetCurrentMediaType(stream, None, &want)
                .map_err(mf_error)?;
            let got = reader.GetCurrentMediaType(stream).map_err(mf_error)?;
            let rate = got
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .map_err(mf_error)?;
            let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(mf_error)?;
            // Enough to fill the video (and no more than ten minutes' worth).
            let limit = (seconds.min(600.0) as usize + 1) * rate as usize * channels as usize;
            let mut samples: Vec<i16> = Vec::new();
            while samples.len() < limit {
                let mut flags = 0u32;
                let mut s: Option<IMFSample> = None;
                reader
                    .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut s))
                    .map_err(mf_error)?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(s) = s else { continue };
                let buffer = s.ConvertToContiguousBuffer().map_err(mf_error)?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut len = 0u32;
                buffer
                    .Lock(&mut ptr, None, Some(&mut len))
                    .map_err(mf_error)?;
                let bytes = std::slice::from_raw_parts(ptr, len as usize);
                samples.extend(
                    bytes
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]])),
                );
                buffer.Unlock().map_err(mf_error)?;
            }
            if samples.is_empty() {
                return Err(Error::Invalid("The music file has no sound in it".into()));
            }
            Ok(fit_music(samples, rate, channels, seconds))
        }
    }
}

#[cfg(not(windows))]
mod encoder {
    use std::path::Path;

    use crate::{Error, Result};

    fn unsupported() -> Error {
        Error::Invalid("Saving slideshows as videos works on Windows for now".into())
    }

    pub struct Writer;

    impl Writer {
        pub fn create(_: &Path, _: u32, _: u32, _: u32, _: bool) -> Result<Writer> {
            Err(unsupported())
        }
        pub fn video(&mut self, _: &image::RgbImage, _: u64) -> Result<()> {
            Err(unsupported())
        }
        pub fn audio(&mut self, _: &[i16], _: usize) -> Result<()> {
            Err(unsupported())
        }
        pub fn finish(self) -> Result<()> {
            Err(unsupported())
        }
    }

    pub fn decode_audio(_: &Path, _: f32) -> Result<Vec<i16>> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_crossfades_and_fades_to_black() {
        let t = Timing::new(3, 4.0);
        assert_eq!(t.frames(), 360);
        assert_eq!(t.at(0.0).0, 0);
        assert!(t.at(2.0).2.is_none());
        let (i, _, next) = t.at(3.5);
        assert_eq!(i, 0);
        let (p_next, w) = next.unwrap();
        assert!((w - 0.5).abs() < 1e-6 && p_next > 0.0);
        assert_eq!(t.at(11.9).0, 2);
        assert!(t.at(11.9).2.is_none(), "nothing after the last photo");
        assert_eq!(t.brightness(0.0), 0.0);
        assert_eq!(t.brightness(6.0), 1.0);
        assert!(t.brightness(11.9) < 0.2);
    }

    #[test]
    fn camera_stays_inside_the_photo() {
        for i in 0..4 {
            for p in [0.0, 0.5, 1.0] {
                let (z, u, v) = motion(i, p);
                let (x, y, w, h) = cover(4000.0, 3000.0, z, u, v);
                assert!(x >= 0.0 && y >= 0.0 && x + w <= 4000.0 + 1e-6 && y + h <= 3000.0 + 1e-6);
                assert!((w / h - 16.0 / 9.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn music_is_looped_resampled_and_faded() {
        let mono: Vec<i16> = (0..22_050)
            .map(|i| if i % 2 == 0 { 1000 } else { -1000 })
            .collect();
        let out = fit_music(mono, 22_050, 1, 3.0);
        assert_eq!(out.len(), 3 * SAMPLE_RATE as usize * 2);
        assert!(out[1000].abs() > 0, "sound at the start");
        assert_eq!(*out.last().unwrap(), 0, "faded out");
    }

    #[test]
    fn nv12_has_the_right_size_and_levels() {
        let white = RgbImage::from_pixel(4, 2, image::Rgb([255, 255, 255]));
        let nv = to_nv12(&white);
        assert_eq!(nv.len(), 12);
        assert_eq!(nv[0], 235, "limited-range white");
        assert_eq!((nv[8], nv[9]), (128, 128), "no colour");
    }

    #[test]
    fn picks_one_of_each_near_duplicate_group_in_time_order() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate_library(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at, focus) VALUES
               (1, 0, 30, 0, 0, 0, 5.0), (2, 0, 10, 0, 0, 0, 50.0), (3, 0, 20, 0, 0, 0, 1.0),
               (4, 1, 5, 0, 0, 0, NULL), (5, 0, 40, 0, 0, 0, 1.0);
             INSERT INTO similar_members (asset_id, group_id) VALUES (1, 1), (2, 1);
             INSERT INTO user_meta (asset_id, favorite) VALUES (5, 1);",
        )
        .unwrap();
        assert_eq!(pick(&conn, &[1, 2, 3, 4, 5], 10).unwrap(), vec![2, 3, 5]);
        // Cut to the best two, still in time order.
        assert_eq!(pick(&conn, &[1, 2, 3, 4, 5], 2).unwrap(), vec![2, 5]);
    }

    #[cfg(windows)]
    #[test]
    fn saves_an_mp4_with_music() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let db = Db::open(
            &dir.path().join("lib.sqlite"),
            1,
            crate::schema::migrate_library,
        )
        .unwrap();
        let root = library::add_root(&db.write(), &photos).unwrap();
        for (id, w, h) in [(1i64, 640u32, 480u32), (2, 300, 600)] {
            let img = RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
            });
            let name = format!("{id}.jpg");
            std::fs::write(
                photos.join(&name),
                crate::jpeg::encode(&img, 90, crate::jpeg::Purpose::Keep).unwrap(),
            )
            .unwrap();
            let conn = db.write();
            conn.execute(
                "INSERT INTO assets (id, media_kind, taken_at, taken_src, local_day, created_at) VALUES (?1, 0, 0, 0, 0, 0)",
                params![id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO files (root_id, rel_path, size, mtime, asset_id) VALUES (?1, ?2, 1, 0, ?3)",
                params![root, name, id],
            )
            .unwrap();
        }
        // A one-second 440 Hz tone as a WAV file.
        let wav = dir.path().join("tone.wav");
        let tone: Vec<i16> = (0..44_100)
            .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 8000.0) as i16)
            .collect();
        let mut bytes = Vec::new();
        let data_len = tone.len() as u32 * 2;
        bytes.extend(b"RIFF");
        bytes.extend((36 + data_len).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(44_100u32.to_le_bytes());
        bytes.extend((44_100u32 * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data_len.to_le_bytes());
        bytes.extend(tone.iter().flat_map(|s| s.to_le_bytes()));
        std::fs::write(&wav, bytes).unwrap();

        let out = dir.path().join("memories.mp4");
        let cancel = AtomicBool::new(false);
        let last = std::sync::Mutex::new((0, 0));
        let report = make(
            &db,
            &[1, 2],
            &Options {
                seconds_per_photo: 1.5,
                music: Some(wav),
            },
            &out,
            &cancel,
            &|d, t| *last.lock().unwrap() = (d, t),
        )
        .unwrap();
        assert!(!report.music_skipped);
        assert_eq!(report.photos, 2);
        assert_eq!(*last.lock().unwrap(), (90, 90));
        let data = std::fs::read(&out).unwrap();
        assert_eq!(&data[4..8], b"ftyp");
        assert!(data.len() > 10_000, "{} bytes", data.len());
        // It reads back as a video of the right length.
        let read = crate::video::read(&out).unwrap();
        assert_eq!((read.width, read.height), (Some(1920), Some(1080)));
        assert!(
            read.duration_ms.is_some_and(|d| (2900..=3100).contains(&d)),
            "{:?}",
            read.duration_ms
        );

        // Stopping removes the partial file.
        cancel.store(true, Ordering::SeqCst);
        let stopped = dir.path().join("stopped.mp4");
        let none = Options {
            seconds_per_photo: 2.0,
            music: None,
        };
        assert!(make(&db, &[1, 2], &none, &stopped, &cancel, &|_, _| {}).is_err());
        assert!(!stopped.exists());
    }
}
