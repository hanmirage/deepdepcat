//! Plan approval — the "pause and plan with the user" mechanism.
//!
//! Mirrors the plan-approval flow: the agent enters plan mode
//! (`enter_plan_mode`), writes a plan, then calls `exit_plan_mode` — which is
//! INTERCEPTED (the tool does not return immediately). The backend registers
//! a blocking approval request, the frontend shows a plan-approval panel,
//! and the tool future stays parked until the user decides:
//!
//! - `Approved` → the previous permission mode is restored and the tool
//!   returns "plan approved — start coding" so the model continues
//!   automatically in the next loop iteration.
//! - `Rejected(feedback)` → the agent stays in plan mode; the feedback is
//!   returned as a tool error so the model revises the plan and re-submits.
//!
//! The mode bookkeeping (enter → record previous mode → exit → restore) lives
//! on `AppState` (`plan_previous_modes`); this module provides the pending
//! request registry and the git-change summary used by the approval panel.

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// The user's decision on a submitted plan.
#[derive(Debug, Clone)]
pub enum PlanDecision {
    /// Plan approved — leave plan mode and start implementing.
    Approved,
    /// Plan rejected — stay in plan mode, revise per the feedback.
    Rejected(String),
}

/// A parked plan-approval request.
pub struct PendingPlanApproval {
    pub sender: tokio::sync::oneshot::Sender<PlanDecision>,
    pub session_id: String,
}

/// A user interaction the agent is parked on — surfaced to the frontend as
/// a "waiting for you" status. Kinds: `permission` | `plan` | `question`.
#[derive(Debug, Clone, Serialize)]
pub struct PendingInteraction {
    pub kind: &'static str,
    pub request_id: String,
    pub summary: String,
    pub since: u64,
}

/// A single step of an approved plan — the structured planner (P2-5).
///
/// After `exit_plan_mode` is approved, the plan text is parsed into these
/// steps and stored per session. The loop's checklist gate reminds the model
/// to walk through them one by one instead of improvising.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub text: String,
    pub done: bool,
}

/// Maximum plan steps kept per session (plans beyond this are truncated).
const MAX_PLAN_STEPS: usize = 20;

/// Parse an approved plan into structured steps.
///
/// Recognizes numbered ("1. …", "1) …", "1、…"), bullet ("- …", "* …"),
/// and labeled ("步骤 1: …", "Step 1: …") list lines. Non-list lines
/// (headings, prose, code fences) are ignored. Returns an empty vec when
/// the plan has no recognizable structure — the loop then skips the
/// checklist gate and relies on the model's own tracking.
pub fn parse_plan_steps(plan: &str) -> Vec<PlanStep> {
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut in_fence = false;
    for line in plan.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.is_empty() {
            continue;
        }
        let text = extract_step_text(line);
        if let Some(text) = text {
            if text.chars().count() >= 4 {
                steps.push(PlanStep {
                    id: format!("step-{}", steps.len() + 1),
                    text,
                    done: false,
                });
                if steps.len() >= MAX_PLAN_STEPS {
                    break;
                }
            }
        }
    }
    steps
}

/// Extract the step text from a list line, or `None` if not a list line.
fn extract_step_text(line: &str) -> Option<String> {
    // Numbered: "1. …" / "1) …" / "1、…" / "1．…" / "（1）…" / "(1) …"
    if let Some(idx) = line.find(|c: char| c.is_ascii_digit()) {
        let rest = &line[idx..];
        let num_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        let after = rest[num_len..].trim_start();
        let tail = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix("."))
            .or_else(|| after.strip_prefix(") "))
            .or_else(|| after.strip_prefix(")"))
            .or_else(|| after.strip_prefix("、"))
            .or_else(|| after.strip_prefix("．"))
            .or_else(|| after.strip_prefix("）"))
            .or_else(|| after.strip_prefix(")"));
        if let Some(tail) = tail {
            let t = tail.trim();
            if !t.is_empty() && !t.starts_with(|c: char| c.is_ascii_digit()) {
                return Some(t.to_string());
            }
        }
    }
    // Bullets: "- …" / "* …" / "• …" / "· …"
    for prefix in ["- ", "* ", "• ", "· "] {
        if let Some(tail) = line.strip_prefix(prefix) {
            if !tail.trim().is_empty() {
                return Some(tail.trim().to_string());
            }
        }
    }
    // Labeled: "Step 1: …" / "步骤 1: …" / "第 1 步：…" / "第一步：…"
    let lower = line.to_lowercase();
    if lower.starts_with("step ") || lower.starts_with("步骤") || lower.starts_with("第") {
        if let Some((colon, colon_char)) =
            line.char_indices().find(|(_, c)| *c == ':' || *c == '：')
        {
            let head = line[..colon].to_lowercase();
            let head_has_step_word =
                head.contains("step") || head.contains("步骤") || head.contains("步");
            let head_has_number = head.chars().any(|c| c.is_ascii_digit())
                || head.contains('一')
                || head.contains('二')
                || head.contains('三')
                || head.contains('四')
                || head.contains('五');
            if head_has_step_word && head_has_number {
                let tail = line[colon + colon_char.len_utf8()..].trim();
                if !tail.is_empty() {
                    return Some(tail.to_string());
                }
            }
        }
    }
    None
}

