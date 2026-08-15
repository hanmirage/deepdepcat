//! SSE stream parsing — converts raw HTTP byte streams into typed chunks.
//!
//! Handles both OpenAI-compatible SSE format (`data: {...}\n\n`) and
//! Anthropic's event format (`event: content_block_delta\ndata: {...}`).

use crate::core::types::tool::ToolCall;
use crate::core::types::TokenUsage;
use serde::{Deserialize, Serialize};

/// A single chunk parsed from the LLM stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamChunk {
    /// A text content delta.
    TextDelta { text: String },
    /// Start of a tool call.
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Arguments delta for a tool call.
    ToolCallDelta { index: usize, arguments: String },
    /// End of a tool call (arguments are complete).
    ToolCallEnd { index: usize },
    /// Usage information (may arrive mid-stream or at end).
    Usage { usage: TokenUsage },
    /// DeepSeek thinking mode: reasoning content delta in stream.
    /// Also emitted by Responses-family providers (reasoning_text deltas)
    /// and Anthropic-compatible streams — not a DeepSeek-exclusive signal.
    // deepseek-native: reasoning deltas are consumed by the agent loop's
    // reasoning flusher and echoed back on tool-call turns.
    ReasoningDelta { text: String },
    /// The model finished generating.
    Finish { reason: String },
    /// An error occurred during streaming.
    Error { message: String },
}

/// Parse a provider `usage` JSON object into `TokenUsage`.
///
/// Shared by the streaming parsers (which emit `StreamChunk::Usage`) and
/// the non-streaming response parsers (which previously dropped usage
/// entirely — #88 audit H7). Covers OpenAI-compatible fields
/// (`prompt_tokens` / `completion_tokens` / `prompt_tokens_details.
/// cached_tokens` / `completion_tokens_details.reasoning_tokens`) plus the
/// deepseek-native KV cache fields (`prompt_cache_hit_tokens` /
/// `prompt_cache_miss_tokens`). Missing fields default to zero.
pub fn parse_usage_object(usage: &serde_json::Value) -> TokenUsage {
    let prompt = usage
        .get("prompt_tokens")
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let completion = usage
        .get("completion_tokens")
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|t| t.as_u64());
    let reasoning = usage
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|t| t.as_u64());
    // deepseek-native: KV cache hit/miss tokens
    let cache_hit = usage
        .get("prompt_cache_hit_tokens")
        .and_then(|t| t.as_u64());
    let cache_miss = usage
        .get("prompt_cache_miss_tokens")
        .and_then(|t| t.as_u64());
    TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        cached_read_tokens: cached,
        reasoning_tokens: reasoning,
        prompt_cache_hit_tokens: cache_hit,
        prompt_cache_miss_tokens: cache_miss,
    }
}

/// Parser for SSE (Server-Sent Events) streams.
///
/// Accumulates raw bytes into complete SSE events, then delegates to
/// the appropriate format-specific parser.
pub struct StreamParser {
    /// The format of the stream.
    format: StreamFormat,
    /// Buffer for incomplete SSE data.
    buffer: String,
    /// Accumulated tool calls (indexed by position).
    tool_calls: Vec<ToolCallAccumulator>,
}

/// Hard cap on the incomplete-SSE buffer. A well-formed provider stream
/// delivers `\n\n`-terminated events continuously, so an unfinished event
/// larger than this means a broken/garbage stream (or a stalled connection
/// silently buffering). Beyond the cap the buffer is dropped and an error
/// chunk surfaces instead of growing memory without bound.
const MAX_BUFFER_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormat {
    /// OpenAI-compatible: `data: {"choices":[{"delta":{...}}]}\n\n`
    OpenAi,
    /// Anthropic: `event: content_block_delta\ndata: {...}\n\n`
    Anthropic,
    /// OpenAI Responses API: `event: response.output_text.delta\ndata: {...}\n\n`
    /// (payloads carry a top-level `type` discriminator)
    Responses,
}

