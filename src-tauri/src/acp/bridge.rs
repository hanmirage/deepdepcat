//! ACP evidence bridge — forwards tool-level stream events and collects
//! session evidence for bench/audit archives.
//!
//! The ACP SSE channel used to carry only text deltas; external clients
//! (bench, IDEs, other agents) could not see WHICH tools ran or what they
//! returned. This module extends the bridge to tool_call_start/delta/
//! result + usage, and exposes `session/evidence` so a client can archive
//! messages + agent_events BEFORE closing the session (bench scoring needs
//! independent, replayable evidence).

use super::AcpState;
use crate::core::types::{ConversationItem, StreamEvent};
use serde::Serialize;

/// Session evidence bundle returned by the `session/evidence` RPC.
#[derive(Debug, Clone, Serialize)]
pub struct SessionEvidence {
    pub session: Option<crate::core::types::Session>,
    pub messages: Vec<ConversationItem>,
    /// Agent event log in replay order (oldest first).
    pub events: Vec<crate::storage::database::AgentEvent>,
}

impl AcpState {
    /// Route a `chat-stream` event into the SSE bus (session-scoped).
    pub async fn forward_stream_event(&self, event: StreamEvent) {
        use StreamEvent::*;
        match &event {
            TurnStart { session_id, .. } => {
                let mut turns = self.active_turns.lock().await;
                if let StreamEvent::TurnStart { turn_id, .. } = &event {
                    turns.insert(turn_id.clone(), session_id.clone());
                }
                // Signal the ACP client that a new agent turn began.
                self.bus.emit(
                    "prompt/streaming_update",
                    serde_json::json!({
                        "sessionId": session_id,
                        "kind": "turn_start",
                    }),
                );
            }
            TextDelta { turn_id, text } => {
                if let Some(session_id) = self.session_for_turn(turn_id).await {
                    self.bus.emit(
                        "prompt/streaming_update",
                        serde_json::json!({
                            "sessionId": session_id,
                            "text": text,
                        }),
                    );
                }
            }
            ToolCallStart {
                turn_id,
                call_id,
                name,
            } => {
                if let Some(session_id) = self.session_for_turn(turn_id).await {
                    self.bus.emit(
                        "prompt/streaming_update",
                        serde_json::json!({
                            "sessionId": session_id,
                            "kind": "tool_call_start",
                            "turnId": turn_id,
                            "callId": call_id,
                            "name": name,
                        }),
                    );
                }
            }
            ToolCallDelta {
                turn_id,
                call_id,
                arguments,
            } => {
                if let Some(session_id) = self.session_for_turn(turn_id).await {
                    self.bus.emit(
                        "prompt/streaming_update",
                        serde_json::json!({
                            "sessionId": session_id,
                            "kind": "tool_call_delta",
                            "turnId": turn_id,
                            "callId": call_id,
                            "arguments": arguments,
                        }),
                    );
                }
            }
            ToolCallResult {
                turn_id,
                call_id,
                name,
                result,
                is_error,
            } => {
                if let Some(session_id) = self.session_for_turn(turn_id).await {
                    self.bus.emit(
                        "prompt/streaming_update",
                        serde_json::json!({
                            "sessionId": session_id,
                            "kind": "tool_call_result",
                            "turnId": turn_id,
                            "callId": call_id,
                            "name": name,
                            "result": result,
                            "isError": is_error,
                        }),
                    );
                }
            }
            Usage { turn_id, usage } => {
                if let Some(session_id) = self.session_for_turn(turn_id).await {
                    self.bus.emit(
                        "prompt/streaming_update",
                        serde_json::json!({
                            "sessionId": session_id,
                            "kind": "usage",
                            "turnId": turn_id,
                            "usage": usage,
                        }),
                    );
                }
            }
            TurnEnd { session_id, .. } | Error { session_id, .. } => {
                let mut turns = self.active_turns.lock().await;
                let mut remove = Vec::new();
                for (tid, sid) in turns.iter() {
                    if sid == session_id {
                        remove.push(tid.clone());
                    }
                }
                for tid in remove {
                    turns.remove(&tid);
                }
            }
            _ => {}
        }
    }

    /// Resolve the session id owning a turn (from the TurnStart map).
    async fn session_for_turn(&self, turn_id: &str) -> Option<String> {
        self.active_turns.lock().await.get(turn_id).cloned()
    }

    /// Collect a session's messages + agent events for archiving. Call
    /// BEFORE `session/close` — closing deletes the session cascade.
    pub async fn collect_session_evidence(
        &self,
        session_id: &str,
    ) -> Result<SessionEvidence, String> {
        let db = &self.state.db;
        let session = db.get_session(session_id).map_err(|e| e.to_string())?;
        let messages = db.load_messages(session_id).map_err(|e| e.to_string())?;
        let mut events =
            crate::storage::database::list_events(db, session_id, 100_000)
                .map_err(|e| e.to_string())?;
        // The event log is stored newest-first; evidence should replay in
        // execution order.
        events.reverse();
        Ok(SessionEvidence {
            session,
            messages,
            events,
        })
    }
}
