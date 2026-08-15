//! Read file tool — reads file contents with optional offset and limit.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

pub struct ReadFileTool {
    max_output_chars: usize,
}

/// Bytes sniffed up front in the range path to decide whether a full read
/// is justified (image/document payloads) before streaming text.
const SNIFF_BYTES: u64 = 256 * 1024;

impl ReadFileTool {
    pub fn new(max_output_chars: usize) -> Self {
        Self { max_output_chars }
    }

    /// Image branch shared by the full-read and range paths: embed the
    /// picture (or transcribe it for text-only models). `note` is appended
    /// when offset/limit were requested and are ignored.
    async fn image_result(
        &self,
        path: &str,
        bytes: Vec<u8>,
        mime: &'static str,
        context: &ToolContext,
        note: &str,
    ) -> AppResult<ToolResult> {
        use tauri::Manager as _;
        let state = context.app.state::<crate::bootstrap::AppState>();
        let can_see_images = {
            let sessions = state.sessions.lock().await;
            sessions.model_catalog().supports_vision(&context.model)
        };
        let mut result = if can_see_images {
            super::read_file_image::build_image_result(path, bytes, mime).await?
        } else {
            match crate::agent::image_transcribe::transcribe_images(
                &state,
                &context.session_id,
                vec![crate::agent::image_transcribe::ImageInput {
                    mime: mime.to_string(),
                    bytes,
                }],
            )
            .await
            {
                Ok((mut descriptions, usage)) => {
                    if let Some(ref tracker) = context.usage_tracker {
                        tracker.record_llm_usage(0, &usage);
                    }
                    let mut desc = descriptions.remove(0);
                    desc = format!("Image file: {path}\n\n{desc}");
                    ToolResult::success(desc)
                }
                Err(e) => ToolResult::error(format!("Could not describe image file '{path}': {e}")),
            }
        };
        if !note.is_empty() {
            result.content.push_str(note);
        }
        Ok(result)
    }

    /// Document branch shared by both paths; `note` appended when
    /// offset/limit were requested and are ignored.
    fn document_result(
        &self,
        path_buf: &Path,
        bytes: Vec<u8>,
        kind: super::read_file_document::DocumentKind,
        note: &str,
    ) -> AppResult<ToolResult> {
        let mut result = super::read_file_document::build_document_result(path_buf, bytes, kind)?;
        if !note.is_empty() {
            result.content.push_str(note);
        }
        Ok(result)
    }

