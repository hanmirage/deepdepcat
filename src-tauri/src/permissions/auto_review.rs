//! Auto-Review — independent reviewer agent for gray-zone permission asks.
//!
//! Industry model (Codex Auto-review, 2026): a permission `Ask` that would
//! normally stop for a human is routed to a SEPARATE reviewer context which
//! decides allow/deny with a rationale. It is a reviewer swap, never a
//! permission grant — sandbox/rule layers already ran and still win.
//!
//! Circuit breaker (Codex official semantics): 3 consecutive denials or 10
//! denials in the last 50 reviews interrupt auto-review for the turn so the
//! agent cannot loop on escalation attempts. Hard deny layers (rules,
//! sensitive files, dangerous bash) never reach this module.

use crate::bootstrap::AppState;
use crate::core::types::ConversationItem;
use crate::llm::provider::{LlmProvider, LlmRequest};
use serde_json::Value;
use std::collections::VecDeque;
use tauri::{AppHandle, Emitter, Manager};

/// Consecutive denials that trip the breaker.
const CONSECUTIVE_DENIAL_LIMIT: usize = 3;
/// Denials inside the rolling window that trip the breaker.
const WINDOW_DENIAL_LIMIT: usize = 10;
/// Rolling window size (last N reviews).
const WINDOW_SIZE: usize = 50;
/// Reviewer transcript tail limit (chars).
const REVIEW_TRANSCRIPT_CHARS: usize = 4000;

/// A single auto-review verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoReviewVerdict {
    pub allow: bool,
    pub reason: String,
}

/// Per-session denial accounting for the circuit breaker.
#[derive(Debug, Default)]
pub struct AutoReviewTracker {
    consecutive_denials: usize,
    recent_denials: VecDeque<bool>,
}

impl AutoReviewTracker {
    /// Record a verdict outcome.
    pub fn record(&mut self, denied: bool) {
        if denied {
            self.consecutive_denials += 1;
        } else {
            self.consecutive_denials = 0;
        }
        self.recent_denials.push_back(denied);
        if self.recent_denials.len() > WINDOW_SIZE {
            self.recent_denials.pop_front();
        }
    }

    /// Whether the breaker is tripped for this session.
    pub fn tripped(&self) -> bool {
        self.consecutive_denials >= CONSECUTIVE_DENIAL_LIMIT
            || self
                .recent_denials
                .iter()
                .filter(|d| **d)
                .count()
                >= WINDOW_DENIAL_LIMIT
    }
}

/// Run the reviewer for one action. Returns `Err` only on infrastructure
/// failure (LLM unreachable) — policy outcomes are `Ok(verdict)`.
pub async fn review_action(
    app: &AppHandle,
    session_id: &str,
    tool_name: &str,
    args: &Value,
) -> Result<AutoReviewVerdict, String> {
    let state = app.state::<AppState>();
    let (model, provider, workspace) = {
        let config = state.config().map_err(|e| e.to_string())?;
        let model = if config.permissions.auto_review_model.trim().is_empty() {
            config.app.default_model.clone()
        } else {
            config.permissions.auto_review_model.clone()
        };
        let provider = config.app.default_provider.clone();
        let workspace = state.workspace.read().ok().and_then(|w| w.clone());
        (model, provider, workspace)
    };

    let transcript = {
        let mut sessions = state.sessions.lock().await;
        match sessions.get_chat_state(session_id) {
            Ok(cs) => render_transcript_tail(&cs.conversation),
            Err(_) => String::new(), // checked out by the running loop — fine
        }
    };

    let system_prompt = concat!(
        "You are an independent permission reviewer for a coding agent. ",
        "You see ONE proposed action that needs approval. Decide with the ",
        "principle: allow only what is safe, reversible, and within the ",
        "user's project scope; deny anything that leaks secrets, reaches ",
        "sensitive paths, is destructive, or is clearly outside the task. ",
        "Reply with exactly one line:\n",
        "- `ALLOW`\n",
        "- `DENY:<reason>`\n",
        "Do not output anything else."
    );
    let user = format!(
        "Session: {session_id}\nWorkspace: {}\nTool: {tool_name}\nArguments: {}\n\nRecent context:\n{}",
        workspace
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        args,
        transcript,
    );

    let request = LlmRequest {
        model: model.clone(),
        provider: Some(provider),
        messages: vec![ConversationItem::user(user)],
        system_prompt: system_prompt.to_string(),
        stream: false,
        ..Default::default()
    };

    let response = state
        .llm_client
        .complete(&request)
        .await
        .map_err(|e| format!("Auto-Review 评审调用失败: {e}"))?;

    // The reviewer's tokens count toward the session that triggered it.
    let usage_tracker = state.usage_tracker(session_id).await;
    usage_tracker.record_llm_usage(0, &response.usage);

    let content = response.content.trim();
    let upper = content.to_uppercase();
    if upper.starts_with("DENY") {
        let reason = content
            .trim_start_matches("DENY")
            .trim_start_matches(':')
            .trim()
            .to_string();
        Ok(AutoReviewVerdict {
            allow: false,
            reason: if reason.is_empty() {
                "Auto-Review 拒绝该操作".to_string()
            } else {
                reason
            },
        })
    } else {
        Ok(AutoReviewVerdict {
            allow: true,
            reason: "Auto-Review 允许该操作".to_string(),
        })
    }
}