/// Map parsed plan steps to task-panel todo items (plan → todo bridge).
///
/// Each step becomes a pending `TodoItem` whose `content` is the step text;
/// ids are `plan-<n>` so the model can reference them in `todo_write` and
/// mark them done as it executes. The bridge seeds the task panel when the
/// session has no todo list yet — an existing list is never overwritten.
pub fn plan_steps_to_todos(steps: &[PlanStep]) -> Vec<crate::tools::builtin::todo_write::TodoItem> {
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| crate::tools::builtin::todo_write::TodoItem {
            id: format!("plan-{}", i + 1),
            content: s.text.clone(),
            status: crate::tools::builtin::todo_write::TodoStatus::Pending,
            priority: None,
            parent_id: None,
            depends_on: None,
            verify: None,
        })
        .collect()
}

/// Collect the workspace's current git change summary for the approval panel.
///
/// Returns the list of changed/untracked paths (e.g. `M src/main.rs`,
/// `?? plan.md`). Empty on non-git workspaces or any failure — the panel
/// degrades gracefully to plan-text-only.
pub fn collect_changed_files(workspace: Option<&std::path::Path>) -> Vec<String> {
    let Some(ws) = workspace else {
        return Vec::new();
    };
    let mut cmd = std::process::Command::new("git");
    crate::core::proc::no_window(&mut cmd);
    cmd.args(["-c", "core.quotepath=false"]);
    let output = cmd.args(["status", "--porcelain"]).current_dir(ws).output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    raw.lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .take(50)
        .collect()
}

/// Current unix seconds (for the "waiting since" display).
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One plan hindsight record — what a plan predicted and what actually
/// happened (or why the user pushed back). Persisted per workspace to
/// `.deepdepcat/plans/_reflections.json`; the plan-writer reads the recent
/// history when entering plan mode so it can calibrate against outcomes.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct PlanReflection {
    pub at: u64,
    pub session_id: String,
    /// First meaningful line of the plan — what the task was about.
    pub plan_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_done: Option<usize>,
    /// The user's rejection feedback, when the approved plan was rejected at
    /// least once before approval — the most calibrating signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

/// Cap on retained plan reflections (FIFO) — the plan-writer only needs the
/// recent past, and the file must not grow without bound.
const MAX_REFLECTIONS: usize = 20;

fn reflections_path(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace
        .join(".deepdepcat")
        .join("plans")
        .join("_reflections.json")
}

