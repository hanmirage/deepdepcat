//! Image compression for conversation embedding (read_file multimodal).
//!
//! The LLM request layer accepts base64 images, but a raw file on disk can be
//! tens of megabytes — embedding it as-is would blow the context budget. This
//! module shrinks an image so its base64 form stays under
//! [`MAX_IMAGE_PAYLOAD_BYTES`]: small structurally-complete images pass through
//! raw; anything else is downscaled and re-encoded (PNG or JPEG, whichever is
//! smaller) against the pixel-area and byte budgets.

use image::imageops::FilterType;
use image::ImageReader;

/// Max base64 size for an image embedded in the conversation.
pub const MAX_IMAGE_PAYLOAD_BYTES: usize = 768 * 1024;

/// Raw-byte budget derived from [`MAX_IMAGE_PAYLOAD_BYTES`].
const MAX_IMAGE_RAW_BYTES: usize = MAX_IMAGE_PAYLOAD_BYTES * 3 / 4;

/// Total pixel budget (w*h) for images sent to the model — the old 1024x1024
/// square budget as an aspect-agnostic area.
const MAX_IMAGE_PIXELS: u64 = 1_048_576;

/// Max pixel dimension (width or height) for images sent to the model.
const MAX_IMAGE_DIMENSION: u32 = 2000;

/// Floor dimension — re-encode gives up when `max_side` falls to or below this.
const MIN_IMAGE_DIMENSION: u32 = 128;

/// JPEG quality ladder for the read-file image compression path.
const QUALITY_STEPS: &[u8] = &[85, 70, 50, 40];

/// Absolute upper bound on decoded pixel count before we refuse to decode.
/// Matches the model API's pixel ceiling so any photo the API would accept can
/// be read and downscaled — a 20-48 Mpx camera photo must not fail `read_file`.
const MAX_DECODE_PIXELS: u64 = 178_956_970;

/// Why [`compress_image_for_conversation`] could not produce a
/// model-embeddable image.
#[derive(Debug, thiserror::Error)]
pub enum CompressImageError {
    #[error("image dimensions {width}x{height} exceed the {limit_pixels} pixel decode limit")]
    PixelLimitExceeded {
        width: u32,
        height: u32,
        limit_pixels: u64,
    },
    #[error("compressed image still exceeds the {0}-byte conversation payload cap")]
    PayloadCapExceeded(usize),
    #[error("image format could not be detected")]
    FormatDetectionFailed,
    #[error("image decode failed: {0}")]
    DecodeFailed(String),
}

/// Sniff an image's MIME type from its magic bytes.
///
/// Only formats the endpoint accepts on the wire are reported: PNG/JPEG/WebP
/// (native) plus GIF/BMP/TIFF/ICO (transcoded to PNG). Anything else (SVG,
/// TGA, HEIC, plain text) returns `None`.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::WebP => Some("image/webp"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::Bmp => Some("image/bmp"),
        image::ImageFormat::Tiff => Some("image/tiff"),
        image::ImageFormat::Ico => Some("image/x-icon"),
        _ => None,
    }
}

/// Resize and compress an image so its base64 form stays under
/// [`MAX_IMAGE_PAYLOAD_BYTES`].
///
/// Returns `(encoded_bytes, mime)` — the bytes are raw (not yet base64); the
/// caller encodes them. The returned MIME is `image/png` or `image/jpeg`.
pub fn compress_image_for_conversation(
    raw_bytes: Vec<u8>,
    original_mime: String,
) -> Result<(Vec<u8>, String), CompressImageError> {
    compress_image_for_conversation_with_caps(
        raw_bytes,
        original_mime,
        MAX_IMAGE_RAW_BYTES,
        MAX_IMAGE_PAYLOAD_BYTES,
    )
}