/// Accumulator for building tool calls from stream deltas.
///
/// Used by [`StreamParser`] internally and by the agent loop to assemble
/// tool calls from `ToolCallStart` → `ToolCallDelta` → `ToolCallEnd` chunks.
#[derive(Debug, Clone, Default)]
pub struct ToolCallAccumulator {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCallAccumulator {
    /// Deduplicate tool calls by id.
    ///
    /// DeepSeek / OpenAI-compatible streams occasionally declare the same
    /// logical call twice (two stream indices sharing one call id). The
    /// first declaration keeps its position; a later duplicate with MORE
    /// complete arguments upgrades the kept call. Order is preserved.
    /// Without this the assistant message would carry a duplicate
    /// `call_id` and the NEXT request fails with HTTP 400
    /// "Duplicate 'call_id'".
    pub fn dedupe_tool_calls_by_id(calls: Vec<ToolCall>) -> Vec<ToolCall> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<ToolCall> = Vec::with_capacity(calls.len());
        for tc in calls {
            if seen.insert(tc.id.clone()) {
                out.push(tc);
            } else if let Some(last) = out.iter_mut().find(|c| c.id == tc.id) {
                if tc.arguments.len() > last.arguments.len() {
                    last.arguments = tc.arguments;
                    last.name = tc.name;
                }
            }
        }
        out
    }
}

impl StreamParser {
    pub fn new(format: StreamFormat) -> Self {
        Self {
            format,
            buffer: String::new(),
            tool_calls: vec![],
        }
    }

    /// Find the next event terminator, honoring both LF (`\n\n`) and CRLF
    /// (`\r\n\r\n`) line endings — some providers and proxies emit CRLF, and
    /// an SSE stream that only ever sees `\r\n` would previously never split
    /// (buffering until the 16MB cap blew the whole stream).
    fn find_event_boundary(&self) -> Option<usize> {
        match (self.buffer.find("\n\n"), self.buffer.find("\r\n\r\n")) {
            (Some(lf), Some(crlf)) => Some(lf.min(crlf)),
            (Some(lf), None) => Some(lf),
            (None, crlf) => crlf,
        }
    }

    /// Feed raw text data and return any complete chunks parsed.
    pub fn feed(&mut self, data: &str) -> Vec<StreamChunk> {
        self.buffer.push_str(data);
        if self.buffer.len() > MAX_BUFFER_BYTES {
            // No event terminator in sight for 16MB — the stream is broken.
            // Drop the buffer so memory stays bounded; the error chunk
            // aborts the request instead of feeding a truncated parse.
            self.buffer.clear();
            return vec![StreamChunk::Error {
                message: format!(
                    "SSE stream exceeded the {} byte event buffer without a terminator",
                    MAX_BUFFER_BYTES
                ),
            }];
        }
        let mut chunks = Vec::new();

        while let Some(pos) = self.find_event_boundary() {
            // `\n\n` (2 bytes) vs `\r\n\r\n` (4 bytes) — consume exactly the
            // terminator that matched so the next scan starts clean.
            let consumed = if self.buffer[pos..].starts_with("\r\n\r\n") {
                4
            } else {
                2
            };
            let event_text = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + consumed..].to_string();

            if event_text.trim().is_empty() {
                continue;
            }

            // Parse the event based on format
            match self.format {
                StreamFormat::OpenAi => {
                    if let Some(chunk) = self.parse_openai_event(&event_text) {
                        chunks.extend(chunk);
                    }
                }
                StreamFormat::Anthropic => {
                    if let Some(chunk) = self.parse_anthropic_event(&event_text) {
                        chunks.extend(chunk);
                    }
                }
                StreamFormat::Responses => {
                    if let Some(chunk) = self.parse_responses_event(&event_text) {
                        chunks.extend(chunk);
                    }
                }
            }
        }

