//! Session manager — creates, restores, lists, and deletes chat sessions.
//!
//! Each session has its own `ChatState` and metadata. Sessions are persisted
//! to the SQLite database.
//!
//! ## Lock discipline
//!
//! The `SessionManager` mutex must never be held across a long-running
//! operation (LLM streaming, tool execution). Use [`take_chat_state`] /
//! [`put_chat_state`] to extract the `ChatState` before running the agent
//! loop, then write it back afterwards.

use crate::agent::chat_state::ChatState;
use crate::core::error::{AppError, AppResult};
use crate::agent::running::turn_message_preview;
use crate::core::types::conversation::{ContentPart, ConversationItem};
use crate::core::types::{Session, SessionStatus};
use crate::llm::models::ModelCatalog;
use crate::storage::database::Database;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

/// Manages all chat sessions — in-memory state + database persistence.
pub struct SessionManager {
    /// Active sessions (session_id → Session + optional ChatState).
    sessions: HashMap<String, ActiveSession>,
    /// The model catalog for context window lookups.
    model_catalog: ModelCatalog,
    db: Arc<Database>,
}

/// First non-empty message preview for the sidebar row — walks the
/// conversation backwards (last message wins), extracting text from User
/// parts or the Assistant content; tool results / reasoning / system are
/// skipped (they are noise for a "what is this session about" preview).
fn last_message_preview(conv: &[ConversationItem]) -> String {
    for item in conv.iter().rev() {
        let text = match item {
            ConversationItem::User(msg) => msg
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            ConversationItem::Assistant(msg) => msg.content.clone(),
            _ => continue,
        };
        if !text.trim().is_empty() {
            return turn_message_preview(&text);
        }
    }
    String::new()
}

/// A session whose `ChatState` may be "checked out" by the agent loop.
///
/// When `chat_state` is `None`, the state has either been taken out by
/// [`SessionManager::take_chat_state`] (must be restored with
/// [`SessionManager::put_chat_state`]) or evicted to the database by the
/// idle-reaper (see [`SessionManager::evict_idle`]) — in the latter case any
/// accessor transparently re-loads it from the database.
pub struct ActiveSession {
    pub session: Session,
    pub chat_state: Option<ChatState>,
    /// Last moment a turn finished for this session (idle tracking).
    pub last_active_at: Option<DateTime<Utc>>,
}