    /// Bounded-memory text read for range requests (offset/limit).
    ///
    /// The file is streamed in ONE pass: every byte is hashed (stale-edit
    /// fingerprint parity with the full-read path) and validated as UTF-8,
    /// but only the requested window of lines is kept — a multi-GB file
    /// costs time proportional to its size, never memory proportional to it.
    async fn execute_range(
        &self,
        path: &str,
        path_buf: &PathBuf,
        offset: usize,
        limit: Option<usize>,
        context: &ToolContext,
    ) -> AppResult<ToolResult> {
        let mut file = match std::fs::File::open(path_buf) {
            Ok(f) => f,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file '{}': {}",
                    path, e
                )))
            }
        };

        // Sniff the head before deciding: image/document payloads require
        // the full bytes; anything else is streamed as text below.
        let mut prefix = Vec::new();
        {
            let mut limited = (&mut file).take(SNIFF_BYTES);
            let _ = limited.read_to_end(&mut prefix);
        }

        if let Some(mime) = super::read_file_image::is_supported_image(&prefix) {
            let bytes = match std::fs::read(path_buf) {
                Ok(b) => b,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Failed to read file '{}': {}",
                        path, e
                    )))
                }
            };
            return self
                .image_result(
                    path,
                    bytes,
                    mime,
                    context,
                    "\nNote: offset/limit are ignored for image files.",
                )
                .await;
        }

        // Document magic: PDF needs only the prefix for kind detection;
        // OOXML zips need the full payload (their index lives at the end).
        let document = if prefix.starts_with(b"%PDF-") {
            super::read_file_document::detect_document(&prefix)
        } else if prefix.starts_with(b"PK\x03\x04") {
            let bytes = match std::fs::read(path_buf) {
                Ok(b) => b,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Failed to read file '{}': {}",
                        path, e
                    )))
                }
            };
            let kind = super::read_file_document::detect_document(&bytes);
            return match kind {
                Some(kind) => self.document_result(
                    path_buf,
                    bytes,
                    kind,
                    "\nNote: offset/limit are ignored for documents.",
                ),
                None => Ok(ToolResult::error(format!(
                    "File appears to be binary ({:?}). Cannot display as text.",
                    path_buf
                ))),
            };
        } else {
            None
        };
        if let Some(kind) = document {
            return self.document_result(
                path_buf,
                prefix,
                kind,
                "\nNote: offset/limit are ignored for documents.",
            );
        }

        if std::str::from_utf8(&prefix).is_err() && prefix.contains(&0) {
            return Ok(ToolResult::error(format!(
                "File appears to be binary ({:?}). Cannot display as text.",
                path_buf
            )));
        }

        // Text: stream lines, keep only the requested window.
        let mut reader = std::io::BufReader::new(file);
        let mut hash = crate::tools::stale_edit::FNV64_OFFSET;
        let mut line_index: usize = 0;
        let mut collected: Vec<String> = Vec::new();
        let mut collected_chars: usize = 0;
        // Small slack above the cap so the existing truncation suffix still
        // engages and reports the real total length, while memory stays
        // bounded to roughly max_output_chars.
        let collect_budget = self.max_output_chars + 512;
        let limit_max = limit.unwrap_or(usize::MAX);
        let mut line_bytes: Vec<u8> = Vec::with_capacity(256);

        loop {
            line_bytes.clear();
            let n = match reader.read_until(b'\n', &mut line_bytes) {
                Ok(n) => n,
                Err(e) => return Err(AppError::Io(e)),
            };
            if n == 0 {
                break;
            }
            hash = crate::tools::stale_edit::fnv64_update(hash, &line_bytes);

            let owned;
            let text = match std::str::from_utf8(&line_bytes) {
                Ok(t) => t,
                Err(_) => {
                    // NUL bytes mean real binary; otherwise this is a legacy
                    // CJK encoding (GBK) — decode it instead of breaking the
                    // whole tool chain on Windows source files.
                    if line_bytes.contains(&0) {
                        return Ok(ToolResult::error(format!(
                            "File appears to be binary ({:?}). Cannot display as text.",
                            path_buf
                        )));
                    }
                    owned = crate::core::encoding::decode_native_output(&line_bytes);
                    owned.as_str()
                }
            };
            let text = text.strip_suffix('\r').unwrap_or(text);

            if line_index >= offset
                && collected.len() < limit_max
                && collected_chars <= collect_budget
            {
                collected.push(text.to_string());
                collected_chars += text.len();
            }
            line_index += 1;
        }

        let total_lines = line_index;
        let start = offset.min(total_lines);
        let end = limit
            .map(|l| (start + l).min(total_lines))
            .unwrap_or(total_lines);

        let mut output = String::new();
        for (i, line) in collected.iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{:>6}|{}\n", line_num, line));
        }

        if end < total_lines {
            output.push_str(&format!("\n...({} more lines)", total_lines - end));
        }

        // Record the fingerprint the agent just saw — the stale-edit guard
        // uses it to refuse writes that would overwrite changes made after
        // this read (see tools/stale_edit.rs).
        crate::tools::stale_edit::record_seen_hash(
            &context.app,
            &context.session_id,
            context.workspace.as_deref(),
            path_buf,
            hash,
        )
        .await;

        // Offset beyond the file's line count silently produced an empty
        // window (real session 2d02f3dc: offset=225 on a ~210-line file
        // made the model believe the file was empty/unreadable and it
        // started bypassing the edit tools). Say it explicitly instead.
        if offset > 0 && offset >= total_lines {
            return Ok(ToolResult::success(format!(
                "offset {} is beyond the file's {} line(s) — no lines matched. \
                     The file exists and is readable; re-read with offset 0 (or \
                     a smaller offset) to see the content.",
                offset, total_lines
            ))
            .with_metadata("total_lines", json!(total_lines)));
        }

        // Truncate if too long
        if output.len() > self.max_output_chars {
            output = format!(
                "{}\n\n...(output truncated, showing {} of {} chars)",
                crate::core::str_util::truncate_at_char_boundary(&output, self.max_output_chars),
                self.max_output_chars,
                output.len()
            );
        }

        Ok(ToolResult::success(output).with_metadata("total_lines", json!(total_lines)))
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Supports reading specific line ranges with offset and limit. Returns the file content with line numbers."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read. Relative paths resolve against the workspace root."
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (0-indexed). Defaults to 0."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read. Defaults to all lines."
                }
            },
            "required": ["path"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path = args.get("path").and_then(|p| p.as_str()).ok_or_else(|| {
            crate::core::error::AppError::Parse("Missing 'path' parameter".into())
        })?;

        let offset = args.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|l| l.as_u64())
            .map(|l| l as usize);

        let path_buf = super::resolve_path(context.workspace.as_deref(), path);

        if !path_buf.exists() {
            return Ok(ToolResult::error(format!("File not found: {}", path)));
        }

        if path_buf.is_dir() {
            return Ok(ToolResult::error(format!(
                "Path is a directory, not a file: {}",
                path
            )));
        }

        if offset > 0 || limit.is_some() {
            return self
                .execute_range(path, &path_buf, offset, limit, context)
                .await;
        }

        // Size guard: the default (no offset/limit) read slurps the WHOLE
        // file into memory, then makes a UTF-8 copy and builds the entire
        // numbered output before truncation — a multi-GB log or minified file
        // would OOM (3-4x its size in transient allocations). Steer the model
        // to the bounded range path instead.
        const FULL_READ_MAX_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB
        if let Ok(meta) = std::fs::metadata(&path_buf) {
            if meta.len() > FULL_READ_MAX_BYTES {
                return Ok(ToolResult::error(format!(
                    "File '{}' is {} bytes — too large to read in full. \
                     Pass `limit` (or `offset` + `limit`) to read a window \
                     instead, e.g. limit: 200 for the first 200 lines.",
                    path, meta.len()
                )));
            }
        }

        // Read raw bytes first — an image needs the binary payload (compressed
        // and base64-embedded) rather than a lossy text decode.
        let bytes = match std::fs::read(&path_buf) {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file '{}': {}",
                    path, e
                )));
            }
        };

        // Image branch — split by main model capability: vision-capable
        // models get the picture embedded natively (read_file_image); text-only
        // models (DeepSeek) get an automatic transcription from the configured
        // vision model.
        if let Some(mime) = super::read_file_image::is_supported_image(&bytes) {
            return self.image_result(path, bytes, mime, context, "").await;
        }

        // Document branch: extract text from PDF / docx / pptx.
        if let Some(kind) = super::read_file_document::detect_document(&bytes) {
            return self.document_result(&path_buf, bytes, kind, "");
        }

        let (content, seen_bytes) = match String::from_utf8(bytes) {
            Ok(c) => {
                let seen = c.as_bytes().to_vec();
                (c, seen)
            }
            Err(e) => {
                let raw = e.into_bytes();
                if raw.contains(&0) {
                    // Not UTF-8 text and not an embeddable image — binary.
                    return Ok(ToolResult::error(format!(
                        "File appears to be binary ({:?}). Cannot display as text.",
                        path_buf
                    )));
                }
                // Legacy CJK encoding (GBK) — readable text, not binary.
                (crate::core::encoding::decode_native_output(&raw), raw)
            }
        };

        // Record the fingerprint the agent just saw — the stale-edit guard
        // uses it to refuse writes that would overwrite changes made after
        // this read (see tools/stale_edit.rs).
        crate::tools::stale_edit::record_seen(
            &context.app,
            &context.session_id,
            context.workspace.as_deref(),
            &path_buf,
            &seen_bytes,
        )
        .await;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.min(total_lines);
        let end = limit
            .map(|l| (start + l).min(total_lines))
            .unwrap_or(total_lines);

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{:>6}|{}\n", line_num, line));
        }

        if end < total_lines {
            output.push_str(&format!("\n...({} more lines)", total_lines - end));
        }

        // Truncate if too long
        if output.len() > self.max_output_chars {
            output = format!(
                "{}\n\n...(output truncated, showing {} of {} chars)",
                crate::core::str_util::truncate_at_char_boundary(&output, self.max_output_chars),
                self.max_output_chars,
                output.len()
            );
        }

        Ok(ToolResult::success(output).with_metadata("total_lines", json!(total_lines)))
    }
}
