//! Stream events emitted during a conversation turn.
//!
//! Events are broadcast via Tauri events (`"chat-stream"`) to the frontend.
//! All delta events share a common `turn_id` so the frontend can group
//! them into a single turn view (reasoning + text + tool timeline).
//!
//! Every event travels inside a [`StreamEnvelope`](crate::core::stream) that
//! adds a monotonic `seq` — the frontend detects lost deltas by watching the
//! per-turn sequence and pulls a [`TurnSnapshot`] when a gap appears.
//!
//! ## Event lifecycle per turn
//!
//! ```text
//! TurnStart
//!   ├── ReasoningDelta...  (thinking mode, provider-dependent)
//!   ├── TextDelta...       (response text)
//!   ├── ToolCallStart
//!   │   ├── ToolCallDelta...  (arguments streaming)
//!   │   └── ToolCallProgress... (tool output, e.g. bash)
//!   └── ToolCallResult    (tool output)
//!   ├── ReasoningDelta...  (next LLM call in same turn)
//!   ├── TextDelta...
//!   ├── ToolCallStart...   (next tool call)
//!   ...
//!   ├── TurnStatus        (optional phase signals, e.g. verifying)
//!   ├── Usage
//!   ├── TurnEnd | Error   (the ONLY terminal events)
//!   └── Snapshot          (authoritative terminal state, always last)
//! ```

use serde::{Deserialize, Serialize};

/// Turn terminal outcome — the semantic result of a conversation turn.
///
/// `done` is a TERMINAL state: the work was verified/accepted and the turn
/// must not self-drive another round. Everything else is an explicit
/// non-normal outcome the frontend can surface (limit / cancelled / denied /
/// failed) or a hand-back to the user (needs_input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// Work finished and accepted (normal end — the terminal state).
    Done,
    /// The turn ended waiting on user input (ask_user / elicitation).
    NeedsInput,
    /// The turn failed with an error.
    Failed,
    /// Output/budget limit reached — a forced final summary was emitted.
    Limit,
    /// Cancelled by the user.
    Cancelled,
    /// Terminated after repeated permission denials.
    Denied,
}

/// Live phase of a running turn — a non-terminal signal the frontend can
/// surface ("verifying", later "compacting", …). Terminal signals are
/// exclusively `TurnEnd` / `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    /// A stop-path gate held the turn (verification pending, evaluator
    /// review failed, todo/plan/background discipline, stop hook) — the
    /// already-streamed summary is NOT final.
    Verifying,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_outcome_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&TurnOutcome::NeedsInput).unwrap(),
            "\"needs_input\""
        );
        assert_eq!(
            serde_json::to_string(&TurnOutcome::Done).unwrap(),
            "\"done\""
        );
    }

    #[test]
    fn turn_phase_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&TurnPhase::Verifying).unwrap(),
            "\"verifying\""
        );
    }
}

/// One tool call's authoritative terminal state, captured in a turn
/// snapshot for gap recovery (arguments are the FINAL accumulated string,
/// `result` is present once the call completed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallSnapshot {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub is_error: bool,
}

/// An interactive MCP app attached to a tool result (`ui://` resource),
/// captured in a turn snapshot so a lost `mcp_app` event can be repaired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAppSnapshot {
    pub call_id: String,
    pub name: String,
    pub server: String,
    pub resource_uri: String,
    pub html: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp: Option<serde_json::Value>,
}

