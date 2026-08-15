//! Preview commands — read a local HTML target for the in-app preview pane.
//!
//! The rebuilt dev browser renders generated HTML reports in a sandboxed
//! srcdoc iframe (Claude-Preview style) instead of driving a real Chromium
//! child process. This command feeds that pane: it reads a local file's
//! content and returns it (capped) so the frontend can inject a CSP and
//! render it. External URLs never reach here — they open the system browser.

use serde::Serialize;

/// Cap on previewed HTML (chars). A runaway generated report must not hang
/// the pane or bloat IPC; beyond the cap the content is truncated with a
/// marker the frontend can surface.
const MAX_PREVIEW_CHARS: usize = 2_000_000;

#[derive(Debug, Clone, Serialize)]
pub struct PreviewTarget {
    pub html: String,
    pub filename: String,
}

#[tauri::command]
pub async fn read_preview_target(path: String) -> Result<PreviewTarget, String> {
    let p = std::path::PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    let raw = tokio::fs::read(&p).await.map_err(|e| e.to_string())?;
    let mut html = String::from_utf8_lossy(&raw).to_string();
    if html.chars().count() > MAX_PREVIEW_CHARS {
        let keep: String = html.chars().take(MAX_PREVIEW_CHARS).collect();
        html = format!("{keep}\n<!-- truncated by DeepDepCat preview -->");
    }
    let filename = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    Ok(PreviewTarget { html, filename })
}

/// Open a preview target in the system default handler — an http(s) URL in
/// the default browser, or a local file in its default app. The target was
/// already validated by `dev_browser_open` (absolute existing file or a
/// normalized http/https URL), so no scheme is accepted here.
#[tauri::command]
pub async fn open_preview_external(target: String) -> Result<(), String> {
    let t = target.clone();
    tokio::task::spawn_blocking(move || open::that_detached(&t))
        .await
        .map_err(|e| format!("open task panicked: {e}"))?
        .map_err(|e| format!("Failed to open {target}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn reads_a_local_html_file() {
        let dir = std::env::temp_dir().join(format!("ddc-preview-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("report.html");
        std::fs::write(&f, "<html><body>hi 世界</body></html>").unwrap();

        let target = read_preview_target(f.to_string_lossy().to_string())
            .await
            .expect("read");
        assert_eq!(target.filename, "report.html");
        assert!(target.html.contains("hi 世界"));
    }

    #[tokio::test]
    async fn rejects_non_files() {
        let dir = std::env::temp_dir().join(format!("ddc-preview-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_preview_target(dir.to_string_lossy().to_string())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn caps_oversized_html() {
        let dir = std::env::temp_dir().join(format!("ddc-preview-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.html");
        let mut out = std::fs::File::create(&f).unwrap();
        // Write MAX + 100k 'x' chars.
        let chunk = "x".repeat(100_000);
        for _ in 0..(MAX_PREVIEW_CHARS / 100_000 + 2) {
            out.write_all(chunk.as_bytes()).unwrap();
        }
        drop(out);

        let target = read_preview_target(f.to_string_lossy().to_string())
            .await
            .expect("read");
        assert!(target.html.chars().count() <= MAX_PREVIEW_CHARS + 64);
        assert!(target.html.contains("truncated by DeepDepCat"));
    }
}