        chunks
    }

    /// Parse an OpenAI-compatible SSE event.
    fn parse_openai_event(&mut self, event_text: &str) -> Option<Vec<StreamChunk>> {
        let mut chunks = Vec::new();

        for line in event_text.lines() {
            let line = line.trim();
            if !line.starts_with("data: ") {
                continue;
            }
            let json_str = &line[6..];

            if json_str == "[DONE]" {
                chunks.push(StreamChunk::Finish {
                    reason: "stop".to_string(),
                });
                continue;
            }

            let value: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Extract choices
            if let Some(choices) = value.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    if let Some(delta) = choice.get("delta") {
                        // Text content
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                            if !content.is_empty() {
                                chunks.push(StreamChunk::TextDelta {
                                    text: content.to_string(),
                                });
                            }
                        }

                        // deepseek-native: reasoning_content delta (thinking mode)
                        if let Some(rc) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                            if !rc.is_empty() {
                                chunks.push(StreamChunk::ReasoningDelta {
                                    text: rc.to_string(),
                                });
                            }
                        }

                        // Tool calls
                        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array())
                        {
                            for tc in tool_calls {
                                let index =
                                    tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                                // Ensure we have enough slots
                                while self.tool_calls.len() <= index {
                                    self.tool_calls.push(ToolCallAccumulator::default());
                                }

                                let acc = &mut self.tool_calls[index];

                                if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                    acc.id = id.to_string();
                                    if let Some(function) = tc.get("function") {
                                        if let Some(name) =
                                            function.get("name").and_then(|n| n.as_str())
                                        {
                                            acc.name = name.to_string();
                                            chunks.push(StreamChunk::ToolCallStart {
                                                index,
                                                id: acc.id.clone(),
                                                name: acc.name.clone(),
                                            });
                                        }
                                    }
                                }

                                if let Some(function) = tc.get("function") {
                                    if let Some(args) =
                                        function.get("arguments").and_then(|a| a.as_str())
                                    {
                                        acc.arguments.push_str(args);
                                        chunks.push(StreamChunk::ToolCallDelta {
                                            index,
                                            arguments: args.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Finish reason
                    if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                        if reason != "null" {
                            chunks.push(StreamChunk::Finish {
                                reason: reason.to_string(),
                            });
                        }
                    }
                }
            }

            // Usage (may come at the end)
            if let Some(usage) = value.get("usage") {
                chunks.push(StreamChunk::Usage {
                    usage: parse_usage_object(usage),
                });
            }
        }

        if chunks.is_empty() {
            None
        } else {
            Some(chunks)
        }
    }

    /// Parse an Anthropic SSE event.
    fn parse_anthropic_event(&mut self, event_text: &str) -> Option<Vec<StreamChunk>> {
        let mut chunks = Vec::new();
        let mut event_type = String::new();
        let mut data_json: Option<serde_json::Value> = None;

        for line in event_text.lines() {
            let line = line.trim();
            if let Some(stripped) = line.strip_prefix("event: ") {
                event_type = stripped.to_string();
            } else if let Some(stripped) = line.strip_prefix("data: ") {
                data_json = serde_json::from_str(stripped).ok();
            }
        }

        let data = data_json?;
        match event_type.as_str() {
            "content_block_start" => {
                if let Some(block) = data.get("content_block") {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let id = block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let index =
                            data.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                        while self.tool_calls.len() <= index {
                            self.tool_calls.push(ToolCallAccumulator::default());
                        }
                        self.tool_calls[index].id = id.clone();
                        self.tool_calls[index].name = name.clone();

                        chunks.push(StreamChunk::ToolCallStart { index, id, name });
                    }
                }
            }
            "content_block_delta" => {
                if let Some(delta) = data.get("delta") {
                    let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match delta_type {
                        "text_delta" => {
                            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                chunks.push(StreamChunk::TextDelta {
                                    text: text.to_string(),
                                });
                            }
                        }
                        "input_json_delta" => {
                            let index =
                                data.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            if let Some(partial) =
                                delta.get("partial_json").and_then(|p| p.as_str())
                            {
                                if index < self.tool_calls.len() {
                                    self.tool_calls[index].arguments.push_str(partial);
                                }
                                chunks.push(StreamChunk::ToolCallDelta {
                                    index,
                                    arguments: partial.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let index = data.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                chunks.push(StreamChunk::ToolCallEnd { index });
            }
            "message_delta" => {
                if let Some(delta) = data.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(|r| r.as_str()) {
                        // Anthropic's truncation stop_reason is "max_tokens";
                        // normalize it to the provider-agnostic "length" the
                        // agent loop's truncation-recovery path matches
                        // (#88 audit H4 — before this, Anthropic cut-off
                        // answers were silently accepted as complete).
                        let normalized = if reason == "max_tokens" {
                            "length"
                        } else {
                            reason
                        };
                        chunks.push(StreamChunk::Finish {
                            reason: normalized.to_string(),
                        });
                    }
                }
                if let Some(usage) = data.get("usage") {
                    let output = usage
                        .get("output_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    // Anthropic reports cache metrics in the message_delta
                    // usage too when a cache breakpoint is crossed mid-stream.
                    // Map read → cache hit, creation → cache miss (same 口径
                    // as the non-streaming parser).
                    let cache_read = usage
                        .get("cache_read_input_tokens")
                        .and_then(|t| t.as_u64());
                    let cache_creation = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|t| t.as_u64());
                    chunks.push(StreamChunk::Usage {
                        usage: TokenUsage {
                            prompt_tokens: 0,
                            completion_tokens: output,
                            cached_read_tokens: cache_read,
                            reasoning_tokens: None,
                            prompt_cache_hit_tokens: cache_read,
                            prompt_cache_miss_tokens: cache_creation,
                        },
                    });
                }
            }
            "message_start" => {
                if let Some(message) = data.get("message") {
                    if let Some(usage) = message.get("usage") {
                        let input = usage
                            .get("input_tokens")
                            .and_then(|t| t.as_u64())
                            .unwrap_or(0);
                        // cache_read → cache hit, cache_creation → cache miss
                        // (same 口径 as the non-streaming parser) so the
                        // usage ring sees the KV discount.
                        let cache_read = usage
                            .get("cache_read_input_tokens")
                            .and_then(|t| t.as_u64());
                        let cache_creation = usage
                            .get("cache_creation_input_tokens")
                            .and_then(|t| t.as_u64());
                        chunks.push(StreamChunk::Usage {
                            usage: TokenUsage {
                                prompt_tokens: input,
                                completion_tokens: 0,
                                cached_read_tokens: cache_read,
                                reasoning_tokens: None,
                                prompt_cache_hit_tokens: cache_read,
                                prompt_cache_miss_tokens: cache_creation,
                            },
                        });
                    }
                }
            }
            _ => {}
        }

        if chunks.is_empty() {
            None
        } else {
            Some(chunks)
        }
    }

    /// Parse an OpenAI Responses API SSE event.
    ///
    /// Responses events carry a top-level `type` discriminator in each
    /// payload, so the `event:` line is informational only:
    /// - `response.output_text.delta` / `response.reasoning_text.delta` → text
    /// - `response.output_item.added` (function_call) → tool-call start
    /// - `response.function_call_arguments.delta` → tool-call args delta
    /// - `response.output_item.done` (function_call) → tool-call end
    /// - `response.completed` → finish + usage
    fn parse_responses_event(&mut self, event_text: &str) -> Option<Vec<StreamChunk>> {
        let mut chunks = Vec::new();

        for line in event_text.lines() {
            let line = line.trim();
            if !line.starts_with("data: ") {
                continue;
            }
            let json_str = &line[6..];
            if json_str.is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match event_type {
                "response.output_text.delta" => {
                    if let Some(text) = value.get("delta").and_then(|d| d.as_str()) {
                        if !text.is_empty() {
                            chunks.push(StreamChunk::TextDelta {
                                text: text.to_string(),
                            });
                        }
                    }
                }
                "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                    if let Some(text) = value.get("delta").and_then(|d| d.as_str()) {
                        if !text.is_empty() {
                            chunks.push(StreamChunk::ReasoningDelta {
                                text: text.to_string(),
                            });
                        }
                    }
                }
                "response.output_item.added" => {
                    if let Some(item) = value.get("item") {
                        if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                            let index = value
                                .get("output_index")
                                .and_then(|i| i.as_u64())
                                .unwrap_or(0) as usize;
                            // CRITICAL: use `call_id` (call_xxx), NOT the item
                            // `id` (fc_xxx). The next request's
                            // function_call_output references `call_id` —
                            // using the item id yields HTTP 400 "No tool call
                            // found for tool output with call_id …".
                            let id = item
                                .get("call_id")
                                .and_then(|i| i.as_str())
                                .or_else(|| item.get("id").and_then(|i| i.as_str()))
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            while self.tool_calls.len() <= index {
                                self.tool_calls.push(ToolCallAccumulator::default());
                            }
                            self.tool_calls[index].id = id.clone();
                            self.tool_calls[index].name = name.clone();
                            chunks.push(StreamChunk::ToolCallStart { index, id, name });
                        }
                    }
                }
                "response.function_call_arguments.delta" => {
                    let index = value
                        .get("output_index")
                        .and_then(|i| i.as_u64())
                        .unwrap_or(0) as usize;
                    if let Some(partial) = value.get("delta").and_then(|d| d.as_str()) {
                        if index < self.tool_calls.len() {
                            self.tool_calls[index].arguments.push_str(partial);
                        }
                        chunks.push(StreamChunk::ToolCallDelta {
                            index,
                            arguments: partial.to_string(),
                        });
                    }
                }
                "response.output_item.done" => {
                    if let Some(item) = value.get("item") {
                        if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                            let index = value
                                .get("output_index")
                                .and_then(|i| i.as_u64())
                                .unwrap_or(0) as usize;
                            chunks.push(StreamChunk::ToolCallEnd { index });
                        }
                    }
                }
                "response.completed" => {
                    if let Some(resp) = value.get("response") {
                        let status = resp
                            .get("status")
                            .and_then(|s| s.as_str())
                            .unwrap_or("completed");
                        let reason = match status {
                            "incomplete" => "length",
                            "failed" => "error",
                            _ => "stop",
                        };
                        chunks.push(StreamChunk::Finish {
                            reason: reason.to_string(),
                        });
                        if let Some(usage) = resp.get("usage") {
                            let input = usage
                                .get("input_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0);
                            let output = usage
                                .get("output_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0);
                            let cached = usage
                                .get("input_tokens_details")
                                .and_then(|d| d.get("cached_tokens"))
                                .and_then(|t| t.as_u64());
                            let reasoning = usage
                                .get("output_tokens_details")
                                .and_then(|d| d.get("reasoning_tokens"))
                                .and_then(|t| t.as_u64());
                            chunks.push(StreamChunk::Usage {
                                usage: TokenUsage {
                                    prompt_tokens: input,
                                    completion_tokens: output,
                                    cached_read_tokens: cached,
                                    reasoning_tokens: reasoning,
                                    prompt_cache_hit_tokens: None,
                                    prompt_cache_miss_tokens: None,
                                },
                            });
                        }
                    }
                }
                "response.failed" | "error" => {
                    let message = value
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Responses API stream failed")
                        .to_string();
                    chunks.push(StreamChunk::Error { message });
                }
                _ => {}
            }
        }

        if chunks.is_empty() {
            None
        } else {
            Some(chunks)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_tool_calls_keeps_first_and_upgrades_arguments() {
        let calls = vec![
            ToolCall {
                id: "call-x".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            },
            ToolCall {
                id: "call-y".into(),
                name: "read_file".into(),
                arguments: "{\"path\": \"a\"}".into(),
            },
            // Duplicate declaration of call-x with MORE complete args —
            // the kept call must be upgraded, not duplicated.
            ToolCall {
                id: "call-x".into(),
                name: "bash".into(),
                arguments: "{\"command\": \"pwd\"}".into(),
            },
            // Another duplicate with shorter args — ignored.
            ToolCall {
                id: "call-y".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            },
        ];
        let out = ToolCallAccumulator::dedupe_tool_calls_by_id(calls);
        assert_eq!(out.len(), 2, "duplicates must collapse");
        assert_eq!(out[0].id, "call-x");
        assert_eq!(out[0].arguments, "{\"command\": \"pwd\"}");
        assert_eq!(out[1].id, "call-y");
        assert_eq!(out[1].arguments, "{\"path\": \"a\"}");
    }

    #[test]
    fn dedupe_tool_calls_handles_empty_input() {
        assert!(ToolCallAccumulator::dedupe_tool_calls_by_id(vec![]).is_empty());
    }

    fn responses_stream(chunks: &[&str]) -> Vec<StreamChunk> {
        let mut parser = StreamParser::new(StreamFormat::Responses);
        let mut out = Vec::new();
        for c in chunks {
            out.extend(parser.feed(&format!("event: {c}\ndata: {c}\n\n")));
        }
        out
    }

    #[test]
    fn responses_text_deltas() {
        let out = responses_stream(&[
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"Hel"}"#,
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"lo"}"#,
        ]);
        assert!(matches!(out.as_slice(), [
            StreamChunk::TextDelta { text: a },
            StreamChunk::TextDelta { text: b },
        ] if a == "Hel" && b == "lo"));
    }

    #[test]
    fn responses_reasoning_deltas() {
        let out = responses_stream(&[
            r#"{"type":"response.reasoning_summary_text.delta","item_id":"r_1","summary_index":0,"delta":"think"}"#,
        ]);
        assert!(
            matches!(out.as_slice(), [StreamChunk::ReasoningDelta { text }] if text == "think")
        );
    }

    #[test]
    fn responses_tool_call_lifecycle() {
        let mut parser = StreamParser::new(StreamFormat::Responses);
        let mut out = Vec::new();
        out.extend(parser.feed(&format!(
            "data: {}\n\n",
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"grep","arguments":""}}"#
        )));
        out.extend(parser.feed(&format!(
            "data: {}\n\n",
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"pattern\":"}"#
        )));
        out.extend(parser.feed(&format!(
            "data: {}\n\n",
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"\"fn\"}"}"#
        )));
        out.extend(parser.feed(&format!(
            "data: {}\n\n",
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"grep"}}"#
        )));
        out.extend(parser.feed(&format!(
            "data: {}\n\n",
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":5,"input_tokens_details":{"cached_tokens":4},"output_tokens_details":{"reasoning_tokens":2}}}}"#
        )));

        let has_start = out
            .iter()
            .any(|c| matches!(c, StreamChunk::ToolCallStart { index: 0, id, name } if id == "call_1" && name == "grep"));
        let has_deltas = out
            .iter()
            .filter(|c| matches!(c, StreamChunk::ToolCallDelta { index: 0, .. }))
            .count();
        let has_end = out
            .iter()
            .any(|c| matches!(c, StreamChunk::ToolCallEnd { index: 0 }));
        let has_finish = out
            .iter()
            .any(|c| matches!(c, StreamChunk::Finish { reason } if reason == "stop"));
        let has_usage = out
            .iter()
            .any(|c| matches!(c, StreamChunk::Usage { usage } if usage.prompt_tokens == 10 && usage.cached_read_tokens == Some(4) && usage.reasoning_tokens == Some(2)));
        assert!(has_start, "tool call start emitted");
        assert_eq!(has_deltas, 2, "argument deltas emitted");
        assert!(has_end, "tool call end emitted");
        assert!(has_finish, "finish emitted");
        assert!(has_usage, "usage emitted");
    }

    #[test]
    fn responses_failed_emits_error() {
        let out = responses_stream(&[r#"{"type":"response.failed","message":"model crashed"}"#]);
        assert!(matches!(out.as_slice(), [StreamChunk::Error { .. }]));
    }

    #[test]
    fn responses_incomplete_finish_is_length() {
        let out = responses_stream(&[
            r#"{"type":"response.completed","response":{"status":"incomplete","usage":{}}}"#,
        ]);
        assert!(out
            .iter()
            .any(|c| matches!(c, StreamChunk::Finish { reason } if reason == "length")));
    }

    #[test]
    fn anthropic_max_tokens_finish_normalizes_to_length() {
        // Anthropic's truncation stop_reason is "max_tokens"; the agent
        // loop's truncation-recovery path matches "length". Without the
        // normalization a cut-off answer was silently accepted as complete
        // (#88 audit H4).
        let mut parser = StreamParser::new(StreamFormat::Anthropic);
        let out = parser.feed(
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
        );
        assert!(
            out.iter()
                .any(|c| matches!(c, StreamChunk::Finish { reason } if reason == "length")),
            "max_tokens must normalize to length: {out:?}"
        );
    }

    #[test]
    fn anthropic_stream_usage_maps_cache_read_to_hit_and_creation_to_miss() {
        let mut parser = StreamParser::new(StreamFormat::Anthropic);
        let mut out = parser.feed(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100,\"cache_creation_input_tokens\":60,\"cache_read_input_tokens\":40}}}\n\n",
        );
        out.extend(parser.feed(
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":50,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":5}}\n\n",
        ));

        let usages: Vec<TokenUsage> = out
            .iter()
            .filter_map(|c| match c {
                StreamChunk::Usage { usage } => Some(usage.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            usages.len(),
            2,
            "message_start + message_delta usage chunks"
        );

        assert_eq!(usages[0].prompt_tokens, 100);
        assert_eq!(usages[0].prompt_cache_hit_tokens, Some(40));
        assert_eq!(usages[0].prompt_cache_miss_tokens, Some(60));
        assert_eq!(usages[1].completion_tokens, 50);
        assert_eq!(usages[1].prompt_cache_hit_tokens, Some(5));
        assert_eq!(usages[1].prompt_cache_miss_tokens, Some(10));

        // The agent loop merges the chunks via TokenUsage::add — totals land.
        let mut merged = TokenUsage::default();
        for u in &usages {
            merged.add(u);
        }
        assert_eq!(merged.prompt_tokens, 100);
        assert_eq!(merged.completion_tokens, 50);
        assert_eq!(merged.prompt_cache_hit_tokens, Some(45));
        assert_eq!(merged.prompt_cache_miss_tokens, Some(70));
    }

    #[test]
    fn parse_usage_object_covers_openai_and_deepseek_fields() {
        use serde_json::json;
        // The shared non-streaming usage parser (#88 audit H7) must read
        // prompt/completion tokens, the cached/reasoning details, and the
        // deepseek-native KV cache hit/miss fields.
        let usage = parse_usage_object(&json!({
            "prompt_tokens": 1000,
            "completion_tokens": 500,
            "prompt_tokens_details": { "cached_tokens": 700 },
            "completion_tokens_details": { "reasoning_tokens": 200 },
            "prompt_cache_hit_tokens": 600,
            "prompt_cache_miss_tokens": 400,
        }));
        assert_eq!(usage.prompt_tokens, 1000);
        assert_eq!(usage.completion_tokens, 500);
        assert_eq!(usage.cached_read_tokens, Some(700));
        assert_eq!(usage.reasoning_tokens, Some(200));
        assert_eq!(usage.prompt_cache_hit_tokens, Some(600));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(400));

        // Missing fields default to zero / None — never panic on partial
        // usage objects from providers that omit fields.
        let empty = parse_usage_object(&json!({}));
        assert_eq!(empty.prompt_tokens, 0);
        assert_eq!(empty.cached_read_tokens, None);
        assert_eq!(empty.prompt_cache_hit_tokens, None);
    }

    #[test]
    fn crlf_stream_parses_events_like_lf() {
        // A CRLF provider (`\r\n` line endings) previously never split:
        // `\r\n\r\n` contains no `\n\n`, so the buffer grew to the 16MB cap
        // and the whole stream failed. Both endings must parse identically.
        let mut lf = StreamParser::new(StreamFormat::OpenAi);
        let mut crlf = StreamParser::new(StreamFormat::OpenAi);
        let lf_data = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\n";
        let crlf_data = lf_data.replace("\n", "\r\n");
        let lf_chunks = lf.feed(lf_data);
        let crlf_chunks = crlf.feed(&crlf_data);
        assert_eq!(
            lf_chunks.len(),
            crlf_chunks.len(),
            "CRLF stream must produce the same chunk count as LF"
        );
        let text = |chunks: &[StreamChunk]| -> String {
            chunks
                .iter()
                .filter_map(|c| match c {
                    StreamChunk::TextDelta { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(text(&lf_chunks), text(&crlf_chunks));
        assert_eq!(text(&crlf_chunks), "hi there");
    }

    #[test]
    fn mixed_lf_crlf_terminators_split_evenly() {
        // A stream mixing both terminator styles must not deadlock on the
        // first event's boundary choice.
        let mut parser = StreamParser::new(StreamFormat::OpenAi);
        let data = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\r\n\r\n";
        let chunks = parser.feed(data);
        assert_eq!(chunks.len(), 2);
        let text: String = chunks
            .iter()
            .filter_map(|c| match c {
                StreamChunk::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "ab");
    }
}