/// Render a compact tail of the conversation for the reviewer (trimmed).
fn render_transcript_tail(conversation: &[ConversationItem]) -> String {
    let mut out = String::new();
    let mut chars = 0usize;
    for item in conversation.iter().rev().take(24) {
        let line = match item {
            ConversationItem::User(m) => {
                let text: Vec<String> = m
                    .content
                    .iter()
                    .filter_map(|p| match p {
                        crate::core::types::ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect();
                format!("user: {}", text.join(" "))
            }
            ConversationItem::Assistant(m) => format!("assistant: {}", m.content),
            ConversationItem::ToolResult(m) => {
                format!("tool({}): {}", m.tool_call_id, m.content)
            }
            ConversationItem::System(m) => format!("system: {}", m.content),
            ConversationItem::Reasoning(_) => continue,
        };
        chars += line.chars().count();
        if chars > REVIEW_TRANSCRIPT_CHARS {
            break;
        }
        out.insert_str(0, &format!("{line}\n"));
    }
    if out.chars().count() > REVIEW_TRANSCRIPT_CHARS {
        out = out.chars().take(REVIEW_TRANSCRIPT_CHARS).collect();
    }
    out
}

/// Emit an app event so the frontend can surface auto-review denials.
pub fn emit_denied(
    app: &AppHandle,
    session_id: &str,
    tool_name: &str,
    args: &Value,
    reason: &str,
) {
    let _ = app.emit(
        "auto-review-denied",
        serde_json::json!({
            "session_id": session_id,
            "tool_name": tool_name,
            "args": args,
            "reason": reason,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_trips_on_three_consecutive_denials() {
        let mut t = AutoReviewTracker::default();
        assert!(!t.tripped());
        t.record(true);
        t.record(true);
        assert!(!t.tripped());
        t.record(true);
        assert!(t.tripped());
    }

    #[test]
    fn allow_resets_consecutive_counter() {
        let mut t = AutoReviewTracker::default();
        t.record(true);
        t.record(true);
        t.record(false);
        t.record(true);
        assert!(!t.tripped(), "an allow resets the consecutive chain");
    }

    #[test]
    fn breaker_trips_on_ten_denials_in_window() {
        let mut t = AutoReviewTracker::default();
        // Interleave allows so the consecutive chain never trips first —
        // the rolling-window condition is what must fire at 10/50.
        for i in 0..10 {
            t.record(true);
            if i < 9 {
                t.record(false);
            }
        }
        assert!(t.tripped(), "10 denies in the 50-window must trip the breaker");
        // Sanity: 9 denies in the window stay under the threshold.
        let mut u = AutoReviewTracker::default();
        for _ in 0..9 {
            u.record(true);
            u.record(false);
        }
        assert!(!u.tripped());
    }

    #[test]
    fn window_slides_past_old_denials() {
        let mut t = AutoReviewTracker::default();
        for _ in 0..50 {
            t.record(true);
        }
        assert!(t.tripped());
        // 41 allows slide the old denials out of the 50-window.
        for _ in 0..41 {
            t.record(false);
        }
        assert!(!t.tripped());
    }

    #[test]
    fn transcript_rendering_keeps_tail() {
        let conv = vec![
            ConversationItem::user("修复 bug"),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "开始检查。".into(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        let text = render_transcript_tail(&conv);
        assert!(text.contains("修复 bug"));
        assert!(text.contains("开始检查"));
    }
}
