//! Scheduled agent tasks — the persistent "定时任务" subsystem.
//!
//! Unlike the shell-command scheduler (`tools/builtin/scheduler.rs`), a
//! scheduled agent task runs a FULL agent session in the background:
//! create a real session → run the agent loop → persist the transcript →
//! record a run row in the Scheduled inbox. Runs are unattended:
//! permission prompts become denials (never a 30s stall) and `ask_user`
//! is unavailable — the loop must adapt or fail with a clear reason.
//!
//! ## Schedule kinds
//! - `Interval { every_secs }` — fixed period, minimum 60s (an agent task
//!   is far heavier than a shell command).
//! - `Daily { time }` — once per local day at "HH:MM" (24h clock).
//!
//! ## Worktree isolation
//! When `use_worktree` is set and the project is a git repository, each
//! run executes in a fresh linked worktree and the changes stay there for
//! review — they are never auto-merged into the user's working tree.

pub mod runner;
pub mod store;

pub use runner::AutomationRunner;
pub use store::AutomationStore;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A persisted scheduled agent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    /// The user prompt every run starts with.
    pub prompt: String,
    pub schedule: ScheduleSpec,
    /// Working directory for the run. Empty = the app's current workspace.
    pub project_path: String,
    /// Run in an isolated git worktree (git repos only).
    pub use_worktree: bool,
    /// Product surface: "code" | "depwork".
    pub work_mode: String,
    /// Model override. Empty = the configured default model.
    pub model: String,
    /// Persistent mode: the agent reuses ONE session across fires so its
    /// context/goal accumulates (it "lives"), instead of a fresh disposable
    /// session per run.
    pub persistent: bool,
    /// The session the persistent agent owns — written back by the runner on
    /// the first fire, reused on subsequent ones. None for one-shot tasks.
    pub persistent_session_id: Option<String>,
    pub active: bool,
    pub last_run_at_ms: Option<i64>,
    pub run_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Schedule specification for a scheduled task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    Interval { every_secs: i64 },
    Daily { time: String },
}

impl ScheduleSpec {
    /// Parse the wire shape produced by the frontend.
    pub fn parse(kind: &str, every_secs: i64, daily_time: &str) -> Result<Self, String> {
        match kind {
            "interval" => {
                if every_secs < 60 {
                    return Err("interval must be at least 60 seconds".to_string());
                }
                Ok(Self::Interval {
                    every_secs: every_secs.min(86_400 * 30),
                })
            }
            "daily" => {
                let ok = daily_time.len() == 5
                    && daily_time.as_bytes()[2] == b':'
                    && daily_time[..2].parse::<u8>().is_ok_and(|h| h < 24)
                    && daily_time[3..].parse::<u8>().is_ok_and(|m| m < 60);
                if !ok {
                    return Err("daily time must be 'HH:MM' (24h)".to_string());
                }
                Ok(Self::Daily {
                    time: daily_time.to_string(),
                })
            }
            _ => Err(format!("unknown schedule kind '{kind}'")),
        }
    }

    /// Next due time (epoch ms) after `now_ms`. `None` when no run is due.
    pub fn next_due_ms(&self, last_run_ms: Option<i64>, now_ms: i64) -> Option<i64> {
        match self {
            Self::Interval { every_secs } => {
                let period_ms = every_secs.saturating_mul(1000).max(1);
                let last = last_run_ms.unwrap_or(0);
                if last == 0 || now_ms.saturating_sub(last) >= period_ms {
                    Some(now_ms)
                } else {
                    None
                }
            }
            Self::Daily { time } => {
                let (hh, mm) = split_daily_time(time)?;
                let today = daily_at_ms(now_ms, hh, mm);
                let last = last_run_ms.unwrap_or(0);
                if last >= today {
                    None
                } else if today <= now_ms {
                    Some(now_ms)
                } else {
                    None
                }
            }
        }
    }
}

/// One execution record of a scheduled task (the Scheduled inbox).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledRun {
    pub id: String,
    pub task_id: String,
    pub session_id: Option<String>,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Short human-readable outcome (truncated last assistant text).
    pub summary: String,
    pub error: String,
    /// Worktree path when this run used worktree isolation.
    pub worktree_path: String,
}

/// Lifecycle of a scheduled run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            "cancelled" => Self::Cancelled,
            _ => Self::Pending,
        }
    }
}

/// Split "HH:MM" into (hour, minute). `None` when malformed.
fn split_daily_time(time: &str) -> Option<(u8, u8)> {
    if time.len() != 5 || time.as_bytes()[2] != b':' {
        return None;
    }
    let hh = time[..2].parse::<u8>().ok().filter(|h| *h < 24)?;
    let mm = time[3..].parse::<u8>().ok().filter(|m| *m < 60)?;
    Some((hh, mm))
}

