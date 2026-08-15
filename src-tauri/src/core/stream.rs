//! Central `chat-stream` emitter.
//!
//! Every agent-stream event must be emitted through [`emit_stream`] — the
//! single funnel that:
//!
//! 1. Wraps the typed event in a [`StreamEnvelope`] carrying a monotonic
//!    `seq` (global across sessions; monotonic implies per-turn monotonic,
//!    so the frontend can detect lost deltas per turn).
//! 2. Accumulates a bounded authoritative [`TurnSnapshot`] per turn while
//!    deltas flow, then emits it as the `snapshot` event immediately after
//!    the terminal `turn_end`/`error`.
//!
//! The snapshot store also backs the `get_turn_snapshot` command so a
//! frontend that detected a seq gap can pull the terminal state on demand.

use crate::core::types::{
    McpAppSnapshot, StreamEvent, TokenUsage, ToolCallSnapshot, TurnOutcome, TurnSnapshot,
};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

/// Per-turn snapshot size caps — the snapshot is a REPAIR payload, not a
/// full archive; oversized content is truncated while it streams.
const MAX_TEXT_CHARS: usize = 200_000;
const MAX_TOOL_RESULT_CHARS: usize = 100_000;
const MAX_TOOL_ARGS_CHARS: usize = 50_000;
const MAX_MCP_APP_CHARS: usize = 200_000;
/// Bounded snapshot retention (FIFO) — gap repair only needs the recent
/// past; an unbounded map would grow with every turn ever run.
const MAX_SNAPSHOTS: usize = 64;

/// The wire envelope: `seq` rides flattened alongside the typed event.
/// Deserializers of the inner `StreamEvent` (ACP bridge) ignore the extra
/// field, so the external ACP contract is unchanged.
#[derive(Debug, Clone, Serialize)]
struct StreamEnvelope {
    seq: u64,
    #[serde(flatten)]
    event: StreamEvent,
}

/// Monotonic sequence across all emits (any session, any turn). Per-turn
/// monotonicity is implied: events of one turn are emitted in order.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// In-flight per-turn accumulators (keyed by turn id — unique app-wide).
static ACTIVE: OnceLock<Mutex<HashMap<String, TurnAccumulator>>> = OnceLock::new();

/// Sealed terminal snapshots, bounded FIFO.
static STORE: OnceLock<Mutex<SnapshotStore>> = OnceLock::new();

#[derive(Default)]
struct TurnAccumulator {
    text: String,
    reasoning: String,
    tool_calls: HashMap<String, ToolCallSnapshot>,
    tool_order: Vec<String>,
    mcp_apps: Vec<McpAppSnapshot>,
    usage: Option<TokenUsage>,
}

struct SnapshotStore {
    map: HashMap<(String, String), Arc<TurnSnapshot>>,
    order: VecDeque<(String, String)>,
}

