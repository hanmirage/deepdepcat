//! Structural validation for image bytes before conversation passthrough.
//!
//! The endpoint accepts a narrow set of formats on the wire, and a truncated
//! container (a JPEG cut mid-scan, a PNG with a corrupt chunk) would 400 on
//! every request it is embedded in. These walks decide whether bytes can pass
//! through raw or must fall through to re-encoding.

use image::ImageFormat;

/// Walk the JPEG marker structure and report whether a top-level EOI (`FFD9`)
/// is reached. Truncated files run off the buffer without one.
///
/// The `image` crate's JPEG decoder (zune-jpeg) pads missing scan data instead
/// of erroring, so a truncated JPEG passes a full pixel decode yet is still
/// rejected by the endpoint — only this marker walk catches it. Length-prefixed
/// segments are skipped by their declared length, and entropy-coded data after
/// SOS is scanned with byte-stuffing awareness (`FF00` literal, `FFD0`-`FFD7`
/// restart markers). Trailing bytes after the first top-level EOI are ignored.
pub fn jpeg_reaches_eoi(bytes: &[u8]) -> bool {
    let n = bytes.len();
    if n < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return false;
    }
    let mut i = 2;
    loop {
        // Skip stray garbage, then FF fill bytes, to reach the next marker.
        while i < n && bytes[i] != 0xFF {
            i += 1;
        }
        while i < n && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= n {
            return false;
        }
        let marker = bytes[i];
        i += 1;
        match marker {
            // Stuffed/stray FF00: still data, keep scanning.
            0x00 => {}
            // EOI reached.
            0xD9 => return true,
            // Standalone markers without a length field.
            0x01 | 0xD0..=0xD7 => {}
            // SOS: skip the length-prefixed header, then scan the
            // entropy-coded stream for the next real marker.
            0xDA => {
                let Some(next) = skip_segment(bytes, i) else {
                    return false;
                };
                i = next;
                loop {
                    while i < n && bytes[i] != 0xFF {
                        i += 1;
                    }
                    if i + 1 >= n {
                        return false;
                    }
                    match bytes[i + 1] {
                        // Byte-stuffed FF or fill byte: still entropy data.
                        0x00 => i += 2,
                        0xFF => i += 1,
                        // Restart marker: entropy data continues after it.
                        0xD0..=0xD7 => i += 2,
                        // Real marker terminates the scan; outer loop consumes it.
                        _ => break,
                    }
                }
            }
            _ => {
                let Some(next) = skip_segment(bytes, i) else {
                    return false;
                };
                i = next;
            }
        }
    }
}

/// Skip a length-prefixed JPEG segment starting at its 2-byte length field.
fn skip_segment(bytes: &[u8], at: usize) -> Option<usize> {
    let len_bytes = bytes.get(at..at + 2)?;
    let len = usize::from(len_bytes[0]) << 8 | usize::from(len_bytes[1]);
    if len < 2 {
        return None;
    }
    let end = at.checked_add(len)?;
    (end <= bytes.len()).then_some(end)
}

/// WebP: the RIFF header declares the total payload size at bytes 4..8;
/// truncation leaves the buffer shorter than declared. An optional pad byte
/// and trailing garbage are tolerated.
pub fn webp_riff_complete(bytes: &[u8]) -> bool {
    let Some(header) = bytes.get(..12) else {
        return false;
    };
    if &header[..4] != b"RIFF" || &header[8..12] != b"WEBP" {
        return false;
    }
    let riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    riff_size
        .checked_add(8)
        .is_some_and(|end| end <= bytes.len())
}

/// A PNG is structurally complete when the full pixel decode succeeds —
/// catching chunk CRC/IDAT truncation that a header-only read misses. Only
/// called on images already within the byte budget, so decode cost is bounded.
pub fn png_decodes(bytes: &[u8]) -> bool {
    image::load_from_memory(bytes).is_ok()
}

/// Structural validity walk for a known format. Formats without a dedicated
/// walk return true — their decoders reject truncation strictly, and they
/// never pass through raw (always transcoded to PNG first).
pub fn format_structurally_complete(format: ImageFormat, bytes: &[u8]) -> bool {
    match format {
        ImageFormat::Jpeg => jpeg_reaches_eoi(bytes),
        ImageFormat::Png => png_decodes(bytes),
        ImageFormat::WebP => webp_riff_complete(bytes),
        _ => true,
    }
}