/// Read the workspace's recent plan reflections (empty when none/absent).
pub fn read_plan_reflections(workspace: &std::path::Path) -> Vec<PlanReflection> {
    let Ok(raw) = std::fs::read_to_string(reflections_path(workspace)) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Append one plan reflection to the workspace history (FIFO-capped).
pub fn append_plan_reflection(workspace: &std::path::Path, reflection: PlanReflection) {
    let mut list = read_plan_reflections(workspace);
    list.push(reflection);
    if list.len() > MAX_REFLECTIONS {
        list.drain(..list.len() - MAX_REFLECTIONS);
    }
    if let Ok(json) = serde_json::to_string(&list) {
        if let Some(dir) = reflections_path(workspace).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(reflections_path(workspace), json);
    }
}

/// Broadcast the session's current pending interactions to the frontend.
///
/// Callers invoke this after registering or resolving any interaction so the
/// "waiting for you" indicator stays in sync across permission dialogs,
/// plan-approval panels, and ask_user cards.
pub async fn broadcast_pending_interactions(app: &tauri::AppHandle, session_id: &str) {
    use tauri::{Emitter, Manager};
    let state = app.state::<crate::bootstrap::AppState>();
    let snapshot = state.pending_interactions_snapshot(session_id).await;
    let payload = serde_json::json!({
        "session_id": session_id,
        "interactions": snapshot,
    });
    let _ = app.emit("pending-interactions", &payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbered_steps() {
        let plan = "Implementation plan:\n1. Create the module skeleton\n2. Add the API layer\n3. Wire the UI";
        let steps = parse_plan_steps(plan);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].text, "Create the module skeleton");
        assert_eq!(steps[2].text, "Wire the UI");
        assert!(!steps[0].done);
    }

    #[test]
    fn parses_bullet_and_labeled_steps() {
        let plan = "- First thing\n* Second thing\nStep 3: Final wiring";
        let steps = parse_plan_steps(plan);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1].text, "Second thing");
        assert_eq!(steps[2].text, "Final wiring");
    }

    #[test]
    fn ignores_prose_and_code_fences() {
        let plan = "We should refactor this.\n```\n1. not a step\n```\nA paragraph explaining the trade-offs.";
        let steps = parse_plan_steps(plan);
        assert!(steps.is_empty());
    }

    #[test]
    fn caps_at_max_steps() {
        let mut lines = Vec::new();
        for i in 1..=30 {
            lines.push(format!("{i}. Step number {i}"));
        }
        let steps = parse_plan_steps(&lines.join("\n"));
        assert_eq!(steps.len(), MAX_PLAN_STEPS);
    }

    #[test]
    fn chinese_numbered_steps() {
        let plan = "1、创建骨架\n2、添加接口\n3、接入前端";
        let steps = parse_plan_steps(plan);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].text, "创建骨架");
    }

    #[test]
    fn chinese_step_variants() {
        let plan = "（1）创建骨架\n第 2 步：添加接口\n第一步：接入前端\n1.合并发布";
        let steps = parse_plan_steps(plan);
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].text, "创建骨架");
        assert_eq!(steps[1].text, "添加接口");
        assert_eq!(steps[2].text, "接入前端");
        assert_eq!(steps[3].text, "合并发布");
    }

    #[test]
    fn labeled_prose_without_step_word_is_ignored() {
        // "第一件事：" is prose, not a step label — no "步" marker.
        let plan = "第一件事：先看看整体结构\n然后我们讨论方案";
        let steps = parse_plan_steps(plan);
        assert!(steps.is_empty());
    }

    #[test]
    fn plan_step_serializes() {
        let step = PlanStep {
            id: "step-1".into(),
            text: "Do the thing".into(),
            done: false,
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains("step-1"));
        assert!(json.contains("Do the thing"));
    }

    #[test]
    fn plan_steps_bridge_to_todo_items() {
        let steps = parse_plan_steps("1. Create skeleton\n2. Wire the API");
        let todos = plan_steps_to_todos(&steps);
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].id, "plan-1");
        assert_eq!(todos[0].content, "Create skeleton");
        assert_eq!(
            todos[0].status,
            crate::tools::builtin::todo_write::TodoStatus::Pending
        );
        assert_eq!(todos[1].id, "plan-2");
        assert_eq!(todos[1].content, "Wire the API");
    }

    #[test]
    fn empty_plan_bridges_to_empty_todos() {
        let todos = plan_steps_to_todos(&[]);
        assert!(todos.is_empty());
    }

    #[test]
    fn reflections_roundtrip_and_keep_feedback() {
        let dir = std::env::temp_dir().join(format!("ddc-refl-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_plan_reflections(&dir).is_empty());

        append_plan_reflection(
            &dir,
            PlanReflection {
                at: 1,
                session_id: "s1".into(),
                plan_hint: "refactor the auth layer".into(),
                steps_total: Some(4),
                steps_done: Some(3),
                feedback: Some("scope too big".into()),
            },
        );
        let list = read_plan_reflections(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].plan_hint, "refactor the auth layer");
        assert_eq!(list[0].steps_done, Some(3));
        assert_eq!(list[0].feedback.as_deref(), Some("scope too big"));
    }

    #[test]
    fn reflections_are_fifo_capped() {
        let dir = std::env::temp_dir().join(format!("ddc-refl-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..(MAX_REFLECTIONS + 5) {
            append_plan_reflection(
                &dir,
                PlanReflection {
                    at: i as u64,
                    session_id: format!("s{i}"),
                    plan_hint: format!("task {i}"),
                    steps_total: None,
                    steps_done: None,
                    feedback: None,
                },
            );
        }
        let list = read_plan_reflections(&dir);
        assert_eq!(list.len(), MAX_REFLECTIONS);
        // The oldest entries were dropped; the most recent survive.
        let first_kept = (MAX_REFLECTIONS + 5) - MAX_REFLECTIONS;
        assert_eq!(list.first().unwrap().plan_hint, format!("task {first_kept}"));
        assert_eq!(list.last().unwrap().plan_hint, format!("task {}", MAX_REFLECTIONS + 4));
    }
}