impl SnapshotStore {
    fn insert(&mut self, key: (String, String), snapshot: Arc<TurnSnapshot>) {
        if self.map.insert(key.clone(), snapshot).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > MAX_SNAPSHOTS {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
    }

    fn get(&self, session_id: &str, turn_id: &str) -> Option<TurnSnapshot> {
        self.map
            .get(&(session_id.to_string(), turn_id.to_string()))
            .map(|s| (**s).clone())
    }
}

fn active() -> &'static Mutex<HashMap<String, TurnAccumulator>> {
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn store() -> &'static Mutex<SnapshotStore> {
    STORE.get_or_init(|| {
        Mutex::new(SnapshotStore {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// Append `delta` to `dst` while keeping the total under `cap` chars.
fn push_capped(dst: &mut String, delta: &str, cap: usize) {
    let room = cap.saturating_sub(dst.len());
    if room == 0 {
        return;
    }
    let take: String = delta.chars().take(room).collect();
    dst.push_str(&take);
}

/// Fold one emitted event into its turn's snapshot accumulator.
fn observe(event: &StreamEvent) {
    match event {
        StreamEvent::TurnStart { turn_id, .. } => {
            active()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(turn_id.clone(), TurnAccumulator::default());
        }
        StreamEvent::TextDelta { turn_id, text } => {
            if let Some(acc) = active()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(turn_id)
            {
                push_capped(&mut acc.text, text, MAX_TEXT_CHARS);
            }
        }
        StreamEvent::ReasoningDelta { turn_id, text } => {
            if let Some(acc) = active()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(turn_id)
            {
                push_capped(&mut acc.reasoning, text, MAX_TEXT_CHARS);
            }
        }
        StreamEvent::ToolCallStart {
            turn_id,
            call_id,
            name,
        } => {
            let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
            let Some(acc) = guard.get_mut(turn_id) else {
                return;
            };
            if !acc.tool_calls.contains_key(call_id) {
                acc.tool_order.push(call_id.clone());
                acc.tool_calls.insert(
                    call_id.clone(),
                    ToolCallSnapshot {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                        result: None,
                        is_error: false,
                    },
                );
            }
        }
        StreamEvent::ToolCallDelta {
            turn_id,
            call_id,
            arguments,
        } => {
            let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tool) = guard
                .get_mut(turn_id)
                .and_then(|acc| acc.tool_calls.get_mut(call_id))
            {
                push_capped(&mut tool.arguments, arguments, MAX_TOOL_ARGS_CHARS);
            }
        }
        StreamEvent::ToolCallResult {
            turn_id,
            call_id,
            name,
            result,
            is_error,
        } => {
            let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tool) = guard
                .get_mut(turn_id)
                .and_then(|acc| acc.tool_calls.get_mut(call_id))
            {
                let mut capped = String::new();
                push_capped(&mut capped, result, MAX_TOOL_RESULT_CHARS);
                tool.name = name.clone();
                tool.result = Some(capped);
                tool.is_error = *is_error;
            }
        }
        StreamEvent::McpApp {
            turn_id,
            call_id,
            name,
            server,
            resource_uri,
            html,
            is_error,
            csp,
        } => {
            let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
            let Some(acc) = guard.get_mut(turn_id) else {
                return;
            };
            let mut capped_html = String::new();
            push_capped(&mut capped_html, html, MAX_MCP_APP_CHARS);
            acc.mcp_apps.push(McpAppSnapshot {
                call_id: call_id.clone(),
                name: name.clone(),
                server: server.clone(),
                resource_uri: resource_uri.clone(),
                html: capped_html,
                is_error: *is_error,
                csp: csp.clone(),
            });
        }
        StreamEvent::Usage { turn_id, usage } => {
            if let Some(acc) = active()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(turn_id)
            {
                // Usage is emitted once PER LLM CALL; a tool-using turn makes
                // several calls. Accumulate so the sealed snapshot carries the
                // turn total (the frontend live path sums the same way), not
                // just the last call's tokens.
                match &mut acc.usage {
                    Some(total) => total.add(usage),
                    None => acc.usage = Some(usage.clone()),
                }
            }
        }
        _ => {}
    }
}

/// Seal the turn's accumulator into a stored snapshot and emit it as the
/// final `snapshot` event. No-op when the turn has no accumulator (already
/// sealed, or a session-level error with an empty turn id).
fn seal_and_emit(app: &AppHandle, event: &StreamEvent) {
    let (turn_id, session_id, status, reason, trace_id) = match event {
        StreamEvent::TurnEnd {
            turn_id,
            session_id,
            status,
            reason,
            trace_id,
        } => (turn_id, session_id, *status, reason, trace_id.clone()),
        StreamEvent::Error {
            turn_id,
            session_id,
            message,
            trace_id,
        } if !turn_id.is_empty() => (
            turn_id,
            session_id,
            TurnOutcome::Failed,
            message,
            trace_id.clone(),
        ),
        _ => return,
    };

    let snapshot = {
        let Some(acc) = active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(turn_id)
        else {
            return;
        };
        let TurnAccumulator {
            text,
            reasoning,
            tool_calls,
            tool_order,
            mcp_apps,
            usage,
        } = acc;
        let calls: Vec<ToolCallSnapshot> = tool_order
            .iter()
            .filter_map(|id| tool_calls.get(id).cloned())
            .collect();
        TurnSnapshot {
            turn_id: turn_id.clone(),
            session_id: session_id.clone(),
            status,
            reason: reason.clone(),
            text,
            reasoning,
            tool_calls: calls,
            mcp_apps,
            usage,
            trace_id,
        }
    };

    store().lock().unwrap_or_else(|e| e.into_inner()).insert(
        (session_id.clone(), turn_id.clone()),
        Arc::new(snapshot.clone()),
    );
    emit_impl(app, StreamEvent::Snapshot { snapshot });
}

fn emit_impl(app: &AppHandle, event: StreamEvent) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let _ = app.emit("chat-stream", StreamEnvelope { seq, event });
}

/// Emit one `chat-stream` event with its sequence number, folding it into
/// the turn snapshot accumulator and sealing/emitting the snapshot when the
/// event is terminal.
pub fn emit_stream(app: &AppHandle, event: StreamEvent) {
    observe(&event);
    emit_impl(app, event.clone());
    if matches!(
        &event,
        StreamEvent::TurnEnd { .. } | StreamEvent::Error { .. }
    ) {
        seal_and_emit(app, &event);
    }
}

/// Pull a sealed turn snapshot (used by the `get_turn_snapshot` command).
pub fn get_turn_snapshot(session_id: &str, turn_id: &str) -> Option<TurnSnapshot> {
    store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id, turn_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrips_with_seq() {
        let envelope = StreamEnvelope {
            seq: 7,
            event: StreamEvent::TextDelta {
                turn_id: "t1".into(),
                text: "hi".into(),
            },
        };
        let json = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(json["seq"], 7);
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hi");
        // The inner StreamEvent still deserializes from the envelope JSON
        // (unknown `seq` ignored) — ACP bridge compatibility.
        let inner: StreamEvent = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(inner, StreamEvent::TextDelta { text, .. } if text == "hi"));
    }

    #[test]
    fn accumulator_builds_terminal_snapshot() {
        let turn = "t-snap";
        let session = "s1";
        let mut acc = TurnAccumulator::default();
        acc.text.push_str("answer");
        acc.reasoning.push_str("think");
        acc.tool_calls.insert(
            "c1".into(),
            ToolCallSnapshot {
                call_id: "c1".into(),
                name: "grep".into(),
                arguments: "\"q\"".into(),
                result: Some("hit".into()),
                is_error: false,
            },
        );
        acc.tool_order.push("c1".into());
        let snapshot = TurnSnapshot {
            turn_id: turn.into(),
            session_id: session.into(),
            status: TurnOutcome::Done,
            reason: "stop".into(),
            text: acc.text,
            reasoning: acc.reasoning,
            tool_calls: acc
                .tool_order
                .iter()
                .filter_map(|id| acc.tool_calls.get(id).cloned())
                .collect(),
            mcp_apps: acc.mcp_apps,
            usage: acc.usage,
            trace_id: None,
        };
        assert_eq!(snapshot.text, "answer");
        assert_eq!(snapshot.reasoning, "think");
        assert_eq!(snapshot.tool_calls.len(), 1);
        assert_eq!(snapshot.tool_calls[0].result.as_deref(), Some("hit"));
    }

    #[test]
    fn store_evicts_oldest() {
        let mut store = SnapshotStore {
            map: HashMap::new(),
            order: VecDeque::new(),
        };
        for i in 0..(MAX_SNAPSHOTS + 8) {
            let key = (format!("s{i}"), format!("t{i}"));
            store.insert(
                key,
                Arc::new(TurnSnapshot {
                    turn_id: format!("t{i}"),
                    session_id: format!("s{i}"),
                    status: TurnOutcome::Done,
                    reason: "stop".into(),
                    text: String::new(),
                    reasoning: String::new(),
                    tool_calls: Vec::new(),
                    mcp_apps: Vec::new(),
                    usage: None,
                    trace_id: None,
                }),
            );
        }
        assert_eq!(store.map.len(), MAX_SNAPSHOTS);
        assert!(store.get("s0", "t0").is_none());
        assert!(store.get("s1", "t1").is_none());
        assert!(store.get("s8", "t8").is_some());
    }

    #[test]
    fn push_capped_limits_chars() {
        let mut dst = String::new();
        push_capped(&mut dst, "hello 世界", 8);
        assert_eq!(dst.chars().count(), 8);
        assert!(dst.starts_with("hello "));
    }

    #[test]
    fn terminal_turn_seals_snapshot() {
        let turn = "t-seal";
        let session = "s-seal";
        observe(&StreamEvent::TurnStart {
            turn_id: turn.into(),
            session_id: session.into(),
            model: "m".into(),
            trace_id: None,
        });
        observe(&StreamEvent::TextDelta {
            turn_id: turn.into(),
            text: "final".into(),
        });
        let sealed = {
            let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
            let acc = guard.remove(turn).expect("accumulator present");
            TurnSnapshot {
                turn_id: turn.into(),
                session_id: session.into(),
                status: TurnOutcome::Done,
                reason: "stop".into(),
                text: acc.text,
                reasoning: acc.reasoning,
                tool_calls: Vec::new(),
                mcp_apps: acc.mcp_apps,
                usage: acc.usage,
                trace_id: None,
            }
        };
        store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((session.into(), turn.into()), Arc::new(sealed.clone()));
        assert_eq!(
            get_turn_snapshot(session, turn)
                .expect("snapshot stored")
                .text,
            "final"
        );
    }

    #[test]
    fn usage_accumulates_across_multiple_llm_calls() {
        let turn = "t-usage-acc";
        observe(&StreamEvent::TurnStart {
            turn_id: turn.into(),
            session_id: "s".into(),
            model: "m".into(),
            trace_id: None,
        });
        // A tool-using turn emits Usage once per LLM call; the accumulator
        // must SUM them, not keep only the last.
        observe(&StreamEvent::Usage {
            turn_id: turn.into(),
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            },
        });
        observe(&StreamEvent::Usage {
            turn_id: turn.into(),
            usage: TokenUsage {
                prompt_tokens: 20,
                completion_tokens: 7,
                ..Default::default()
            },
        });
        let usage = active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(turn)
            .expect("accumulator present")
            .usage
            .clone()
            .expect("usage set");
        assert_eq!(usage.prompt_tokens, 30);
        assert_eq!(usage.completion_tokens, 12);
        // Clean up the global accumulator so it can't leak into other tests.
        active().lock().unwrap_or_else(|e| e.into_inner()).remove(turn);
    }
}
