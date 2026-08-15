//! live_doc_write — stream a full document into an open WPS/Word window.
//!
//! One tool call that writes the whole deliverable with typewriter pacing:
//! the content is split into readable chunks and each chunk is appended to
//! the document through the persistent office host (`office_automate`'s
//! `host_call` / Selection.TypeText), so the user watches the document
//! being written live in the SAME open office window — nothing is closed,
//! reopened, or dumped as a finished file.
//!
//! Path can be omitted (or "active") to write into whatever document the
//! user is currently looking at.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Emitter;

/// Maximum characters in a single typing chunk (keeps the chunk below the
/// host's comfortable pacing and gives a natural paragraph rhythm).
const MAX_CHUNK_CHARS: usize = 400;

/// Split markdown-ish text into chunks that read like paragraphs: split on
/// blank lines first, then break any oversized paragraph at a sentence-ish
/// boundary (whitespace after punctuation) or hard window.
fn split_chunks(content: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for block in content.split('\n') {
        let block = block.trim_end_matches('\r');
        if block.trim().is_empty() {
            // Blank line ends the current chunk.
            if !current.trim().is_empty() {
                chunks.push(current.trim_end().to_string());
                current.clear();
            }
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(block);
        if current.chars().count() >= MAX_CHUNK_CHARS {
            // Hard-split any oversized buffer by character window.
            while current.chars().count() > MAX_CHUNK_CHARS {
                let take: String = current.chars().take(MAX_CHUNK_CHARS).collect();
                chunks.push(take);
                current = current.chars().skip(MAX_CHUNK_CHARS).collect();
            }
            chunks.push(current.trim_end().to_string());
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim_end().to_string());
    }
    chunks
}

/// Live document writing tool.
pub struct LiveDocWriteTool;

impl LiveDocWriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LiveDocWriteTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "live_doc_write"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        "Write a full document into an OPEN WPS/Word window with typewriter \
        pacing, so the user watches it being typed live (window stays open, \
        nothing is reopened). Content is split into chunks and appended \
        continuously. Path is optional: omit it or use \"active\" to write \
        into the user's CURRENT document. Prefer this over writing to a file \
        when the user wants to SEE the writing happen."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional document path (.docx/.wps). OMIT or use \"active\" to write into the user's CURRENTLY OPEN document."
                },
                "content": {
                    "type": "string",
                    "description": "The full document text (paragraphs separated by blank lines)."
                },
                "pace": {
                    "type": "integer",
                    "description": "Milliseconds per typing chunk (default 180 — the office host's type_text pace; larger = slower typing)."
                },
                "chunk": {
                    "type": "integer",
                    "description": "Characters per keystroke burst (default 4 — the office host's type_text chunk; smaller = slower typing)."
                },
                "pause_ms": {
                    "type": "integer",
                    "description": "Pause between content chunks in ms (default 800)."
                }
            },
            "required": ["content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Self-approval: typing into a pathless/active doc asks (cannot tell
    /// what it targets); a named file skips the prompt when it is new or
    /// the session's own output. Runs after deny rules.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(p) = args.get("path").and_then(|p| p.as_str()) else {
            return PermissionDecision::Ask;
        };
        if p.is_empty() || p == "active" {
            return PermissionDecision::Ask;
        }
        let target = super::permissions::resolve_target(context.workspace.as_deref(), p, None);
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;
        if content.trim().is_empty() {
            return Ok(ToolResult::error("content is empty — nothing to write"));
        }

        let mut config = json!({ "action": "type_text", "app": "wps" });
        if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
            if !p.is_empty() && p != "active" {
                let resolved = crate::tools::builtin::resolve_path(None, p);
                config["path"] = json!(resolved.to_string_lossy());
            }
        }
        if let Some(pace) = args.get("pace").and_then(|v| v.as_u64()) {
            config["pace"] = json!(pace);
        }
        if let Some(chunk) = args.get("chunk").and_then(|v| v.as_u64()) {
            config["chunk"] = json!(chunk);
        }
        let pause_ms = args.get("pause_ms").and_then(|v| v.as_u64()).unwrap_or(800);

        let chunks = split_chunks(content);
        if chunks.is_empty() {
            return Ok(ToolResult::error("content produced no writable chunks"));
        }

        let target = args
            .get("path")
            .and_then(|p| p.as_str())
            .filter(|p| !p.is_empty() && *p != "active")
            .map(str::to_string)
            .unwrap_or_else(|| "the user's current document".to_string());
        let target_short = std::path::Path::new(&target)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| target.clone());

        let mut written = 0usize;
        let mut last_error: Option<String> = None;
        for (i, chunk_text) in chunks.iter().enumerate() {
            // Live progress: the frontend shows a small "typing in WPS
            // window" hint while the document is being written.
            let _ = context.app.emit(
                "office-typing",
                json!({
                    "active": true,
                    "chunk": i + 1,
                    "total": chunks.len(),
                    "chars": written,
                    "target": target_short,
                }),
            );
            config["text"] = json!(chunk_text);
            match crate::tools::builtin::depwork::office_automate::host_call(&config) {
                Ok(resp) => {
                    if let Some(err) = resp.get("error").and_then(|e| e.as_str()) {
                        if err == "NO_OFFICE" {
                            let _ = context
                                .app
                                .emit("office-typing", json!({ "active": false }));
                            return Ok(ToolResult::error(
                                crate::tools::builtin::depwork::office_automate::fallback_hint(
                                    "type_text",
                                    std::path::Path::new(""),
                                ),
                            ));
                        }
                        last_error = Some(err.to_string());
                        break;
                    }
                    written += chunk_text.chars().count();
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                    break;
                }
            }
            if i + 1 < chunks.len() && pause_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await;
            }
        }
        let _ = context.app.emit(
            "office-typing",
            json!({ "active": false, "chars": written, "target": target_short }),
        );

        let written_str = format!(
            "Written {written} characters in {}/{} chunks into {target} — \
             visible live in the open office window.",
            chunks.len(),
            chunks.len(),
        );
        match last_error {
            None => Ok(ToolResult::success(written_str)),
            Some(err) => Ok(ToolResult::error(format!(
                "{written_str}\nInterrupted: {err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_blank_lines() {
        let chunks = split_chunks("第一段。\n\n第二段。\n\n第三段。");
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "第一段。");
        assert_eq!(chunks[2], "第三段。");
    }

    #[test]
    fn splits_oversized_blocks_at_window() {
        let big = "字".repeat(1200);
        let chunks = split_chunks(&big);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= MAX_CHUNK_CHARS));
        let joined: String = chunks.concat();
        assert_eq!(joined.chars().count(), 1200);
    }

    #[test]
    fn handles_crlf_and_trailing_blank() {
        let chunks = split_chunks("一\r\n\r\n二\r\n\r\n");
        assert_eq!(chunks, vec!["一".to_string(), "二".to_string()]);
    }

    #[test]
    fn single_line_content_is_one_chunk() {
        let chunks = split_chunks("只有一行。");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "只有一行。");
    }
}