/// Cap-parameterised body — exposed for tests that need to reach the
/// `PayloadCapExceeded` branch deterministically.
fn compress_image_for_conversation_with_caps(
    raw_bytes: Vec<u8>,
    original_mime: String,
    max_raw_bytes: usize,
    max_payload_bytes: usize,
) -> Result<(Vec<u8>, String), CompressImageError> {
    // Pixel-bomb guard FIRST: a tiny compressed GIF/TIFF/ICO can declare a
    // huge canvas (30k×30k ≈ 3.6 GB RGBA). Reading the declared dimensions
    // from the header is cheap; the transcode below calls `load_from_memory`
    // (a full framebuffer decode) BEFORE the later dimension checks, so
    // without this guard the bomb would be fully decoded into memory first.
    // The native JPEG/PNG/WebP path also benefits (its own check is now
    // redundant but harmless).
    if let Some((w, h)) = ImageReader::new(std::io::Cursor::new(&raw_bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
    {
        if u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS {
            return Err(CompressImageError::PixelLimitExceeded {
                width: w,
                height: h,
                limit_pixels: MAX_DECODE_PIXELS,
            });
        }
    }

    // The endpoint only samples JPEG/PNG/WebP; transcode GIF/BMP/TIFF/ICO to
    // PNG first (kept before the small-image early return so the converted
    // bytes flow through).
    let (raw_bytes, original_mime) =
        match crate::core::image_codec_validate::transcode_to_endpoint_png(&raw_bytes) {
            Some(Ok(png)) => (png, "image/png".to_string()),
            Some(Err(e)) => {
                return Err(CompressImageError::DecodeFailed(format!(
                    "non-native image format transcode failed: {e}"
                )));
            }
            None => (raw_bytes, original_mime),
        };

    let params = ReEncodeParams {
        max_bytes: max_raw_bytes,
        max_side_px: MAX_IMAGE_DIMENSION,
        max_pixels: MAX_IMAGE_PIXELS,
        min_side_px: MIN_IMAGE_DIMENSION,
        quality_steps: QUALITY_STEPS,
        filter: FilterType::Lanczos3,
    };

    // An image can be small in bytes yet still too large in pixels (a
    // flat-colour UI screenshot). Only skip re-encoding when within BOTH the
    // byte budget and the dimension caps.
    let within_pixel_budget = ImageReader::new(std::io::Cursor::new(&raw_bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_none_or(|(w, h)| !params.exceeds_dimension_caps(w, h));

    // Pass through raw only if the bytes are a structurally complete
    // JPEG/PNG/WebP — the formats the endpoint accepts on the wire. Anything
    // else (truncated container, corrupt payload) falls through to the
    // re-encode chain, which either emits valid bytes or fails — never
    // embedding a payload that would 400 on this and every following turn.
    let passthrough_sendable = match image::guess_format(&raw_bytes) {
        Ok(
            format
            @ (image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP),
        ) => crate::core::image_codec_validate::format_structurally_complete(format, &raw_bytes),
        _ => false,
    };

    if (raw_bytes.len() * 4).div_ceil(3) <= max_payload_bytes
        && within_pixel_budget
        && passthrough_sendable
    {
        return Ok((raw_bytes, original_mime));
    }

    let reader = match ImageReader::new(std::io::Cursor::new(&raw_bytes)).with_guessed_format() {
        Ok(r) => r,
        Err(_) => return Err(CompressImageError::FormatDetectionFailed),
    };

    if reader.format().is_none() {
        return Err(CompressImageError::FormatDetectionFailed);
    }

    if let Ok((w, h)) = reader.into_dimensions() {
        if (w as u64) * (h as u64) > MAX_DECODE_PIXELS {
            return Err(CompressImageError::PixelLimitExceeded {
                width: w,
                height: h,
                limit_pixels: MAX_DECODE_PIXELS,
            });
        }
    }

    let img = match ImageReader::new(std::io::Cursor::new(&raw_bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.decode().ok())
    {
        Some(img) => img,
        None => {
            return Err(CompressImageError::DecodeFailed(
                "pixel decode returned no image".into(),
            ));
        }
    };

    let (buf, _w, _h, mime) = match re_encode_under_limit(&img, &params) {
        Ok(v) => v,
        Err(ReEncodeError::CouldNotFit { .. }) => {
            return Err(CompressImageError::PayloadCapExceeded(max_payload_bytes));
        }
    };

    Ok((buf, mime.to_string()))
}

/// Parameters that control the re-encode loop.
#[derive(Debug, Clone, Copy)]
pub struct ReEncodeParams {
    max_bytes: usize,
    max_side_px: u32,
    max_pixels: u64,
    min_side_px: u32,
    quality_steps: &'static [u8],
    filter: FilterType,
}

impl ReEncodeParams {
    /// True when either side exceeds `max_side_px` or the total pixel count
    /// exceeds `max_pixels` — shared by re-encode triggers and passthrough
    /// gates so the rule cannot drift between them.
    fn exceeds_dimension_caps(&self, w: u32, h: u32) -> bool {
        w > self.max_side_px
            || h > self.max_side_px
            || u64::from(w) * u64::from(h) > self.max_pixels
    }
}

/// Why `re_encode_under_limit` could not produce a compliant output.
#[derive(Debug, thiserror::Error)]
pub enum ReEncodeError {
    #[error(
        "re-encode could not fit under {max_bytes} bytes after PNG+JPEG attempts (last side {last_side}px)"
    )]
    CouldNotFit { max_bytes: usize, last_side: u32 },
}

/// Try PNG and JPEG encodings at descending dimensions, returning whichever
/// is smallest and fits under `params.max_bytes`.
///
/// On success returns `(bytes, width, height, mime_type)`.
fn re_encode_under_limit(
    decoded: &image::DynamicImage,
    params: &ReEncodeParams,
) -> Result<(Vec<u8>, u32, u32, &'static str), ReEncodeError> {
    // Never upscale: a small-but-heavy image is re-encoded at its own
    // resolution, not enlarged to `max_side_px`. `image::resize` scales *up*
    // to fill the target box, so starting at `max_side_px` would enlarge
    // anything smaller — adding no detail and wasting request bytes.
    let original_max_side = decoded.width().max(decoded.height());
    let mut max_side = params.max_side_px.min(original_max_side);
    let original_pixels = u64::from(decoded.width()) * u64::from(decoded.height());
    if original_pixels > params.max_pixels {
        max_side = max_side.min(area_capped_side(
            original_max_side,
            decoded.width().min(decoded.height()),
            params.max_pixels,
        ));
    }

    loop {
        // Only resample when actually downscaling. `resize(w, h)` preserves
        // aspect ratio (fits inside w×h, not stretch-to-square).
        let scaled: std::borrow::Cow<'_, image::DynamicImage> = if max_side < original_max_side {
            std::borrow::Cow::Owned(decoded.resize(max_side, max_side, params.filter))
        } else {
            std::borrow::Cow::Borrowed(decoded)
        };
        let img: &image::DynamicImage = &scaled;
        let (w, h) = (img.width(), img.height());

        let png_candidate = {
            let mut buf = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .ok()
                .filter(|_| buf.len() <= params.max_bytes)
                .map(|_| buf)
        };

        let jpeg_candidate = params.quality_steps.iter().find_map(|&quality| {
            let mut buf = Vec::new();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
            enc.encode_image(img).ok()?;
            (buf.len() <= params.max_bytes).then_some(buf)
        });

        match (png_candidate, jpeg_candidate) {
            (Some(png), Some(jpeg)) => {
                if png.len() <= jpeg.len() {
                    return Ok((png, w, h, "image/png"));
                }
                return Ok((jpeg, w, h, "image/jpeg"));
            }
            (Some(png), None) => return Ok((png, w, h, "image/png")),
            (None, Some(jpeg)) => return Ok((jpeg, w, h, "image/jpeg")),
            (None, None) => {}
        }

        if max_side <= params.min_side_px {
            return Err(ReEncodeError::CouldNotFit {
                max_bytes: params.max_bytes,
                last_side: max_side,
            });
        }
        max_side = max_side * 3 / 4;
    }
}

/// Largest target long side whose resize output area stays within `max_pixels`.
fn area_capped_side(long: u32, short: u32, max_pixels: u64) -> u32 {
    let scale = (max_pixels as f64 / (u64::from(long) * u64::from(short)) as f64).sqrt();
    let mut side = ((f64::from(long) * scale).floor() as u32).clamp(1, long);
    // Nearest-rounding of the short side can overshoot the budget by ~side/2
    // pixels, so step down until the predicted output fits.
    while side > 1 && predicted_resize_area(long, short, side) > max_pixels {
        side -= 1;
    }
    side
}

/// Minimum crop side (px) after upscaling — a zoomed region must be large
/// enough for the vision model to actually read its detail.
pub const CROP_MIN_SIDE_PX: u32 = 512;

/// A crop region, expressed either in pixel coordinates or as fractions of
/// the image's own dimensions (0.0–1.0). Parsed from the tool's `region`
/// string (`"x,y,w,h"` pixels, or every value suffixed `%` for fractions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageRegion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Values are fractions of the image when true, pixels when false.
    pub relative: bool,
}

impl ImageRegion {
    /// Parse `"x,y,w,h"` (pixels) or `"10%,20%,30%,40%"` (fractions).
    /// Mixed pixel/% values or a malformed string → `None`; a zero/negative
    /// size or an out-of-range relative region is also rejected.
    pub fn parse(s: &str) -> Option<ImageRegion> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return None;
        }
        let any_pct = parts.iter().any(|p| p.ends_with('%'));
        let all_pct = parts.iter().all(|p| p.ends_with('%'));
        if any_pct && !all_pct {
            return None;
        }
        let mut vals = [0f32; 4];
        for (i, part) in parts.iter().enumerate() {
            let cleaned = part.trim_end_matches('%');
            let mut v: f32 = cleaned.parse().ok()?;
            if !v.is_finite() || v < 0.0 {
                return None;
            }
            // Fraction values arrive as "10%" → 0.1.
            if all_pct {
                v /= 100.0;
            }
            vals[i] = v;
        }
        let (x, y, w, h) = (vals[0], vals[1], vals[2], vals[3]);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        if all_pct && (x + w > 1.0 + 1e-3 || y + h > 1.0 + 1e-3) {
            return None;
        }
        Some(ImageRegion {
            x,
            y,
            w,
            h,
            relative: all_pct,
        })
    }

    /// Canonical cache-key fragment — distinguishes pixel vs relative regions
    /// so the same image described at two different zoom levels never collides.
    pub fn cache_fragment(&self) -> String {
        if self.relative {
            format!("rel:{:.3},{:.3},{:.3},{:.3}", self.x, self.y, self.w, self.h)
        } else {
            format!("px:{:.0},{:.0},{:.0},{:.0}", self.x, self.y, self.w, self.h)
        }
    }
}

