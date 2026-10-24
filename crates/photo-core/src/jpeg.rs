//! JPEG encoding, with `jpeg-encoder`: 3× faster than the `image` crate's encoder for
//! on-screen images, and about a third smaller at the same quality for kept files.
//!
//! Measured on a 12 MP photo-like image at quality 85 (release):
//! `image` 441 ms / 1545 KB; `Keep` 400 ms / 972 KB; `Show` 145 ms / 1405 KB, all with
//! the same error against the source.
//!
//! jpeg-encoder 0.7.1 writes corrupt files when chroma subsampling is combined with
//! optimized Huffman tables, so `Keep` never subsamples; the tests round-trip every
//! mode used here.

use image::RgbImage;

use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Thumbnails, the viewer, previews: fast (4:2:0 colour below quality 90).
    Show,
    /// Files the user keeps (export): full colour and optimized tables, smallest.
    Keep,
}

pub fn encode(image: &RgbImage, quality: u8, purpose: Purpose) -> Result<Vec<u8>> {
    let quality = quality.clamp(1, 100);
    let (w, h) = image.dimensions();
    // JPEG sizes are 16-bit; the image crate reports the error for oversized images.
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(image)?;
        return Ok(out);
    }
    let mut out = Vec::with_capacity((w * h / 4) as usize);
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    if purpose == Purpose::Keep {
        encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
        encoder.set_optimized_huffman_tables(true);
    }
    encoder
        .encode(
            image.as_raw(),
            w as u16,
            h as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|e| crate::Error::Invalid(format!("JPEG encoding failed: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Mean difference per channel after a round trip.
    fn round_trip_error(image: &RgbImage, quality: u8, purpose: Purpose) -> f64 {
        let data = encode(image, quality, purpose).unwrap();
        let back = image::load_from_memory(&data).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), image.dimensions());
        back.as_raw()
            .iter()
            .zip(image.as_raw())
            .map(|(a, b)| a.abs_diff(*b) as f64)
            .sum::<f64>()
            / image.as_raw().len() as f64
    }

    #[test]
    fn every_mode_round_trips() {
        // Odd sizes, so partial blocks and subsampled edges are covered.
        let image = RgbImage::from_fn(333, 201, |x, y| {
            Rgb([
                (x * 255 / 333) as u8,
                (y * 255 / 201) as u8,
                ((x + y) % 256) as u8,
            ])
        });
        for purpose in [Purpose::Show, Purpose::Keep] {
            for quality in [40, 80, 85, 92, 100] {
                let err = round_trip_error(&image, quality, purpose);
                assert!(err < 4.0, "{purpose:?} q{quality}: error {err:.2}");
            }
        }
    }

    #[test]
    fn kept_files_are_smaller() {
        let image = RgbImage::from_fn(640, 480, |x, y| {
            let v = ((x as f32 / 9.0).sin() * (y as f32 / 7.0).cos() * 60.0 + 128.0) as u8;
            Rgb([v, v / 2 + 40, 255 - v])
        });
        let shown = encode(&image, 95, Purpose::Show).unwrap().len();
        let kept = encode(&image, 95, Purpose::Keep).unwrap().len();
        assert!(kept < shown, "kept {kept} vs shown {shown}");
    }
}
