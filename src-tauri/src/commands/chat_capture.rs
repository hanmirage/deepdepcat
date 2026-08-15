//! Self-evolution capture after a successful turn — background learning
//! extraction and reusable-workflow procedure capture, both throttled to
//! once per 10 minutes per session. Extracted from `chat.rs` so the send
//! path stays within the file-size budget.

use crate::agent::chat_state::ChatState;
use crate::bootstrap::AppState;
use crate::toolkit::WorkMode;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CAPTURE_THROTTLE: Duration = Duration::from_secs(600);

/// A persisted project-cognition note older than this is regenerated in the
/// background — it is a static snapshot with no invalidation hook, so a stale
/// map (new files, changed structure) is worse than a slightly-late one.
const COGNITION_FRESHNESS: Duration = Duration::from_secs(24 * 3600);

/// True when the session has not run a capture within the throttle window;
/// also records the run so the next call is suppressed.
async fn throttle_due(
    last: &Arc<tokio::sync::Mutex<HashMap<String, Instant>>>,
    session_id: &str,
) -> bool {
    let mut map = last.lock().await;
    let now = Instant::now();
    match map.get(session_id) {
        Some(prev) if now.duration_since(*prev) < CAPTURE_THROTTLE => false,
        _ => {
            map.insert(session_id.to_string(), now);
            true
        }
    }
}

/// Background learning extraction — after a turn that actually changed
/// files, extract non-obvious learnings (memory + workspace learnings.md).
/// Failures are silent — never block the turn.
pub async fn maybe_capture_learnings(
    state: &AppState,
    session_id: &str,
    chat_state: &ChatState,
    ok: bool,
) {
    if !ok || chat_state.agent_edited_paths.is_empty() || chat_state.conversation.len() < 10 {
        return;
    }
    tracing::debug!(
        session_id = %session_id,
        edited = chat_state.agent_edited_paths.len(),
        conversation_items = chat_state.conversation.len(),
        "procedure capture candidate (chat)"
    );
    if !throttle_due(&state.learning_last_run, session_id).await {
        return;
    }
    let llm = state.llm_client.clone();
    let memory = state.memory.clone();
    let model = chat_state.model.clone();
    let provider = chat_state.provider.clone();
    let conversation = chat_state.conversation.clone();
    let ws = state.workspace.read().ok().and_then(|w| w.clone());
    tauri::async_runtime::spawn(async move {
        let _ = crate::memory::learning::run_learning_pass(
            &llm,
            &model,
            provider.as_deref(),
            &conversation,
            &memory,
            ws.as_deref(),
        )
        .await;
    });
}

/// Background procedure capture — extract 0-1 reusable workflows into the
/// project procedures.md (mode-locked to this session), throttled like
/// learning. Failures are silent — never block the turn.
pub async fn maybe_capture_procedure(
    state: &AppState,
    session_id: &str,
    chat_state: &ChatState,
    ok: bool,
    work_mode: WorkMode,
) {
    if !ok || chat_state.agent_edited_paths.is_empty() || chat_state.conversation.len() < 10 {
        return;
    }
    if !throttle_due(&state.procedure_last_run, session_id).await {
        return;
    }
    let llm = state.llm_client.clone();
    let model = chat_state.model.clone();
    let provider = chat_state.provider.clone();
    let conversation = chat_state.conversation.clone();
    let ws = state.workspace.read().ok().and_then(|w| w.clone());
    let mode = work_mode.as_str().to_string();
    tauri::async_runtime::spawn(async move {
        let _ = crate::memory::procedure_capture::run_procedure_pass(
            &llm,
            &model,
            provider.as_deref(),
            &conversation,
            ws.as_deref(),
            &mode,
        )
        .await;
    });
}

/// Background project-cognition generation — after a turn, generate the
/// workspace's `.deepdepcat/project-cognition.md` ONCE (an LLM architecture
/// note over the deterministic module snapshot), so long-task planning on
/// later sessions starts with a project map. Failures are silent — never
/// block the turn.
pub async fn maybe_capture_project_cognition(
    state: &AppState,
    chat_state: &ChatState,
    ok: bool,
) {
    if !ok {
        return;
    }
    let Some(ws) = state.workspace.read().ok().and_then(|w| w.clone()) else {
        return;
    };
    // Already generated AND fresh → skip. A note older than
    // COGNITION_FRESHNESS is regenerated in the background — the project may
    // have grown new files since it was written (the note has no invalidation
    // hook), so a stale map misleads planning more than a slightly-late one.
    if let Some(path) = crate::memory::project_cognition::cognition_path(Some(&ws)) {
        let fresh = crate::memory::project_cognition::read_cognition(&path).is_some()
            && std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|e| e < COGNITION_FRESHNESS);
        if fresh {
            return;
        }
    }
    // One generation attempt per workspace.
    {
        let mut tried = state.cognition_tried.lock().await;
        if tried.contains(&ws) {
            return;
        }
        tried.insert(ws.clone());
    }
    let llm = state.llm_client.clone();
    let model = chat_state.model.clone();
    let provider = chat_state.provider.clone();
    let graph = state.dependency_graph.clone();
    let symbols = state.symbol_index.clone();
    tauri::async_runtime::spawn(async move {
        // Ensure the dependency graph is built (lazy, like code_search).
        let graph = {
            let existing = graph.read().ok().and_then(|g| g.clone());
            match existing {
                Some(g) => g,
                None => {
                    let mut g = crate::codebase::dependency::DependencyGraph::new(&ws);
                    g.build();
                    if let Ok(mut slot) = graph.write() {
                        *slot = Some(g.clone());
                    }
                    g
                }
            }
        };
        let symbols = symbols.read().ok().map(|g| g.clone()).unwrap_or_default();
        let project_type = crate::agent::discovery::discover(&ws).project_type;
        let _ = crate::memory::project_cognition::generate_project_cognition(
            &llm,
            &model,
            provider.as_deref(),
            &ws,
            &graph,
            &symbols,
            &project_type,
        )
        .await;
    });
}
