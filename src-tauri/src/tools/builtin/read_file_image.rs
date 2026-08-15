//! read_file image branch — detect, compress and embed picture files.
//!
//! When `read_file` targets an image (design mockup, screenshot, UI figure),
//! the raw file is far too large to embed as-is. This module compresses it to
//! a context-safe payload (see `core::image_codec`), base64-encodes it, and
//! returns a text summary plus the image — which the tool batch injects into
//! the conversation as a transient image the model can see (vision-capable
//! main models only; text-only models get the automatic transcription
//! pipeline instead).

use crate::toolkit::{ToolImage, ToolResult};
use crate::core::error::AppResult;
use crate::core::image_codec;
use base64::Engine as _;

/// Magic-byte sniff: the bytes belong to an image format we can embed.
/// Non-images (text, binary) return `None` and read_file falls back to its
/// text path.
pub fn is_supported_image(bytes: &[u8]) -> Option<&'static str> {
    image_codec::sniff_mime(bytes)
}

/// Compress and encode an image, returning the model-visible summary text
/// with the embedded image attached.
///
/// Compression runs on a blocking thread (image decode/resize is CPU-bound).
/// On failure a model-visible error text is returned instead — the tool must
/// never embed bytes that would 400 the API on this and every following turn.
pub async fn build_image_result(
    path: &str,
    bytes: Vec<u8>,
    mime: &'static str,
) -> AppResult<ToolResult> {
    let (w, h, encoded, out_mime) = match tokio::task::spawn_blocking(move || {
        let (w, h) = probe_dimensions(&bytes).unwrap_or((0, 0));
        let (encoded, out_mime) = image_codec::compress_image_for_conversation(bytes, mime.into())
            .map_err(|e| format!("Could not embed image in conversation: {e}"))?;
        Ok::<_, String>((w, h, encoded, out_mime))
    })
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!(path, error = %e, "Image compression failed for read_file");
            return Ok(ToolResult::error(e));
        }
        Err(e) => {
            // Never leak a panic payload/path into model-visible text.
            tracing::warn!(path, error = %e, "Image compression task panicked");
            return Ok(ToolResult::error(
                "Image compression failed; see logs.".to_string(),
            ));
        }
    };

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&encoded);
    let kb = base64_data.len() / 1024;
    let summary = if (w, h) == (0, 0) {
        format!("Image file: {path} ({mime}, ~{kb} KB). Image content is attached for analysis.")
    } else {
        format!(
            "Image file: {path} ({w}x{h}px, {mime}, ~{kb} KB base64). Image content is attached for analysis."
        )
    };
    Ok(ToolResult::success(summary).with_image(ToolImage {
        media_type: out_mime,
        data: base64_data,
    }))
}

/// Cheap header-only dimension probe — used only for the summary text.
fn probe_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png_bytes() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(8, 8, Rgba([1, 2, 3, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn is_supported_image_detects_png_and_rejects_text() {
        assert_eq!(is_supported_image(&tiny_png_bytes()), Some("image/png"));
        assert_eq!(is_supported_image(b"plain text, not an image"), None);
    }

    #[test]
    fn build_image_result_attaches_embedded_image() {
        let bytes = tiny_png_bytes();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(build_image_result("img.png", bytes, "image/png"))
            .unwrap();
        assert!(!result.is_error, "image must embed: {:?}", result.content);
        assert!(result.content.contains("8x8px"), "summary has dimensions");
        let img = result.image.expect("image attached");
        assert_eq!(img.media_type, "image/png");
        // Base64 decodes back to valid PNG bytes.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .unwrap();
        assert_eq!(
            image::guess_format(&decoded).unwrap(),
            image::ImageFormat::Png
        );
    }

    #[test]
    fn build_image_result_returns_model_visible_error_on_garbage() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(build_image_result(
                "fake.png",
                b"garbage bytes".to_vec(),
                "image/png",
            ))
            .unwrap();
        assert!(result.is_error);
        assert!(result.image.is_none(), "no image on failure");
    }

    #[test]
    fn probe_dimensions_reads_header() {
        let bytes = tiny_png_bytes();
        assert_eq!(probe_dimensions(&bytes), Some((8, 8)));
        assert_eq!(probe_dimensions(b"not an image"), None);
    }
}
