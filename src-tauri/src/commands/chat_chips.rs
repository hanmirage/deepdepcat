//! Context-chip splitting — image chips are pulled out of the prompt and
//! routed into the vision pipeline (native parts for multimodal models,
//! transcription for text-only models). Extracted from `chat.rs` to keep
//! the send path within the file-size budget.

use crate::agent::image_transcribe::ImageInput;
use crate::bootstrap::AppState;
use crate::core::types::ContextChip;

/// Split context chips into (kept chips, image inputs, image notes).
///
/// Pasted/picked images arrive as data URLs, dragged files as paths, and
/// image URLs are downloaded — all are removed from the context so the
/// model never resolves their paths itself.
pub async fn split_image_chips(
    context_chips: Option<Vec<ContextChip>>,
    state: &AppState,
    session_id: &str,
) -> (Vec<ContextChip>, Vec<ImageInput>, Vec<(String, String)>) {
    let chips = context_chips.unwrap_or_default();
    let mut image_inputs: Vec<ImageInput> = Vec::new();
    let mut kept_chips: Vec<ContextChip> = Vec::new();
    // (name, resolvable path) for every attached image — injected into the
    // context so the model can call `visual_describe` on a picture whose
    // automatic transcription is not detailed enough.
    let mut image_notes: Vec<(String, String)> = Vec::new();

    for chip in chips {
        let ContextChip::File { name, path, .. } = &chip else {
            // URL chips that point at an image (dragged/pasted link) are
            // downloaded and pushed through the vision pipeline — the vision
            // tool then sees the picture like any other attachment. Any other
            // URL stays a plain web link.
            if let ContextChip::Url { name, path } = &chip {
                if is_image_url(path) {
                    if let Some((mime, bytes)) = download_image_bytes(path).await {
                        let persisted = match persist_pasted_image(state, name, &mime, &bytes) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(session_id, url = %path, error = %e, "Failed to persist URL image");
                                String::new()
                            }
                        };
                        image_notes.push((name.clone(), persisted));
                        image_inputs.push(ImageInput { mime, bytes });
                        continue;
                    }
                    tracing::warn!(session_id, url = %path, "Image URL download failed — kept as web link");
                }
            }
            kept_chips.push(chip);
            continue;
        };
        if let Some(data_url) = chip.data_url() {
            if let Some((mime, bytes)) = crate::agent::image_transcribe::parse_data_url(data_url) {
                // Pasted/picked image has no filesystem path — persist it so
                // the model can read it via visual_describe later. A persist
                // failure degrades to description-only (never blocks the send).
                let persisted = match persist_pasted_image(state, name, &mime, &bytes) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(session_id, error = %e, "Failed to persist pasted image");
                        String::new()
                    }
                };
                image_notes.push((name.clone(), persisted));
                image_inputs.push(ImageInput { mime, bytes });
                continue;
            }
        }
        if is_image_path(path) {
            if let Ok(bytes) = std::fs::read(path) {
                if let Some(mime) = crate::core::image_codec::sniff_mime(&bytes) {
                    image_notes.push((name.clone(), path.clone()));
                    image_inputs.push(ImageInput {
                        mime: mime.to_string(),
                        bytes,
                    });
                    continue;
                }
            }
        }
        kept_chips.push(chip);
    }

    (kept_chips, image_inputs, image_notes)
}

/// Whether a file path points at an image the transcription path should read
/// directly (dragged files arrive as paths). Kept in sync with
/// `core::image_codec::sniff_mime` — everything sniffable is readable here.
fn is_image_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tiff", ".tif", ".ico",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// Whether a URL points at an image file (path extension check, query/fragment
/// stripped). Kept in sync with [`is_image_path`].
fn is_image_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    [
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tiff", ".tif", ".ico",
    ]
    .iter()
    .any(|ext| path.ends_with(ext))
}