/// Epoch ms of `HH:MM` on the same local day as `now_ms`.
fn daily_at_ms(now_ms: i64, hh: u8, mm: u8) -> i64 {
    let now = DateTime::<Utc>::from_timestamp_millis(now_ms).unwrap_or_else(Utc::now);
    let local = now.with_timezone(&chrono::Local);
    let target = local
        .date_naive()
        .and_hms_opt(u32::from(hh), u32::from(mm), 0)
        .and_then(|naive| naive.and_local_timezone(chrono::Local).earliest())
        .unwrap_or(local);
    target.timestamp_millis()
}

/// Extract a short completion summary from the conversation tail: the last
/// assistant text message (truncated), or a fallback turn count.
pub fn summarize_conversation(
    conversation: &[crate::core::types::ConversationItem],
    turns: u64,
) -> String {
    const MAX_SUMMARY_CHARS: usize = 600;
    for item in conversation.iter().rev() {
        if let crate::core::types::ConversationItem::Assistant(msg) = item {
            let text = msg.content.trim();
            if !text.is_empty() {
                let mut summary = text.to_string();
                if summary.chars().count() > MAX_SUMMARY_CHARS {
                    summary = summary.chars().take(MAX_SUMMARY_CHARS).collect::<String>();
                    summary.push('…');
                }
                return summary;
            }
        }
    }
    format!("完成（共 {turns} 回合）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_schedule_due_logic() {
        let spec = ScheduleSpec::Interval { every_secs: 300 };
        assert_eq!(spec.next_due_ms(None, 1_000_000), Some(1_000_000));
        assert_eq!(spec.next_due_ms(Some(1_000_000), 1_000_000), None);
        assert_eq!(
            spec.next_due_ms(Some(1_000_000), 1_000_000 + 299_999),
            None
        );
        assert_eq!(
            spec.next_due_ms(Some(1_000_000), 1_000_000 + 300_000),
            Some(1_300_000)
        );
    }

    #[test]
    fn interval_minimum_is_60_seconds() {
        assert!(ScheduleSpec::parse("interval", 30, "").is_err());
        assert!(ScheduleSpec::parse("interval", 60, "").is_ok());
    }

    #[test]
    fn daily_time_validation() {
        assert!(ScheduleSpec::parse("daily", 0, "08:30").is_ok());
        assert!(ScheduleSpec::parse("daily", 0, "24:00").is_err());
        assert!(ScheduleSpec::parse("daily", 0, "8:30").is_err());
        assert!(ScheduleSpec::parse("daily", 0, "08:60").is_err());
        assert!(ScheduleSpec::parse("weekly", 0, "").is_err());
    }

    #[test]
    fn daily_fires_once_per_day_after_time() {
        use chrono::TimeZone;
        let spec = ScheduleSpec::Daily {
            time: "09:00".to_string(),
        };
        // 2026-08-09 08:00 local — not yet due.
        let before = chrono::Local
            .with_ymd_and_hms(2026, 8, 9, 8, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        assert_eq!(spec.next_due_ms(None, before), None);
        // 10:00 local — due now, and stays not-due again until tomorrow.
        let at = chrono::Local
            .with_ymd_and_hms(2026, 8, 9, 10, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        assert_eq!(spec.next_due_ms(None, at), Some(at));
        assert_eq!(spec.next_due_ms(Some(at), at), None);
        let tomorrow = chrono::Local
            .with_ymd_and_hms(2026, 8, 10, 9, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        assert_eq!(spec.next_due_ms(Some(at), tomorrow), Some(tomorrow));
    }

    #[test]
    fn summary_prefers_last_assistant_text() {
        use crate::core::types::{AssistantMessage, ConversationItem, ToolCall};
        let conv = vec![
            ConversationItem::user("do it"),
            ConversationItem::ToolResult(
                crate::core::types::ToolResultMessage {
                    tool_call_id: "t1".into(),
                    content: "ok".into(),
                    is_error: false,
                },
            ),
            ConversationItem::Assistant(AssistantMessage {
                content: "完成：修复了 bug，测试全绿。".into(),
                tool_calls: vec![ToolCall {
                    id: "t1".into(),
                    name: "bash".into(),
                    arguments: "{\"command\":\"npm test\"}".into(),
                }],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        let s = summarize_conversation(&conv, 1);
        assert!(s.contains("修复了 bug"));
    }

    #[test]
    fn summary_falls_back_to_turn_count() {
        let s = summarize_conversation(&[], 3);
        assert_eq!(s, "完成（共 3 回合）");
    }
}
