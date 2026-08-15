//! Running-turn registry — cross-session visibility for background agents.
//!
//! A main-agent turn keeps running even after the user switches to another
//! session (the loop lives inside `send_chat_message`, independent of any
//! frontend connection). This registry exposes those in-flight turns so the
//! frontend can show "background sessions", jump back to them, or stop them
//! via the existing `cancel_operation` command.

use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::Mutex;
use tauri::{AppHandle, Emitter};

use crate::bootstrap::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunningTurnStatus {
    Running,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunningTurnInfo {
    pub session_id: String,
    /// Trace id of the owning turn (one per send invocation, stable across
    /// the queued backlog replay).
    pub turn_id: String,
    pub started_at_ms: u64,
    /// First line / truncated user message — enough to recognize the task.
    pub message_preview: String,
    /// Product mode: "code" | "depwork".
    pub work_mode: String,
    pub status: RunningTurnStatus,
}

/// Broadcast when a registered turn finishes (completed / cancelled / error).
#[derive(Debug, Clone, Serialize)]
pub struct TurnCompletedPayload {
    pub session_id: String,
    pub turn_id: String,
    pub status: String,
}

/// In-flight main-agent turns, keyed by session id (one running turn per
/// session; queued backlog replays share the same entry).
#[derive(Default)]
pub struct RunningTurnRegistry {
    inner: Mutex<HashMap<String, RunningTurnInfo>>,
}

impl RunningTurnRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, info: RunningTurnInfo) {
        self.inner.lock().await.insert(info.session_id.clone(), info);
    }

    pub async fn set_paused(&self, session_id: &str, paused: bool) {
        let mut map = self.inner.lock().await;
        if let Some(info) = map.get_mut(session_id) {
            info.status = if paused {
                RunningTurnStatus::Paused
            } else {
                RunningTurnStatus::Running
            };
        }
    }

    /// Remove the entry and return it (callers emit the completion event).
    pub async fn unregister(&self, session_id: &str) -> Option<RunningTurnInfo> {
        self.inner.lock().await.remove(session_id)
    }

    /// All in-flight turns, oldest first.
    pub async fn list(&self) -> Vec<RunningTurnInfo> {
        let mut out: Vec<RunningTurnInfo> = self
            .inner
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        out.sort_by_key(|t| t.started_at_ms);
        out
    }
}

/// First line of a user message, capped — used to label the background turn.
pub fn turn_message_preview(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message);
    first.chars().take(80).collect()
}

/// Unregister the running turn and broadcast its completion (one place for
/// every exit path: success, tool error, user cancel, session lost).
pub async fn finish_running_turn(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    status: &str,
) {
    if let Some(info) = state.running_turns.unregister(session_id).await {
        let _ = app.emit(
            "agent-turn-completed",
            TurnCompletedPayload {
                session_id: info.session_id,
                turn_id: info.turn_id,
                status: status.to_string(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(session_id: &str) -> RunningTurnInfo {
        RunningTurnInfo {
            session_id: session_id.to_string(),
            turn_id: format!("trace-{session_id}"),
            started_at_ms: 1_000,
            message_preview: "refactor auth".to_string(),
            work_mode: "code".to_string(),
            status: RunningTurnStatus::Running,
        }
    }

    #[tokio::test]
    async fn register_list_unregister_roundtrip() {
        let reg = RunningTurnRegistry::new();
        reg.register(sample("s1")).await;
        reg.register(RunningTurnInfo {
            started_at_ms: 2_000,
            ..sample("s2")
        })
        .await;

        let list = reg.list().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session_id, "s1");
        assert_eq!(list[1].session_id, "s2");

        let removed = reg.unregister("s1").await.expect("entry exists");
        assert_eq!(removed.turn_id, "trace-s1");
        assert_eq!(reg.list().await.len(), 1);
        assert!(reg.unregister("s1").await.is_none());
    }

    #[tokio::test]
    async fn re_registration_replaces_same_session() {
        let reg = RunningTurnRegistry::new();
        reg.register(sample("s1")).await;
        let second = RunningTurnInfo {
            turn_id: "trace-s1-2".to_string(),
            ..sample("s1")
        };
        reg.register(second).await;

        let list = reg.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].turn_id, "trace-s1-2");
    }

    #[tokio::test]
    async fn pause_flips_status_only_for_existing_session() {
        let reg = RunningTurnRegistry::new();
        reg.register(sample("s1")).await;

        reg.set_paused("s1", true).await;
        assert_eq!(reg.list().await[0].status, RunningTurnStatus::Paused);
        reg.set_paused("missing", true).await;
        reg.set_paused("s1", false).await;
        assert_eq!(reg.list().await[0].status, RunningTurnStatus::Running);
    }
}
