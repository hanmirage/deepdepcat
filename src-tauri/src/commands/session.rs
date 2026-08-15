//! Session commands — create, list, delete, restore sessions.

use crate::bootstrap::AppState;
use crate::core::types::Session;
use crate::hooks::{HookContext, HookEvent};
use tauri::State;

/// Get the declared goal for a session (update_goal tool / UI capsule).
#[tauri::command]
pub async fn get_session_goal(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state.goal_store.get(&session_id))
}

/// Set (or clear with an empty string) the declared goal for a session.
#[tauri::command]
pub async fn set_session_goal(
    session_id: String,
    goal: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.goal_store.set(&session_id, goal);
    Ok(())
}

/// Get the persisted todo list for a session (todo_write tool) — the
/// frontend task-progress panel re-hydrates from here when a session opens.
#[tauri::command]
pub async fn get_session_todos(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::tools::builtin::todo_write::TodoItem>, String> {
    Ok(state.todo_store.get(&session_id).unwrap_or_default())
}

/// Create a new session.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    model: Option<String>,
    provider: Option<String>,
    system_prompt: Option<String>,
    workspace_path: Option<String>,
    work_mode: Option<String>,
    context_window: Option<u64>,
    permission_mode: Option<String>,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let (model, provider) = {
        let config = state.config().map_err(|e| e.to_string())?;
        (
            model.unwrap_or_else(|| config.app.default_model.clone()),
            provider.unwrap_or_else(|| config.app.default_provider.clone()),
        )
    };

    let session = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .create_session(
            model,
            provider,
            system_prompt,
            workspace_path,
            work_mode,
            context_window,
            permission_mode,
            )
            .map_err(|e| e.to_string())
    }?;
    // SessionStart hook — a session came into being (UI path). Observers
    // can initialize per-session state / audit session creation.
    state
        .hook_executor
        .execute_observe(
            &HookContext::new(HookEvent::SessionStart, &session.id)
                .with_data("model", serde_json::json!(session.model)),
        )
        .await;
    Ok(session)
}

/// List recent sessions.
#[tauri::command]
pub async fn list_sessions(
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<Session>, String> {
    let db = state.db.clone();
    let limit = limit.unwrap_or(50);
    // Offloaded: a large-history scan must not block the tokio worker.
    db.list_sessions_async(limit)
        .await
        .map_err(|e| e.to_string())
}

/// Get a session by ID.
#[tauri::command]
pub async fn get_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let mut sessions = state.sessions.lock().await;
    let session = sessions
        .get_session(&session_id)
        .map_err(|e| e.to_string())?;
    Ok(session.clone())
}

/// Delete a session.
#[tauri::command]
pub async fn delete_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut sessions = state.sessions.lock().await;
        sessions
            .delete_session(&session_id)
            .map_err(|e| e.to_string())?;
    }
    // SessionEnd hook — the session is being torn down; fire it before the
    // per-session registries are purged so observers still see the state.
    state
        .hook_executor
        .execute_observe(
            &HookContext::new(HookEvent::SessionEnd, &session_id)
                .with_data("reason", serde_json::json!("deleted")),
        )
        .await;
    // The deleted session never runs another agent-loop turn — the only
    // path that drains its background-results queue — so purge its pending
    // entries at teardown instead of leaking them in process memory.
    state
        .coordinator
        .purge_background_results(&session_id)
        .await;
    // Purge every in-memory per-session registry entry (usage trackers,
    // caches, grants, plan state, …) — they are keyed by session id and
    // would otherwise leak for the app's whole lifetime.
    state.cleanup_session(&session_id).await;
    Ok(())
}

/// Get session messages (conversation history).
#[tauri::command]
pub async fn get_session_messages(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::core::types::ConversationItem>, String> {
    state
        .db
        .load_messages_async(session_id)
        .await
        .map_err(|e| e.to_string())
}

/// Update session title.
#[tauri::command]
pub async fn update_session_title(
    session_id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().await;
    sessions
        .set_title(&session_id, title)
        .map_err(|e| e.to_string())
}

/// Pin or unpin a session (sidebar top-of-list placement).
#[tauri::command]
pub async fn set_session_pinned(
    session_id: String,
    pinned: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().await;
    sessions
        .set_pinned(&session_id, pinned)
        .map_err(|e| e.to_string())
}

/// Update session model.
#[tauri::command]
pub async fn update_session_model(
    session_id: String,
    model: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().await;
    sessions
        .set_model(&session_id, model)
        .map_err(|e| e.to_string())
}

/// Recall (delete) a user message and everything that followed it.
///
/// `user_content` is the plain text of the user message to remove.
/// The backend truncates the conversation at that message and persists.
#[tauri::command]
pub async fn delete_message(
    session_id: String,
    user_content: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().await;
    let chat_state = sessions
        .get_chat_state(&session_id)
        .map_err(|e| e.to_string())?;

    if !chat_state.truncate_from_user_message(&user_content) {
        return Err("Message not found in conversation".to_string());
    }

    sessions
        .persist_messages(&session_id)
        .map_err(|e| e.to_string())?;
    Ok(())
}