/// Transcode GIF/BMP/TIFF/ICO to PNG. Returns `None` for already-native
/// (JPEG/PNG/WebP) or unrecognised input (caller keeps the original bytes);
/// `Some(Err)` on decode failure.
pub fn transcode_to_endpoint_png(bytes: &[u8]) -> Option<Result<Vec<u8>, String>> {
    let format = image::guess_format(bytes).ok()?;
    if !matches!(
        format,
        ImageFormat::Ico | ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::Tiff
    ) {
        return None;
    }
    Some(decode_to_png(bytes))
}

fn decode_to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("image decode failed: {e}"))?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| format!("image encode failed: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgba};

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, Rgba([1, 2, 3, 4]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    fn noisy_jpeg(w: u32, h: u32) -> Vec<u8> {
        use image::codecs::jpeg::JpegEncoder;
        use image::Rgb;
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
            Rgb([
                (x.wrapping_mul(13) ^ y) as u8,
                (x.wrapping_mul(7).wrapping_add(y * 3)) as u8,
                (x.wrapping_add(y).wrapping_mul(11)) as u8,
            ])
        });
        let mut buf = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, 85)
            .encode_image(&DynamicImage::ImageRgb8(img))
            .unwrap();
        buf
    }

    #[test]
    fn jpeg_reaches_eoi_valid_true_truncated_false() {
        assert!(jpeg_reaches_eoi(&noisy_jpeg(64, 64)));
        // The production failure shape: entropy stream cut at an arbitrary
        // point. zune-jpeg decodes these leniently, so only the walk catches.
        let jpeg = noisy_jpeg(128, 96);
        for frac in [3usize, 5, 7, 9] {
            let mut t = jpeg.clone();
            t.truncate(jpeg.len() * frac / 10);
            assert!(!jpeg_reaches_eoi(&t), "cut at {frac}0% must not reach EOI");
        }
        assert!(!jpeg_reaches_eoi(&[]));
        assert!(!jpeg_reaches_eoi(&[0xFF, 0xD8])); // bare SOI
        assert!(!jpeg_reaches_eoi(b"not a jpeg"));
        // Trailing garbage after EOI is legal (EXIF trailers).
        let mut trailing = noisy_jpeg(32, 32);
        trailing.extend_from_slice(b"trailing \xFF\xD8 junk");
        assert!(jpeg_reaches_eoi(&trailing));
    }

    #[test]
    fn webp_riff_complete_valid_true_truncated_false() {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(16, 16, Rgba([5, 6, 7, 255]));
        let mut webp = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut webp), ImageFormat::WebP)
            .unwrap();
        assert!(webp_riff_complete(&webp));
        let mut padded = webp.clone();
        padded.extend_from_slice(b"trailer");
        assert!(webp_riff_complete(&padded));
        let mut t = webp.clone();
        t.truncate(t.len() / 2);
        assert!(!webp_riff_complete(&t));
        assert!(!webp_riff_complete(b"RIFF"));
    }

    #[test]
    fn png_decodes_valid_true_corrupt_false() {
        assert!(png_decodes(&png_bytes(32, 32)));
        // Bit-flip inside IDAT → decode fails.
        let mut corrupt = png_bytes(32, 32);
        let idat = corrupt
            .windows(4)
            .position(|w| w == b"IDAT")
            .expect("IDAT present");
        corrupt[idat + 6] ^= 0xFF;
        assert!(!png_decodes(&corrupt));
        let mut t = png_bytes(32, 32);
        t.truncate(t.len() / 2);
        assert!(!png_decodes(&t));
    }

    #[test]
    fn transcode_to_endpoint_png_converts_gif_and_leaves_png() {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(6, 4, Rgba([1, 2, 3, 4]));
        let mut gif = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut gif), ImageFormat::Gif)
            .unwrap();
        let png = transcode_to_endpoint_png(&gif)
            .expect("GIF needs transcode")
            .expect("transcode succeeds");
        assert_eq!(image::guess_format(&png).unwrap(), ImageFormat::Png);
        assert!(transcode_to_endpoint_png(&png_bytes(4, 4)).is_none());
        assert!(transcode_to_endpoint_png(b"not an image").is_none());
        // Corrupt GIF must be Some(Err), never None (raw passthrough would 400).
        let mut cut = gif.clone();
        cut.truncate(20.min(cut.len()));
        if matches!(image::guess_format(&cut), Ok(ImageFormat::Gif)) {
            assert!(transcode_to_endpoint_png(&cut).unwrap().is_err());
        }
    }
}