/// Authoritative terminal state of one turn — emitted as the `snapshot`
/// event right after `turn_end`/`error` and served by the
/// `get_turn_snapshot` command. The frontend converges on this after a
/// detected seq gap instead of trusting the (lossy) live delta stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSnapshot {
    pub turn_id: String,
    pub session_id: String,
    pub status: TurnOutcome,
    pub reason: String,
    /// Final assistant text (capped by the emitter).
    pub text: String,
    /// Final reasoning text (capped by the emitter; empty when the provider
    /// produced none or it was truncated).
    pub reasoning: String,
    pub tool_calls: Vec<ToolCallSnapshot>,
    pub mcp_apps: Vec<McpAppSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<super::info::TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Turn starts — emitted once per conversation turn. Carries the
    /// session_id so frontend listeners can reject events from OTHER
    /// sessions (background subagents run their own turns on the same
    /// channel — without the session filter their turn_start would hijack
    /// the parent's stream and their events would bleed into it).
    TurnStart {
        turn_id: String,
        session_id: String,
        model: String,
        /// Full-run trace id — identical across every protocol that
        /// re-broadcasts this event (chat-stream / ACP / SSE).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    /// Text content delta from the LLM stream.
    TextDelta { turn_id: String, text: String },
    /// Reasoning/thinking content delta (DeepSeek, Anthropic, etc.).
    ReasoningDelta { turn_id: String, text: String },
    /// A tool call begins — name and ID are known.
    ToolCallStart {
        turn_id: String,
        call_id: String,
        name: String,
    },
    /// Tool call arguments streaming incrementally.
    ToolCallDelta {
        turn_id: String,
        call_id: String,
        arguments: String,
    },
    /// Tool call completed with its result (or error).
    ToolCallResult {
        turn_id: String,
        call_id: String,
        name: String,
        result: String,
        is_error: bool,
    },
    /// Tool progress — streamed tool output (e.g. bash stdout). The wire
    /// carries exactly one content channel (`delta`) plus an optional byte
    /// counter; `kind` discriminates the semantic (partial_result vs custom
    /// progress). Rich variants (text/blocks/subkind/payload) were removed
    /// from the protocol — nothing emitted them and the frontend renders
    /// everything from `delta`.
    ToolCallProgress {
        turn_id: String,
        call_id: String,
        name: String,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_bytes: Option<u64>,
    },
    /// Token usage report for the turn.
    Usage {
        turn_id: String,
        usage: super::info::TokenUsage,
    },
    /// Turn completed — `status` is the semantic terminal outcome (see
    /// [`TurnOutcome`]); `reason` keeps the legacy stop/length/cancelled
    /// signal for compatibility.
    TurnEnd {
        turn_id: String,
        session_id: String,
        reason: String,
        status: TurnOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    /// Live phase signal for a running turn. Currently only
    /// `phase: "verifying"` is emitted (a stop-path gate forced another
    /// model turn — verification pending, evaluator review failed,
    /// todo/plan/background discipline, stop hook); the frontend shows a
    /// "verifying/continuing" phase instead of treating the already-streamed
    /// summary as final. Terminal signals are exclusively `turn_end`/`error`.
    TurnStatus {
        turn_id: String,
        session_id: String,
        phase: TurnPhase,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    /// Authoritative terminal state of the turn — always emitted last
    /// (right after `turn_end`/`error`). The frontend converges on it after
    /// a detected seq gap; it is a REPAIR payload, never a live delta.
    Snapshot { snapshot: TurnSnapshot },
    /// Turn failed with an error.
    Error {
        turn_id: String,
        session_id: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    /// Conversation compaction notification.
    Compaction {
        session_id: String,
        compacted_tokens: u64,
        summary: String,
    },
    /// Memory was auto-injected into this turn's context (relevance search).
    /// Emitted before the first LLM call of the turn; carries a snippet so
    /// the UI can show a non-intrusive "memory referenced" marker.
    MemoryInjected {
        session_id: String,
        count: u32,
        snippet: String,
    },
    /// Background subagent started. `tool_call_id` links it to the `agent`
    /// tool call that spawned it ("" when unknown, e.g. decompose workers);
    /// `session_id` is the parent session that owns the worker (None for
    /// harness-internal spawns) — frontend stores route on it.
    SubagentStart {
        subagent_id: String,
        task: String,
        agent_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// Background subagent progress.
    SubagentProgress {
        subagent_id: String,
        message: String,
        turn: u32,
        total_turns: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// Background subagent completed.
    SubagentResult {
        subagent_id: String,
        result: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    /// MCP server elicitation request — asks the user for input.
    Elicitation {
        elicitation_id: String,
        server_name: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested_schema: Option<serde_json::Value>,
    },
    /// MCP Apps (2026-07-28 extension) — an interactive HTML UI attached to
    /// a tool result via a `ui://` resource. Emitted right after the linked
    /// `ToolCallResult`; the frontend renders it in a sandboxed iframe.
    McpApp {
        turn_id: String,
        call_id: String,
        name: String,
        server: String,
        resource_uri: String,
        html: String,
        is_error: bool,
        /// CSP domains declared by the server (`_meta.ui.csp`) — injected
        /// into the sandboxed document by the host.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        csp: Option<serde_json::Value>,
    },
}