impl SessionManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            sessions: HashMap::new(),
            model_catalog: ModelCatalog::new(),
            db,
        }
    }

    /// Rebuild a `ChatState` from the database for a session whose memory
    /// was evicted (idle reaper) — mirrors the load path in `ensure_loaded`.
    fn rebuild_chat_state(&self, session: &Session) -> AppResult<ChatState> {
        let context_window = if session.context_window > 0 {
            session.context_window
        } else {
            self.model_catalog.context_window(&session.model)
        };
        let messages = self.db.load_messages(&session.id)?;
        let mut chat_state = ChatState::from_history(
            messages,
            &session.model,
            context_window,
            Some(session.provider.clone()),
        );
        chat_state.set_system_prompt(&session.system_prompt);
        chat_state.repair_dangling_tool_calls();
        Ok(chat_state)
    }

    /// Create a new session.
    #[allow(clippy::too_many_arguments)]
    pub fn create_session(
        &mut self,
        model: impl Into<String>,
        provider: impl Into<String>,
        system_prompt: Option<String>,
        workspace_path: Option<String>,
        work_mode: Option<String>,
        context_window: Option<u64>,
        permission_mode: Option<String>,
    ) -> AppResult<Session> {
        let model = model.into();
        let provider = provider.into();

        let mut session = Session::new(model.clone(), provider);
        session.context_window =
            context_window.unwrap_or_else(|| self.model_catalog.context_window(&model));
        session.workspace_path = workspace_path;
        if let Some(wm) = work_mode {
            let wm = wm.to_ascii_lowercase();
            session.work_mode = if wm == "depwork" { "depwork" } else { "code" }.to_string();
        }

        if let Some(sp) = system_prompt {
            session.system_prompt = sp;
        }
        if let Some(pm) = permission_mode {
            session.permission_mode = pm;
        }

        let mut chat_state = ChatState::with_provider(
            model,
            session.context_window,
            Some(session.provider.clone()),
        );
        if !session.system_prompt.is_empty() {
            chat_state.set_system_prompt(&session.system_prompt);
        }

        // Persist to database
        self.db.upsert_session(&session)?;

        let session_id = session.id.clone();
        self.sessions.insert(
            session_id,
            ActiveSession {
                session: session.clone(),
                chat_state: Some(chat_state),
                last_active_at: Some(Utc::now()),
            },
        );

        Ok(session)
    }

    /// Persist a per-session permission mode ("" = inherit the global
    /// default). Survives restarts through the session row.
    pub fn set_permission_mode(&mut self, session_id: &str, mode: &str) -> AppResult<()> {
        self.ensure_loaded(session_id)?;
        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        active.session.permission_mode = mode.to_string();
        self.db.upsert_session(&active.session)
    }

    /// Pin or unpin a session (sidebar top-of-list placement). Persisted on
    /// the session row via the manager (which ensures the session is loaded)
    /// so a later full-row `persist_session` cannot overwrite it with the
    /// stale in-memory value. Does NOT bump `updated_at` — pinning only
    /// moves the row into the pinned group, it should not reorder recency.
    pub fn set_pinned(&mut self, session_id: &str, pinned: bool) -> AppResult<()> {
        self.ensure_loaded(session_id)?;
        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        active.session.pinned = pinned;
        self.db.upsert_session(&active.session)
    }

    /// Get or load a session by ID.
    fn ensure_loaded(&mut self, session_id: &str) -> AppResult<()> {
        if self.sessions.contains_key(session_id) {
            return Ok(());
        }

        let session = self
            .db
            .get_session(session_id)?
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;

        let chat_state = self.rebuild_chat_state(&session)?;

        self.sessions.insert(
            session_id.to_string(),
            ActiveSession {
                session: session.clone(),
                chat_state: Some(chat_state),
                last_active_at: None,
            },
        );

        Ok(())
    }

    /// Ensure the session has a live `ChatState`, re-loading it from the
    /// database when the idle-reaper evicted it. Marks the session active
    /// again (a dormant session that is touched wakes up).
    fn ensure_chat_state(&mut self, session_id: &str) -> AppResult<()> {
        self.ensure_loaded(session_id)?;
        let needs_rebuild = {
            let active = self.sessions.get(session_id).ok_or_else(|| {
                AppError::Internal("session vanished after ensure_loaded".to_string())
            })?;
            active.chat_state.is_none() && !active.session.is_streaming
        };
        if needs_rebuild {
            let session = self.sessions[&session_id.to_string()].session.clone();
            let rebuilt = self.rebuild_chat_state(&session)?;
            if let Some(active) = self.sessions.get_mut(session_id) {
                active.chat_state = Some(rebuilt);
                active.session.status = SessionStatus::Active;
                let _ = self.db.upsert_session(&active.session);
            }
        }
        Ok(())
    }

    /// Get the live context window for a session — the `ChatState`'s value
    /// (kept current on model switch). Falls back to the model catalog when
    /// the state is checked out by a running agent loop, or the session is
    /// only known in the database. Returns `0` when the model is unknown.
    pub fn context_window(&mut self, session_id: &str) -> AppResult<u64> {
        self.ensure_loaded(session_id)?;
        if let Some(active) = self.sessions.get(session_id) {
            if let Some(cs) = &active.chat_state {
                return Ok(cs.context_window);
            }
            let fallback = if active.session.context_window > 0 {
                active.session.context_window
            } else {
                self.model_catalog.context_window(&active.session.model)
            };
            return Ok(fallback);
        }
        Ok(0)
    }

    /// Get a session (without loading it).
    pub fn get_session(&mut self, session_id: &str) -> AppResult<&Session> {
        self.ensure_loaded(session_id)?;
        self.sessions
            .get(session_id)
            .map(|a| &a.session)
            .ok_or_else(|| AppError::Internal("session vanished after ensure_loaded".to_string()))
    }

    /// Whether a session's agent loop is currently running (its `ChatState`
    /// is checked out and `is_streaming` is set). Only in-memory sessions can
    /// be running — a session not loaded can't stream.
    pub fn is_streaming(&self, session_id: &str) -> bool {
        self.sessions
            .get(session_id)
            .map(|a| a.session.is_streaming)
            .unwrap_or(false)
    }

    /// Take the `ChatState` out of the session manager.
    ///
    /// This allows the caller to run the agent loop without holding the
    /// `SessionManager` mutex. The state MUST be restored via
    /// [`put_chat_state`] before any other operation that needs it.
    ///
    /// While the state is checked out, `session.is_streaming` is set to `true`
    /// so that other callers know the session is busy.
    pub fn take_chat_state(&mut self, session_id: &str) -> AppResult<ChatState> {
        self.ensure_chat_state(session_id)?;
        let active = self.sessions.get_mut(session_id).ok_or_else(|| {
            AppError::Internal("session vanished after ensure_loaded".to_string())
        })?;
        active.session.is_streaming = true;
        active.last_active_at = Some(Utc::now());
        active.chat_state.take().ok_or_else(|| {
            AppError::Internal(format!(
                "chat_state for session '{session_id}' is already checked out \
                 — another agent loop may be running"
            ))
        })
    }

    /// Restore a previously taken `ChatState` and mark the session as not streaming.
    pub fn put_chat_state(&mut self, session_id: &str, chat_state: ChatState) -> AppResult<()> {
        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        active.session.is_streaming = false;
        active.last_active_at = Some(Utc::now());
        active.chat_state = Some(chat_state);
        Ok(())
    }

    /// Get just the chat state (must not be checked out).
    pub fn get_chat_state(&mut self, session_id: &str) -> AppResult<&mut ChatState> {
        self.ensure_chat_state(session_id)?;
        let active = self.sessions.get_mut(session_id).ok_or_else(|| {
            AppError::Internal("session vanished after ensure_loaded".to_string())
        })?;
        active.last_active_at = Some(Utc::now());
        active.chat_state.as_mut().ok_or_else(|| {
            AppError::Internal(format!(
                "chat_state for session '{session_id}' is checked out"
            ))
        })
    }

    /// Delete a session.
    pub fn delete_session(&mut self, session_id: &str) -> AppResult<()> {
        self.sessions.remove(session_id);
        self.db.delete_session(session_id)?;
        Ok(())
    }

    /// Persist a session's current state to the database.
    pub fn persist_session(&mut self, session_id: &str) -> AppResult<()> {
        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        active.session.updated_at = Utc::now();
        if let Some(ref cs) = active.chat_state {
            active.session.total_usage = cs.total_usage.clone();
            active.session.turn_count = cs.prompt_index as u64;
            active.session.last_message = last_message_preview(&cs.conversation);
        }
        // A session whose ChatState was evicted by the idle-reaper stays
        // `Idle`; everything else is active.
        active.session.status = if active.chat_state.is_some() {
            SessionStatus::Active
        } else {
            SessionStatus::Idle
        };
        self.db.upsert_session(&active.session)?;
        Ok(())
    }

    /// Evict idle sessions to the database (Cat's Dormant state).
    ///
    /// Sessions whose last finished turn is older than `idle_timeout` AND
    /// that are neither streaming nor checked out get their `ChatState`
    /// dropped (memory is freed; the database is the source of truth and
    /// [`ensure_chat_state`] reloads on next use). Returns the evicted
    /// session ids so the caller can notify the frontend.
    pub fn evict_idle(&mut self, idle_timeout: std::time::Duration) -> Vec<String> {
        let cutoff = Utc::now() - chrono::Duration::from_std(idle_timeout).unwrap_or_default();
        let mut evicted: Vec<String> = Vec::new();
        for (id, active) in self.sessions.iter_mut() {
            if active.session.is_streaming {
                continue;
            }
            // No recorded activity yet (freshly loaded) — never evict.
            let Some(last_active) = active.last_active_at else {
                continue;
            };
            if last_active >= cutoff || active.chat_state.is_none() {
                continue;
            }
            active.chat_state = None;
            active.session.status = SessionStatus::Idle;
            let evicted_id = id.clone();
            if let Err(e) = self.db.upsert_session(&active.session) {
                tracing::warn!(session_id = %evicted_id, error = %e, "Failed to persist idle session");
            }
            evicted.push(evicted_id);
        }
        evicted
    }

    /// Persist a session's conversation to the database.
    ///
    /// Steady-state persists are incremental: only the items after the
    /// `persisted_upto` checkpoint are INSERTed, so a long conversation
    /// costs O(new turn) per turn instead of O(history) (the previous
    /// delete-all + re-insert ran under a long-held write lock every turn).
    /// Structural rewrites (compaction, rewind, dangling-call repair) reset
    /// the checkpoint to 0 via `ChatState` methods, forcing the atomic full
    /// rewrite here.
    pub fn persist_messages(&mut self, session_id: &str) -> AppResult<()> {
        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        if let Some(ref mut cs) = active.chat_state {
            let total = cs.conversation.len();
            let persisted = cs.persisted_upto;
            if persisted == 0 || total < persisted {
                // First persist, or a structural rewrite happened — the
                // atomic full rewrite is the only correct path.
                self.db.replace_messages(session_id, &cs.conversation)?;
            } else if total > persisted {
                self.db.append_messages(
                    session_id,
                    persisted as i64,
                    &cs.conversation[persisted..],
                )?;
            }
            cs.persisted_upto = total;
        }
        Ok(())
    }

    /// Update the session title.
    pub fn set_title(&mut self, session_id: &str, title: impl Into<String>) -> AppResult<()> {
        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        active.session.title = title.into();
        active.session.updated_at = Utc::now();
        self.db.upsert_session(&active.session)?;
        Ok(())
    }

    /// Update the session's model.
    pub fn set_model(&mut self, session_id: &str, model: impl Into<String>) -> AppResult<()> {
        let model = model.into();
        let context_window = self.model_catalog.context_window(&model);

        let active = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        active.session.model = model.clone();
        if let Some(ref mut cs) = active.chat_state {
            cs.model = model;
            cs.context_window = context_window;
        }
        active.session.updated_at = Utc::now();
        self.db.upsert_session(&active.session)?;
        Ok(())
    }

    /// Get the model catalog.
    pub fn model_catalog(&self) -> &ModelCatalog {
        &self.model_catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SessionManager {
        let dir = std::env::temp_dir().join(format!("ddc-session-test-{}", uuid::Uuid::new_v4()));
        let db = Arc::new(Database::open(&dir.join("t.db"), false).unwrap());
        db.run_migrations().unwrap();
        SessionManager::new(db)
    }

    #[test]
    fn last_message_preview_skips_tool_result_takes_last_reply() {
        let conv = vec![
            ConversationItem::user("第一条"),
            ConversationItem::tool_result("c1", "工具结果"),
            ConversationItem::Assistant(crate::core::types::conversation::AssistantMessage {
                content: "第二条回答".to_string(),
                tool_calls: vec![],
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        // The trailing tool result is skipped; the assistant reply wins.
        assert_eq!(last_message_preview(&conv), "第二条回答");
    }

    #[test]
    fn last_message_preview_empty_without_text() {
        assert_eq!(last_message_preview(&[]), "");
        assert_eq!(last_message_preview(&[ConversationItem::tool_result("c1", "x")]), "");
    }

    #[test]
    fn last_message_preview_joins_user_text_parts() {
        let conv = vec![ConversationItem::user_with_parts(vec![
            ContentPart::Text { text: "AAA".to_string() },
            ContentPart::Text { text: "BBB".to_string() },
        ])];
        assert_eq!(last_message_preview(&conv), "AAA BBB");
    }

    #[test]
    fn evict_idle_drops_only_stale_sessions() {
        let mut mgr = manager();
        let fresh = mgr
            .create_session("m1", "p", None, None, None, None, None)
            .unwrap();
        let stale = mgr
            .create_session("m1", "p", None, None, None, None, None)
            .unwrap();

        // Age the second session beyond the timeout, then evict.
        {
            let stale_id = stale.id.clone();
            let active = mgr.sessions.get_mut(&stale_id).unwrap();
            active.last_active_at = Some(Utc::now() - chrono::Duration::hours(2));
        }
        let evicted = mgr.evict_idle(std::time::Duration::from_secs(60 * 60));
        assert_eq!(evicted, vec![stale.id.clone()]);
        assert!(mgr.get_chat_state(&stale.id).is_ok(), "reloaded on demand");
        assert!(mgr.get_chat_state(&fresh.id).is_ok());
    }

    #[test]
    fn streaming_sessions_never_evicted() {
        let mut mgr = manager();
        let s = mgr
            .create_session("m1", "p", None, None, None, None, None)
            .unwrap();
        {
            let sid = s.id.clone();
            let active = mgr.sessions.get_mut(&sid).unwrap();
            active.last_active_at = Some(Utc::now() - chrono::Duration::hours(2));
            active.session.is_streaming = true;
        }
        let evicted = mgr.evict_idle(std::time::Duration::from_secs(60));
        assert!(evicted.is_empty(), "streaming session must survive");
        assert!(mgr.get_chat_state(&s.id).is_ok());
    }

    #[test]
    fn create_session_stores_provided_context_window_and_rebuilds_with_it() {
        let mut mgr = manager();
        let s = mgr
            .create_session("m1", "p", None, None, None, Some(200_000), None)
            .unwrap();
        assert_eq!(s.context_window, 200_000);
        assert_eq!(mgr.context_window(&s.id).unwrap(), 200_000);

        // Evict → rebuild from the database must reuse the persisted window
        // instead of the built-in catalog default.
        {
            let sid = s.id.clone();
            let active = mgr.sessions.get_mut(&sid).unwrap();
            active.last_active_at = Some(Utc::now() - chrono::Duration::hours(2));
        }
        mgr.evict_idle(std::time::Duration::from_secs(60));
        assert_eq!(mgr.context_window(&s.id).unwrap(), 200_000);
    }

    #[test]
    fn persist_messages_incremental_append_then_full_rewrite() {
        let mut mgr = manager();
        let s = mgr
            .create_session("m1", "p", None, None, None, None, None)
            .unwrap();
        let sid = s.id.clone();

        // Turn 1: two messages persisted via the first full rewrite.
        {
            let cs = mgr.get_chat_state(&sid).unwrap();
            cs.push_user_message("first question");
            cs.push_assistant_message("first answer", vec![], None, None);
        }
        mgr.persist_messages(&sid).unwrap();
        assert_eq!(mgr.db.load_messages(&sid).unwrap().len(), 2);

        // Turn 2: only the new tail is appended.
        {
            let cs = mgr.get_chat_state(&sid).unwrap();
            cs.push_user_message("second question");
            cs.push_assistant_message("second answer", vec![], None, None);
        }
        mgr.persist_messages(&sid).unwrap();
        let loaded = mgr.db.load_messages(&sid).unwrap();
        assert_eq!(loaded.len(), 4, "incremental append must keep all rows");

        // Structural rewrite (truncate) resets the checkpoint → full rewrite.
        {
            let cs = mgr.get_chat_state(&sid).unwrap();
            cs.truncate_to_prompt_index(1);
        }
        mgr.persist_messages(&sid).unwrap();
        let loaded = mgr.db.load_messages(&sid).unwrap();
        assert_eq!(
            loaded.len(),
            2,
            "truncated history must be rewritten, not appended onto"
        );

        // Compaction-style replacement also rewrites.
        {
            let cs = mgr.get_chat_state(&sid).unwrap();
            let mut compacted = vec![crate::core::types::ConversationItem::system("summary")];
            compacted.extend(cs.conversation.iter().cloned());
            cs.replace_conversation(compacted);
        }
        mgr.persist_messages(&sid).unwrap();
        let loaded = mgr.db.load_messages(&sid).unwrap();
        assert_eq!(
            loaded.len(),
            3,
            "replaced history must include the summary row"
        );
        assert!(
            matches!(&loaded[0], crate::core::types::ConversationItem::System(s)
                if s.content == "summary")
        );
    }

    #[test]
    fn reloaded_session_checkpoint_matches_db_rows() {
        // A session rebuilt from the database must not rewrite history on
        // the first persist (checkpoint = DB row count), and must append
        // exactly the new tail afterwards.
        let mut mgr = manager();
        let s = mgr
            .create_session("m1", "p", None, None, None, None, None)
            .unwrap();
        let sid = s.id.clone();
        {
            let cs = mgr.get_chat_state(&sid).unwrap();
            cs.push_user_message("q");
            cs.push_assistant_message("a", vec![], None, None);
        }
        mgr.persist_messages(&sid).unwrap();

        // Force a rebuild from the database (idle-eviction equivalent).
        {
            let active = mgr.sessions.get_mut(&sid).unwrap();
            active.chat_state = None;
            active.session.status = SessionStatus::Idle;
        }
        mgr.get_chat_state(&sid).unwrap(); // reloads
        {
            let cs = mgr.get_chat_state(&sid).unwrap();
            cs.push_user_message("q2");
            cs.push_assistant_message("a2", vec![], None, None);
        }
        mgr.persist_messages(&sid).unwrap();
        let loaded = mgr.db.load_messages(&sid).unwrap();
        assert_eq!(
            loaded.len(),
            4,
            "reloaded checkpoint must not duplicate rows"
        );
    }

    #[test]
    fn evicted_session_status_is_idle_until_woken() {
        let mut mgr = manager();
        let s = mgr
            .create_session("m1", "p", None, None, None, None, None)
            .unwrap();
        {
            let sid = s.id.clone();
            let active = mgr.sessions.get_mut(&sid).unwrap();
            active.last_active_at = Some(Utc::now() - chrono::Duration::hours(2));
        }
        let _ = mgr.evict_idle(std::time::Duration::from_secs(60 * 60));
        let persisted = mgr.db.get_session(&s.id).unwrap().unwrap();
        assert!(matches!(persisted.status, SessionStatus::Idle));
        // Touching the session wakes it up and marks it active again.
        mgr.get_chat_state(&s.id).unwrap();
        let persisted = mgr.db.get_session(&s.id).unwrap().unwrap();
        assert!(matches!(persisted.status, SessionStatus::Active));
    }
}
