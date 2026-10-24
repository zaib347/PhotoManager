//! Thumbnails from the operating system's own decoders, for formats this crate can't
//! decode itself: video poster frames, and HEIC/RAW where the OS has a codec.
//!
//! - Windows: the shell thumbnail cache (what Explorer shows), via
//!   `IShellItemImageFactory`. Covers every format with an installed codec.
//! - Elsewhere: a system `ffmpeg`, if one is on the PATH, for video frames.
//!   Native macOS (QuickLook) support is part of platform hardening (M5).

use std::path::Path;

use image::RgbImage;

use crate::{Error, Result};

/// Returns an upright RGB image whose longer edge is roughly `size` pixels.
pub fn thumbnail(path: &Path, size: u32) -> Result<RgbImage> {
    #[cfg(windows)]
    {
        match windows_shell::thumbnail(path, size) {
            Ok(img) => return Ok(img),
            Err(e) if !ffmpeg_available() => return Err(e),
            Err(_) => {}
        }
    }
    ffmpeg_frame(path, size)
}

/// Decodes a whole image at full resolution with the OS's codecs (HEIC and other
/// formats this crate can't read). Windows only for now (WIC).
pub fn full_image(path: &Path) -> Result<image::DynamicImage> {
    #[cfg(windows)]
    {
        windows_wic::decode(path).map(image::DynamicImage::ImageRgb8)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(Error::Invalid(
            "This format can't be decoded on this system yet".into(),
        ))
    }
}

#[cfg(windows)]
mod windows_wic {
    use std::path::Path;

    use image::RgbImage;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICImagingFactory, IWICPalette,
        WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::HSTRING;

    use crate::{Error, Result};

    fn os_error(e: windows::core::Error) -> Error {
        Error::Invalid(format!("Windows couldn't decode this image: {e}"))
    }

    /// First frame as 24-bit RGB.
    pub fn decode(path: &Path) -> Result<RgbImage> {
        super::windows_shell::ensure_com();
        let path = HSTRING::from(std::path::absolute(path)?.as_os_str());
        // SAFETY: plain COM calls on interfaces we own; the buffer is sized from the
        // frame's own dimensions and stride.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(os_error)?;
            let decoder = factory
                .CreateDecoderFromFilename(
                    &path,
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(os_error)?;
            let frame = decoder.GetFrame(0).map_err(os_error)?;
            let converter = factory.CreateFormatConverter().map_err(os_error)?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat24bppRGB,
                    WICBitmapDitherTypeNone,
                    None::<&IWICPalette>,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(os_error)?;
            let (mut w, mut h) = (0u32, 0u32);
            converter.GetSize(&mut w, &mut h).map_err(os_error)?;
            let stride = w * 3;
            let mut buf = vec![0u8; stride as usize * h as usize];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut buf)
                .map_err(os_error)?;
            RgbImage::from_raw(w, h, buf).ok_or_else(|| Error::Invalid("Bad image size".into()))
        }
    }
}

fn ffmpeg_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// One frame about a second in (skipping black lead-in frames) as JPEG via ffmpeg.
fn ffmpeg_frame(path: &Path, size: u32) -> Result<RgbImage> {
    if !ffmpeg_available() {
        return Err(Error::Invalid("No video thumbnailer available".into()));
    }
    let output = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-ss", "1", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale='min({size},iw)':-2"),
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "-",
        ])
        .stdin(std::process::Stdio::null())
        .output()?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(Error::Invalid("ffmpeg could not read a frame".into()));
    }
    Ok(image::load_from_memory(&output.stdout)?.to_rgb8())
}

#[cfg(windows)]
mod windows_shell {
    use std::cell::Cell;
    use std::path::Path;

    use image::RgbImage;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
        SIIGBF_THUMBNAILONLY,
    };
    use windows::core::HSTRING;

    use crate::{Error, Result};

    thread_local! {
        static COM_READY: Cell<bool> = const { Cell::new(false) };
    }

    /// Shell thumbnail providers (and WIC) expect COM on the calling thread.
    pub(super) fn ensure_com() {
        COM_READY.with(|ready| {
            if !ready.get() {
                // SAFETY: initializes COM for this thread; never uninitialized because the
                // thread is a long-lived pool worker.
                let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                ready.set(true);
            }
        });
    }

    fn os_error(e: windows::core::Error) -> Error {
        Error::Invalid(format!("Windows thumbnail failed: {e}"))
    }

    pub fn thumbnail(path: &Path, size: u32) -> Result<RgbImage> {
        ensure_com();
        let path = HSTRING::from(std::path::absolute(path)?.as_os_str());
        // SAFETY: plain COM calls; the HBITMAP is owned by us and freed below.
        unsafe {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(&path, None).map_err(os_error)?;
            let size = SIZE {
                cx: size as i32,
                cy: size as i32,
            };
            // THUMBNAILONLY: never fall back to the file-type icon.
            let bitmap = factory
                .GetImage(size, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK)
                .map_err(os_error)?;
            let result = read_bitmap(bitmap);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            result
        }
    }

    /// Copies a GDI bitmap out as RGB (it arrives as 32-bit BGRA).
    unsafe fn read_bitmap(bitmap: HBITMAP) -> Result<RgbImage> {
        let mut info = BITMAP::default();
        // SAFETY: `info` is a valid BITMAP buffer of the size passed.
        let got = unsafe {
            GetObjectW(
                HGDIOBJ(bitmap.0),
                size_of::<BITMAP>() as i32,
                Some((&mut info as *mut BITMAP).cast()),
            )
        };
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        if got == 0 || w <= 0 || h <= 0 {
            return Err(Error::Invalid("Empty thumbnail bitmap".into()));
        }

        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: rows top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: the buffer holds exactly h rows of w 32-bit pixels, as requested.
        let rows = unsafe {
            let dc = CreateCompatibleDC(None);
            let rows = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                Some(bgra.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            let _ = DeleteDC(dc);
            rows
        };
        if rows == 0 {
            return Err(Error::Invalid("Could not read thumbnail pixels".into()));
        }

        let rgb = bgra
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        RgbImage::from_raw(w as u32, h as u32, rgb)
            .ok_or_else(|| Error::Invalid("Bad thumbnail size".into()))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use image::{Rgb, RgbImage};

    #[test]
    fn windows_shell_thumbnails_a_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("red.png");
        RgbImage::from_pixel(400, 200, Rgb([200, 10, 10]))
            .save(&path)
            .unwrap();

        let thumb = super::thumbnail(&path, 128).unwrap();
        assert!(
            thumb.width() <= 400 && thumb.width() >= 64,
            "{}",
            thumb.width()
        );
        let px = thumb.get_pixel(thumb.width() / 2, thumb.height() / 2);
        assert!(px[0] > 150 && px[1] < 60, "should be red, got {px:?}");
    }

    #[test]
    fn windows_codecs_decode_at_full_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.png");
        RgbImage::from_pixel(1500, 900, Rgb([10, 200, 10]))
            .save(&path)
            .unwrap();
        let img = super::full_image(&path).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (1500, 900));
        assert_eq!(img.get_pixel(700, 400).0, [10, 200, 10]);
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(super::thumbnail(std::path::Path::new("Z:/does/not/exist.mp4"), 128).is_err());
    }
}