/// Crop an image to a region and re-encode as PNG. Relative regions resolve
/// against the decoded image; pixel regions are clamped to the image bounds
/// (a region that overshoots the edge is trimmed, not rejected). Crops
/// smaller than [`CROP_MIN_SIDE_PX`] are upscaled so the vision model can
/// read the zoomed detail. Returns `(png_bytes, "image/png", width, height)`.
pub fn crop_image_region(
    raw_bytes: Vec<u8>,
    region: &ImageRegion,
) -> Result<(Vec<u8>, String, u32, u32), String> {
    let reader = ImageReader::new(std::io::Cursor::new(&raw_bytes))
        .with_guessed_format()
        .map_err(|e| format!("image format detection failed: {e}"))?;
    let (img_w, img_h) = reader
        .into_dimensions()
        .map_err(|e| format!("dimension probe failed: {e}"))?;
    if u64::from(img_w) * u64::from(img_h) > MAX_DECODE_PIXELS {
        return Err(format!(
            "image {img_w}x{img_h} exceeds the {MAX_DECODE_PIXELS} pixel decode limit"
        ));
    }
    let img = ImageReader::new(std::io::Cursor::new(raw_bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.decode().ok())
        .ok_or_else(|| "image decode returned no image".to_string())?;

    let (x, y, w, h) = if region.relative {
        (
            region.x * img_w as f32,
            region.y * img_h as f32,
            region.w * img_w as f32,
            region.h * img_h as f32,
        )
    } else {
        (region.x, region.y, region.w, region.h)
    };
    let x0 = x.round().max(0.0) as u32;
    let y0 = y.round().max(0.0) as u32;
    let w = w.round().max(1.0) as u32;
    let h = h.round().max(1.0) as u32;
    let w = w.min(img_w.saturating_sub(x0));
    let h = h.min(img_h.saturating_sub(y0));
    if w == 0 || h == 0 {
        return Err("crop region falls outside the image".to_string());
    }
    let crop = img.crop_imm(x0, y0, w, h);

    let out = if w < CROP_MIN_SIDE_PX && h < CROP_MIN_SIDE_PX {
        let scale = CROP_MIN_SIDE_PX as f32 / w.min(h) as f32;
        crop.resize(
            ((w as f32) * scale).round().max(1.0) as u32,
            ((h as f32) * scale).round().max(1.0) as u32,
            FilterType::Lanczos3,
        )
    } else {
        crop
    };
    let (out_w, out_h) = (out.width(), out.height());
    let mut buf = Vec::new();
    out.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| format!("crop re-encode failed: {e}"))?;
    Ok((buf, "image/png".to_string(), out_w, out_h))
}