/// Download image bytes from an http(s) URL, capped in size. Returns
/// `(mime, bytes)` only when the payload is a real sniffable image —
/// anything else (HTTP error, oversize, non-image body) returns `None`.
async fn download_image_bytes(url: &str) -> Option<(String, Vec<u8>)> {
    const MAX_DOWNLOAD_BYTES: usize = 20 * 1024 * 1024;
    // The URL comes from pasted/dragged user input — never let the fetch
    // reach internal networks or the local machine (SSRF, same guard as
    // the web_fetch tool). Rejection degrades to keeping the plain link.
    crate::hooks::ssrf::validate_fetch_url(url).ok()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("DeepDepCat/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return None;
    }
    let mime = crate::core::image_codec::sniff_mime(&bytes)?.to_string();
    Some((mime, bytes.to_vec()))
}

/// Persist a pasted/picked image (data URL bytes) into the app data dir so
/// the `visual_describe` tool can read it by path later.
///
/// Pasted/picked images only exist as in-memory bytes — without a real
/// filesystem location the vision tool chain is unusable for them. Files are
/// written under `app_data_dir/vision-images/` with an id suffix to avoid
/// collisions. Failure is non-fatal to the send (the automatic transcription
/// still works) — callers degrade to description-only.
fn persist_pasted_image(
    state: &AppState,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<String, String> {
    const MAX_VISION_IMAGES: usize = 200;
    let dir = state.app_data_dir.join("vision-images");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create vision-images dir: {e}"))?;

    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "img",
    };
    let stem = if name.trim().is_empty() {
        "pasted".to_string()
    } else {
        let base = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(name)
            .split('.')
            .next()
            .unwrap_or("pasted")
            .to_string();
        if base.trim().is_empty() {
            "pasted".to_string()
        } else {
            base
        }
    };
    let suffix: String = crate::core::ids::generate_id().chars().take(8).collect();
    let file_name = format!("{stem}-{suffix}.{ext}");
    let path = dir.join(file_name);
    std::fs::write(&path, bytes).map_err(|e| format!("Failed to persist pasted image: {e}"))?;
    // The vision-images dir grows one file per attachment with no natural
    // cleanup point (session delete doesn't know the file paths) — cap the
    // total and evict the oldest files so it cannot accumulate unbounded.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .filter_map(|e| {
                e.metadata()
                    .ok()
                    .map(|m| (m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH), e.path()))
            })
            .collect();
        files.sort_by_key(|(modified, _)| *modified);
        let excess = files.len().saturating_sub(MAX_VISION_IMAGES);
        for (_, stale) in files.drain(..excess) {
            let _ = std::fs::remove_file(stale);
        }
    }
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_path_detection_covers_common_formats() {
        assert!(is_image_path("C:\\tmp\\a.png"));
        assert!(is_image_path("C:\\tmp\\a.JPG"));
        assert!(is_image_path("C:\\tmp\\a.webp"));
        assert!(is_image_path("/tmp/b.gif"));
        assert!(is_image_path("/tmp/scan.tiff"));
        assert!(is_image_path("C:\\tmp\\a.TIF"));
        assert!(is_image_path("/tmp/favicon.ico"));
        assert!(!is_image_path("C:\\tmp\\a.txt"));
        assert!(!is_image_path("C:\\tmp\\a.md"));
    }

    #[test]
    fn image_url_detection_ignores_query_and_fragment() {
        assert!(is_image_url("https://cdn.example.com/a.png"));
        assert!(is_image_url("https://cdn.example.com/photo.JPEG?token=abc"));
        assert!(is_image_url("https://img.example.com/1.webp#frag"));
        assert!(is_image_url("http://localhost:8080/shot.tiff"));
        assert!(is_image_url("https://example.com/favicon.ico?v=2"));
        assert!(!is_image_url("https://example.com/page"));
        assert!(!is_image_url("https://example.com/a.png.html"));
        assert!(!is_image_url("not a url"));
    }
}
