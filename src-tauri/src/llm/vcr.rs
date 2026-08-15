//! LLM VCR — record and replay LLM API calls for tests and offline runs.
//!
//! Captures the parsed chunk stream of every request (fingerprinted by
//! model + provider + messages) into a JSONL file per session directory.
//! Replay mode serves the recorded chunks without any HTTP call, so test
//! suites can run deterministically and offline.
//!
//! Enabled via environment variables (never via UI — it is a test/dev tool):
//! - `DEEPDEPCAT_VCR=record|replay|off` (default: off)
//! - `DEEPDEPCAT_VCR_DIR=<dir>` (default: `{app_data}/vcr`)

use crate::core::error::{AppError, AppResult};
use crate::llm::provider::LlmRequest;
use crate::llm::streaming::StreamChunk;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

/// VCR mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcrMode {
    Off,
    Record,
    Replay,
}

impl VcrMode {
    /// Parse from `DEEPDEPCAT_VCR`.
    pub fn from_env() -> Self {
        match std::env::var("DEEPDEPCAT_VCR").as_deref() {
            Ok("record") => Self::Record,
            Ok("replay") => Self::Replay,
            _ => Self::Off,
        }
    }
}

/// One JSONL entry: a request fingerprint plus its recorded chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChunkEntry {
    key: String,
    chunks: Vec<StreamChunk>,
}

/// One JSONL entry: a request fingerprint plus its non-streaming response.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResponseEntry {
    key: String,
    text: String,
}

/// The VCR recorder/replayer.
#[derive(Debug, Clone)]
pub struct LlmVcr {
    mode: VcrMode,
    dir: PathBuf,
}

impl LlmVcr {
    /// Create from environment variables with a default directory.
    pub fn from_env(app_data_dir: &std::path::Path) -> Self {
        let dir = std::env::var("DEEPDEPCAT_VCR_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| app_data_dir.join("vcr"));
        Self::new(VcrMode::from_env(), dir)
    }

    /// Create with an explicit mode and directory.
    pub fn new(mode: VcrMode, dir: PathBuf) -> Self {
        if mode != VcrMode::Off {
            let _ = std::fs::create_dir_all(&dir);
        }
        Self { mode, dir }
    }

    pub fn recording(&self) -> bool {
        self.mode == VcrMode::Record
    }

    pub fn replaying(&self) -> bool {
        self.mode == VcrMode::Replay
    }

    /// Stable request fingerprint — identical inputs replay identically.
    ///
    /// Covers the model, provider hint, system prompt, every message, the
    /// tool schemas and the reasoning-effort knob — the tool list and effort
    /// change what the provider returns as strongly as the messages do, so
    /// recordings made without them replay wrong answers for different
    /// requests.
    pub fn fingerprint(&self, request: &LlmRequest) -> String {
        let mut h = FNV_OFFSET;
        let mut feed = |s: &str| {
            for b in s.as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(FNV_PRIME);
            }
            h ^= 0xFF;
            h = h.wrapping_mul(FNV_PRIME);
        };
        feed(&request.model);
        feed(&request.provider.clone().unwrap_or_default());
        feed(&request.system_prompt);
        for item in &request.messages {
            let text = match item {
                crate::core::types::ConversationItem::User(u) => {
                    serde_json::to_string(&u.content).unwrap_or_default()
                }
                crate::core::types::ConversationItem::Assistant(a) => a.content.clone(),
                crate::core::types::ConversationItem::System(s) => s.content.clone(),
                crate::core::types::ConversationItem::ToolResult(tr) => tr.content.clone(),
                crate::core::types::ConversationItem::Reasoning(r) => r.content.clone(),
            };
            feed(&text);
        }
        for tool in &request.tools {
            feed(&serde_json::to_string(tool).unwrap_or_default());
        }
        if let Some(effort) = &request.reasoning_effort {
            feed(effort);
        }
        format!("{h:016x}")
    }

    /// Append a recorded chunk stream for a key.
    pub fn record_chunks(&self, key: &str, chunks: &[StreamChunk]) -> AppResult<()> {
        if !self.recording() {
            return Ok(());
        }
        let entry = ChunkEntry {
            key: key.to_string(),
            chunks: chunks.to_vec(),
        };
        let line = serde_json::to_string(&entry)
            .map_err(|e| AppError::Config(format!("VCR encode failed: {e}")))?;
        self.append_line("chunks.jsonl", &line)?;
        info!(key = %key, chunks = chunks.len(), "VCR recorded chunk stream");
        Ok(())
    }

    /// Replay a recorded chunk stream for a key. `None` on miss.
    pub fn replay_chunks(&self, key: &str) -> Option<Vec<StreamChunk>> {
        let lines = self.read_lines("chunks.jsonl")?;
        for line in lines.iter().rev() {
            let entry: ChunkEntry = serde_json::from_str(line).ok()?;
            if entry.key == key {
                debug!(key = %key, chunks = entry.chunks.len(), "VCR replay hit");
                return Some(entry.chunks);
            }
        }
        None
    }

    /// Append a recorded non-streaming response for a key.
    pub fn record_response(&self, key: &str, text: &str) -> AppResult<()> {
        if !self.recording() {
            return Ok(());
        }
        let entry = ResponseEntry {
            key: key.to_string(),
            text: text.to_string(),
        };
        let line = serde_json::to_string(&entry)
            .map_err(|e| AppError::Config(format!("VCR encode failed: {e}")))?;
        self.append_line("responses.jsonl", &line)?;
        Ok(())
    }