/// Output area `image::resize` produces for a `side`×`side` bounding box,
/// mirroring `resize_dimensions` expression-for-expression (image-0.25).
fn predicted_resize_area(long: u32, short: u32, side: u32) -> u64 {
    let ratio = f64::from(side) / f64::from(long);
    let scaled_long = (f64::from(long) * ratio).round().max(1.0) as u64;
    let scaled_short = (f64::from(short) * ratio).round().max(1.0) as u64;
    scaled_long * scaled_short
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_noisy_png(width: u32, height: u32) -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img = ImageBuffer::from_fn(width, height, |x, y| {
            let seed = (x as u64).wrapping_mul(6364136223846793005)
                ^ (y as u64).wrapping_mul(1442695040888963407);
            let r = (seed & 0xFF) as u8;
            let g = ((seed >> 8) & 0xFF) as u8;
            let b = ((seed >> 16) & 0xFF) as u8;
            Rgba([r, g, b, 255u8])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn make_small_png(width: u32, height: u32) -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img = ImageBuffer::from_pixel(width, height, Rgba([0u8, 0, 0, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn dims(bytes: &[u8]) -> (u32, u32) {
        ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
    }

    #[test]
    fn compress_small_image_returns_unchanged() {
        let png = make_small_png(16, 16);
        let (result, mime) =
            compress_image_for_conversation(png.clone(), "image/png".into()).unwrap();
        assert_eq!(result, png);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn compress_truncated_small_jpeg_re_encodes_to_valid_bytes() {
        use image::codecs::jpeg::JpegEncoder;
        use image::{DynamicImage, ImageBuffer, Rgb};
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(200, 150, |x, y| {
            Rgb([(x ^ y) as u8, (x * 3) as u8, (y * 5) as u8])
        });
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode_image(&DynamicImage::ImageRgb8(img))
            .unwrap();
        jpeg.truncate(jpeg.len() / 2);
        assert!(
            !crate::core::image_codec_validate::jpeg_reaches_eoi(&jpeg),
            "precondition: input is structurally incomplete"
        );
        let (result, _mime) =
            compress_image_for_conversation(jpeg.clone(), "image/jpeg".into()).unwrap();
        assert_ne!(result, jpeg, "raw truncated bytes must not pass through");
        assert!(
            crate::core::image_codec_validate::format_structurally_complete(
                image::ImageFormat::Jpeg,
                &result
            ),
            "output must be structurally complete"
        );
    }

    #[test]
    fn compress_small_gif_becomes_png() {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(24, 24, Rgba([1u8, 2, 3, 255]));
        let mut gif = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut gif), image::ImageFormat::Gif)
            .unwrap();
        let (result, mime) =
            compress_image_for_conversation(gif, "image/gif".into()).expect("gif compresses");
        assert_eq!(mime, "image/png");
        assert_eq!(
            image::guess_format(&result).unwrap(),
            image::ImageFormat::Png
        );
    }

    #[test]
    fn compress_large_noisy_image_picks_jpeg() {
        let png = make_noisy_png(2048, 1536);
        let b64_before = (png.len() * 4).div_ceil(3);
        assert!(
            b64_before > MAX_IMAGE_PAYLOAD_BYTES,
            "test image ({b64_before} B b64) must exceed the payload limit"
        );
        let (result, mime) = compress_image_for_conversation(png, "image/png".into()).unwrap();
        assert_eq!(mime, "image/jpeg");
        let b64_after = (result.len() * 4).div_ceil(3);
        assert!(
            b64_after <= MAX_IMAGE_PAYLOAD_BYTES,
            "compressed image ({b64_after} B b64) must fit within {MAX_IMAGE_PAYLOAD_BYTES} B"
        );
    }

    #[test]
    fn compress_large_dimensions_small_bytes_downscales() {
        let png = make_small_png(2048, 2600);
        let b64_before = (png.len() * 4).div_ceil(3);
        assert!(
            b64_before <= MAX_IMAGE_PAYLOAD_BYTES,
            "fixture must be under the byte cap to exercise the pixel gate"
        );
        let (result, _mime) =
            compress_image_for_conversation(png, "image/png".into()).expect("downscale succeeds");
        let (w, h) = dims(&result);
        assert!(w <= MAX_IMAGE_DIMENSION && h <= MAX_IMAGE_DIMENSION);
        assert!(u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS);
    }

    #[test]
    fn compress_wide_image_under_area_budget_passes_through() {
        let png = make_small_png(1600, 600);
        let (result, mime) =
            compress_image_for_conversation(png.clone(), "image/png".into()).unwrap();
        assert_eq!(result, png, "within-budget image must not be re-encoded");
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn compress_screenshot_respects_area_budget() {
        let png = make_small_png(3438, 1830);
        let (result, _mime) =
            compress_image_for_conversation(png, "image/png".into()).expect("downscale succeeds");
        let (w, h) = dims(&result);
        assert!(u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS);
        let r_in = 3438.0 / 1830.0;
        let r_out = w as f64 / h as f64;
        assert!((r_in - r_out).abs() < 0.05, "aspect {r_in} -> {r_out}");
    }

    #[test]
    fn compress_camera_sized_photo_succeeds() {
        let png = make_noisy_png(5000, 5000);
        let (out, mime) =
            compress_image_for_conversation(png, "image/png".into()).expect("photo must compress");
        assert_eq!(mime, "image/jpeg");
        let (w, h) = dims(&out);
        assert!(u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS);
    }

    #[test]
    fn compress_above_api_ceiling_returns_pixel_limit_exceeded() {
        use image::codecs::jpeg::JpegEncoder;
        use image::{DynamicImage, ImageBuffer, Rgb};
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(64, 64, Rgb([7, 8, 9]));
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode_image(&DynamicImage::ImageRgb8(img))
            .unwrap();
        let sof = jpeg
            .windows(2)
            .position(|w| w == [0xFF, 0xC0])
            .expect("baseline SOF0 present");
        // 16384 x 16384 = 268 Mpx, above the 178.9 Mpx ceiling.
        jpeg[sof + 5..sof + 9].copy_from_slice(&[0x40, 0x00, 0x40, 0x00]);
        let err = compress_image_for_conversation(jpeg, "image/jpeg".into()).unwrap_err();
        match err {
            CompressImageError::PixelLimitExceeded {
                width,
                height,
                limit_pixels,
            } => {
                assert_eq!((width, height), (16384, 16384));
                assert_eq!(limit_pixels, MAX_DECODE_PIXELS);
            }
            other => panic!("expected PixelLimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn transcode_format_pixel_bomb_is_blocked_before_decode() {
        // A GIF/BMP/TIFF/ICO with a tiny on-disk size but a huge declared
        // canvas (30k×30k ≈ 3.6 GB RGBA) must be rejected by the dimension
        // guard BEFORE the transcode path calls load_from_memory — otherwise
        // the full framebuffer is allocated and the app OOMs.
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&64u16.to_le_bytes()); // logical screen width
        gif.extend_from_slice(&64u16.to_le_bytes()); // logical screen height
        gif.extend_from_slice(&[0x00, 0x00, 0x00]); // flags, bg, aspect
        gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 64, 0, 64, 0, 0x00]); // image descriptor
        gif.extend_from_slice(&[0x3B]); // trailer
        // Patch the declared canvas to 30000×30000 (900 Mpx > 178.9 Mpx cap).
        gif[6..8].copy_from_slice(&30_000u16.to_le_bytes());
        gif[8..10].copy_from_slice(&30_000u16.to_le_bytes());

        let err =
            compress_image_for_conversation_with_caps(gif, "image/gif".into(), 1_000_000, 1_000_000)
                .unwrap_err();
        match err {
            CompressImageError::PixelLimitExceeded {
                width,
                height,
                limit_pixels,
            } => {
                assert_eq!((width, height), (30_000, 30_000));
                assert_eq!(limit_pixels, MAX_DECODE_PIXELS);
            }
            other => panic!("expected PixelLimitExceeded before transcode, got {other:?}"),
        }
    }

    #[test]
    fn compress_small_undecodable_fails_closed() {
        let garbage = b"not an image at all".to_vec();
        let err = compress_image_for_conversation(garbage, "image/svg+xml".into())
            .expect_err("unsniffable bytes must not pass through raw");
        assert!(
            matches!(err, CompressImageError::FormatDetectionFailed),
            "got: {err:?}"
        );
    }

    #[test]
    fn compress_oversized_corrupt_png_returns_decode_failed() {
        let mut png = make_noisy_png(1024, 1024);
        let tag = b"IDAT";
        let pos = png.windows(4).position(|w| w == tag).unwrap();
        for i in 0..512 {
            png[pos + 8 + i] ^= 0x5A;
        }
        let err = compress_image_for_conversation(png, "image/png".into()).unwrap_err();
        assert!(
            matches!(err, CompressImageError::DecodeFailed(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn payload_cap_exceeded_reached_through_production_path() {
        let png = make_noisy_png(256, 256);
        let err = compress_image_for_conversation_with_caps(png, "image/png".into(), 1024, 1400)
            .unwrap_err();
        match err {
            CompressImageError::PayloadCapExceeded(cap) => {
                assert_eq!(cap, 1400);
            }
            other => panic!("expected PayloadCapExceeded, got {other:?}"),
        }
    }

    #[test]
    fn parse_region_accepts_pixels_and_fractions() {
        let px = ImageRegion::parse("10,20,300,400").unwrap();
        assert_eq!((px.x, px.y, px.w, px.h), (10.0, 20.0, 300.0, 400.0));
        assert!(!px.relative);
        let rel = ImageRegion::parse("10%,20%,30%,40%").unwrap();
        assert!((rel.x - 0.1).abs() < 1e-6);
        assert!((rel.y - 0.2).abs() < 1e-6);
        assert!((rel.w - 0.3).abs() < 1e-6);
        assert!((rel.h - 0.4).abs() < 1e-6);
        assert!(rel.relative);
    }

    #[test]
    fn parse_region_rejects_malformed() {
        assert!(ImageRegion::parse("").is_none());
        assert!(ImageRegion::parse("1,2,3").is_none());
        assert!(ImageRegion::parse("1,2,3,4,5").is_none());
        assert!(ImageRegion::parse("10,20,300,abc").is_none());
        assert!(ImageRegion::parse("10%,20%,30,40").is_none(), "mixed px/% rejected");
        assert!(ImageRegion::parse("-1,0,10,10").is_none(), "negative coordinate rejected");
        assert!(ImageRegion::parse("0,0,0,10").is_none(), "zero-size rejected");
        assert!(ImageRegion::parse("50%,50%,60%,60%").is_none(), "out-of-bounds fraction rejected");
    }

    #[test]
    fn crop_image_region_crops_pixel_region() {
        // Region larger than CROP_MIN_SIDE_PX on both sides → exact size, no upscale.
        let png = make_small_png(800, 600);
        let region = ImageRegion::parse("50,20,600,400").unwrap();
        let (out, mime, w, h) = crop_image_region(png, &region).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!((w, h), (600, 400));
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::Png);
    }

    #[test]
    fn crop_image_region_resolves_relative() {
        let png = make_small_png(1200, 800);
        // 25%,25%,50%,50% of 1200x800 → 600x400 at offset (300,200).
        let region = ImageRegion::parse("25%,25%,50%,50%").unwrap();
        let (_, _, w, h) = crop_image_region(png, &region).unwrap();
        assert_eq!((w, h), (600, 400));
    }

    #[test]
    fn crop_image_region_upscales_small_crop() {
        let png = make_small_png(800, 600);
        // A 20x20 crop is far below CROP_MIN_SIDE_PX — it must be upscaled so
        // the vision model can read the zoomed detail.
        let region = ImageRegion::parse("10,10,20,20").unwrap();
        let (_, _, w, h) = crop_image_region(png, &region).unwrap();
        assert!(w >= CROP_MIN_SIDE_PX, "small crop must be upscaled: {w}x{h}");
        assert!(h >= CROP_MIN_SIDE_PX, "small crop must be upscaled: {w}x{h}");
        assert_eq!(w, h, "square crop stays square after upscale");
    }

    #[test]
    fn crop_image_region_clamps_overshoot() {
        let png = make_small_png(100, 100);
        // Region starts at (80,80) with size 50x50 — clamped to the 20x20 tail,
        // then upscaled to the readability floor (square in, square out).
        let region = ImageRegion::parse("80,80,50,50").unwrap();
        let (_, _, w, h) = crop_image_region(png, &region).unwrap();
        assert_eq!(w, h, "clamped square stays square");
        assert!(w >= CROP_MIN_SIDE_PX, "clamped crop is upscaled to readable size");
    }
}
