//! Chat send-path types — the structured send result envelope and the
//! execution-mode parser. Extracted from `chat.rs` to keep the send path
//! within the file-size budget.

use crate::agent::agent_loop::AgentLoopMode;

/// Structured `send_chat_message` result — replaces the magic-string
/// protocol ("queued:..."/"cancelled"/bare turn id) with an explicit
/// discriminated envelope:
/// - `kind = "accepted"` — the turn ran to completion (`turn_id` = the
///   last turn id; the turn's events were already streamed on chat-stream).
/// - `kind = "queued"` — the session was busy; the prompt was queued and
///   will be replayed when the running turn ends (`prompt_id` identifies
///   the queue entry; the frontend keeps its listener alive for the replay).
/// - `kind = "cancelled"` — the invoke was aborted by user cancel before
///   the loop started (or the loop was cancelled).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SendChatResult {
    pub kind: String,
    pub prompt_id: Option<String>,
    pub turn_id: Option<String>,
}

impl SendChatResult {
    pub fn accepted(turn_id: String) -> Self {
        Self {
            kind: "accepted".into(),
            prompt_id: None,
            turn_id: Some(turn_id),
        }
    }

    pub fn queued(prompt_id: String) -> Self {
        Self {
            kind: "queued".into(),
            prompt_id: Some(prompt_id),
            turn_id: None,
        }
    }

    pub fn cancelled() -> Self {
        Self {
            kind: "cancelled".into(),
            prompt_id: None,
            turn_id: None,
        }
    }
}

/// Parse the frontend execution-mode string into an [`AgentLoopMode`].
/// Unknown or absent values fall back to Standard so older clients stay
/// compatible.
pub fn parse_agent_mode(mode: Option<&str>) -> AgentLoopMode {
    match mode {
        Some("plan_execute") => AgentLoopMode::PlanExecute,
        Some("reflexion") => AgentLoopMode::Reflexion,
        Some("coordinator") => AgentLoopMode::Coordinator,
        Some("evaluator_qa") => AgentLoopMode::EvaluatorQa,
        Some("goal") => AgentLoopMode::Goal,
        _ => AgentLoopMode::Standard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_mode_maps_all_known_modes() {
        assert_eq!(parse_agent_mode(None), AgentLoopMode::Standard);
        assert_eq!(parse_agent_mode(Some("unknown")), AgentLoopMode::Standard);
        assert_eq!(
            parse_agent_mode(Some("plan_execute")),
            AgentLoopMode::PlanExecute
        );
        assert_eq!(
            parse_agent_mode(Some("reflexion")),
            AgentLoopMode::Reflexion
        );
        assert_eq!(
            parse_agent_mode(Some("coordinator")),
            AgentLoopMode::Coordinator
        );
        assert_eq!(
            parse_agent_mode(Some("evaluator_qa")),
            AgentLoopMode::EvaluatorQa
        );
        assert_eq!(parse_agent_mode(Some("goal")), AgentLoopMode::Goal);
    }
}