    /// Replay a recorded non-streaming response for a key.
    pub fn replay_response(&self, key: &str) -> Option<String> {
        let lines = self.read_lines("responses.jsonl")?;
        for line in lines.iter().rev() {
            let entry: ResponseEntry = serde_json::from_str(line).ok()?;
            if entry.key == key {
                return Some(entry.text);
            }
        }
        None
    }

    fn append_line(&self, file: &str, line: &str) -> AppResult<()> {
        let path = self.dir.join(file);
        // O_APPEND append: each line is written in one call, so concurrent
        // recorders (parallel sessions) never clobber each other — the old
        // read-whole-file/rewrite lost every interleaved line.
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| AppError::Config(format!("VCR open failed: {e}")))?;
        use std::io::Write;
        // Guard against merging with a previous line that lacks a trailing
        // newline (crash mid-write under the old implementation).
        if out.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
            let last = std::fs::read(&path)
                .ok()
                .and_then(|b| b.last().copied())
                .unwrap_or(b'\n');
            if last != b'\n' {
                out.write_all(b"\n")
                    .map_err(|e| AppError::Config(format!("VCR write failed: {e}")))?;
            }
        }
        out.write_all(line.as_bytes())
            .and_then(|()| out.write_all(b"\n"))
            .map_err(|e| AppError::Config(format!("VCR write failed: {e}")))
    }

    fn read_lines(&self, file: &str) -> Option<Vec<String>> {
        let path = self.dir.join(file);
        let content = std::fs::read_to_string(&path).ok()?;
        Some(content.lines().map(|l| l.to_string()).collect())
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ConversationItem;

    fn request(model: &str, msg: &str) -> LlmRequest {
        LlmRequest {
            model: model.to_string(),
            provider: Some("deepseek".to_string()),
            messages: vec![ConversationItem::user(msg)],
            tools: vec![],
            system_prompt: "sys".to_string(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: true,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        }
    }

    #[test]
    fn mode_parses_from_env() {
        std::env::set_var("DEEPDEPCAT_VCR", "record");
        assert_eq!(VcrMode::from_env(), VcrMode::Record);
        std::env::set_var("DEEPDEPCAT_VCR", "replay");
        assert_eq!(VcrMode::from_env(), VcrMode::Replay);
        std::env::set_var("DEEPDEPCAT_VCR", "off");
        assert_eq!(VcrMode::from_env(), VcrMode::Off);
        std::env::remove_var("DEEPDEPCAT_VCR");
        assert_eq!(VcrMode::from_env(), VcrMode::Off);
    }

    #[test]
    fn fingerprint_stable_and_distinct() {
        let vcr = LlmVcr::new(VcrMode::Off, PathBuf::from("/tmp"));
        let a = request("deepseek-v4-flash", "hello world");
        let b = request("deepseek-v4-flash", "hello world");
        let c = request("deepseek-v4-pro", "hello world");
        let d = request("deepseek-v4-flash", "hello world!");
        assert_eq!(vcr.fingerprint(&a), vcr.fingerprint(&b));
        assert_ne!(vcr.fingerprint(&a), vcr.fingerprint(&c));
        assert_ne!(vcr.fingerprint(&a), vcr.fingerprint(&d));
    }

    #[test]
    fn record_then_replay_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let vcr = LlmVcr::new(VcrMode::Record, tmp.path().to_path_buf());
        let key = "k1";
        let chunks = vec![
            StreamChunk::TextDelta {
                text: "hello ".to_string(),
            },
            StreamChunk::TextDelta {
                text: "world".to_string(),
            },
            StreamChunk::Finish {
                reason: "stop".to_string(),
            },
        ];
        vcr.record_chunks(key, &chunks).unwrap();
        vcr.record_response(key, "full response").unwrap();

        let replay = LlmVcr::new(VcrMode::Replay, tmp.path().to_path_buf());
        let got = replay.replay_chunks(key).unwrap();
        assert_eq!(got.len(), 3);
        // StreamChunk is not PartialEq — compare via serialization.
        let enc = |c: &[StreamChunk]| serde_json::to_string(c).unwrap();
        assert_eq!(enc(&got), enc(&chunks));
        assert_eq!(
            replay.replay_response(key).as_deref(),
            Some("full response")
        );
        assert!(replay.replay_chunks("missing").is_none());
    }

    #[test]
    fn off_mode_does_not_record() {
        let tmp = tempfile::tempdir().unwrap();
        let vcr = LlmVcr::new(VcrMode::Off, tmp.path().to_path_buf());
        vcr.record_chunks("k", &[]).unwrap();
        vcr.record_response("k", "x").unwrap();
        assert!(!tmp.path().join("chunks.jsonl").exists());
    }

    #[test]
    fn replay_uses_latest_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let vcr = LlmVcr::new(VcrMode::Record, tmp.path().to_path_buf());
        vcr.record_chunks("k", &[StreamChunk::TextDelta { text: "v1".into() }])
            .unwrap();
        vcr.record_chunks("k", &[StreamChunk::TextDelta { text: "v2".into() }])
            .unwrap();
        let replay = LlmVcr::new(VcrMode::Replay, tmp.path().to_path_buf());
        let got = replay.replay_chunks("k").unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(&got[0], StreamChunk::TextDelta { text } if text == "v2"));
    }
}
