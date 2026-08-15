use rusqlite::{params, Connection};
use std::sync::Arc;

use crate::core::error::{AppError, AppResult};
use crate::core::types;
use chrono::Utc;

use super::Database;

/// Map a conversation item to its message-row columns. Shared by
/// [`Database::append_message`] and [`Database::replace_messages`].
#[allow(clippy::type_complexity)]
fn message_fields(
    item: &types::ConversationItem,
) -> AppResult<(
    &str,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<types::TokenUsage>,
    Option<String>,
)> {
    match item {
        types::ConversationItem::System(s) => Ok((
            "system",
            s.content.clone(),
            None,
            None,
            None,
            0,
            None,
            None,
            None,
        )),
        types::ConversationItem::User(u) => {
            let content = serde_json::to_string(&u.content)?;
            Ok(("user", content, None, None, None, 0, None, None, None))
        }
        types::ConversationItem::Assistant(a) => {
            let tool_calls = if a.tool_calls.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&a.tool_calls)?)
            };
            Ok((
                "assistant",
                a.content.clone(),
                None,
                None,
                tool_calls,
                0,
                a.model.clone(),
                a.usage.clone(),
                a.reasoning_content.clone(),
            ))
        }
        types::ConversationItem::ToolResult(tr) => Ok((
            "tool_result",
            tr.content.clone(),
            Some(tr.tool_call_id.clone()),
            None,
            None,
            if tr.is_error { 1 } else { 0 },
            None,
            None,
            None,
        )),
        types::ConversationItem::Reasoning(r) => Ok((
            "reasoning",
            r.content.clone(),
            None,
            None,
            None,
            0,
            None,
            None,
            None,
        )),
    }
}

/// All mutable message columns for an INSERT, matched positionally.
type MessageInsertFields<'a> = (
    &'a str,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<types::TokenUsage>,
    Option<String>,
);

/// Execute a message INSERT with the given column values.
fn insert_message(
    conn: &Connection,
    session_id: &str,
    order: i64,
    fields: &MessageInsertFields<'_>,
) -> rusqlite::Result<()> {
    let (
        role,
        content,
        tool_call_id,
        tool_name,
        tool_calls_json,
        is_error,
        model,
        usage,
        reasoning_content,
    ) = fields;
    conn.execute(
        r#"INSERT INTO messages
           (session_id, role, content, tool_call_id, tool_name, tool_calls,
            is_error, model, prompt_tokens, completion_tokens, cached_read_tokens,
            reasoning_tokens, prompt_cache_hit_tokens, prompt_cache_miss_tokens,
            created_at, conversation_order, reasoning_content)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
        params![
            session_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_calls_json,
            is_error,
            model,
            usage.as_ref().map(|u| u.prompt_tokens as i64),
            usage.as_ref().map(|u| u.completion_tokens as i64),
            usage
                .as_ref()
                .and_then(|u| u.cached_read_tokens.map(|v| v as i64)),
            usage
                .as_ref()
                .and_then(|u| u.reasoning_tokens.map(|v| v as i64)),
            // deepseek-native: KV cache tokens
            usage
                .as_ref()
                .and_then(|u| u.prompt_cache_hit_tokens.map(|v| v as i64)),
            usage
                .as_ref()
                .and_then(|u| u.prompt_cache_miss_tokens.map(|v| v as i64)),
            Utc::now().to_rfc3339(),
            order,
            reasoning_content,
        ],
    )?;
    Ok(())
}

impl Database {
    /// Persist a session's full conversation in ONE transaction: delete all
    /// existing rows, then insert the items in order. A failure rolls back,
    /// so the previous history survives a mid-write failure (the old
    /// clear-then-append left a half-deleted conversation).
    ///
    /// Used for the FIRST persist of a session and after structural rewrites
    /// (compaction, rewind, repair); steady-state persists go through
    /// [`Self::append_messages`] which only writes the new tail.
    pub fn replace_messages(
        &self,
        session_id: &str,
        items: &[types::ConversationItem],
    ) -> AppResult<()> {
        let conn = self.conn.lock()?;
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| -> AppResult<()> {
            conn.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params![session_id],
            )?;
            for (order, item) in items.iter().enumerate() {
                let fields = message_fields(item)?;
                insert_message(&conn, session_id, order as i64, &fields)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Append new messages to a session's history in ONE transaction,
    /// continuing `conversation_order` from `start_order`.
    ///
    /// The steady-state persist path: each turn rewrites only its own
    /// additions instead of the O(n) full rewrite that previously ran per
    /// turn (delete-all + re-insert), which scaled the write lock linearly
    /// with conversation length.
    pub fn append_messages(
        &self,
        session_id: &str,
        start_order: i64,
        items: &[types::ConversationItem],
    ) -> AppResult<()> {
        let conn = self.conn.lock()?;
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| -> AppResult<()> {
            for (offset, item) in items.iter().enumerate() {
                let fields = message_fields(item)?;
                insert_message(&conn, session_id, start_order + offset as i64, &fields)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Load all messages for a session, ordered by conversation_order.
    pub fn load_messages(&self, session_id: &str) -> AppResult<Vec<types::ConversationItem>> {
        let conn = self.conn.lock()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM messages WHERE session_id = ?1 ORDER BY conversation_order ASC",
        )?;

        let items = stmt
            .query_map(params![session_id], |row| {
                let role: String = row.get("role")?;
                let content: String = row.get("content")?;
                let is_error: i64 = row.get("is_error")?;

                let item = match role.as_str() {
                    "system" => types::ConversationItem::system(content),
                    "user" => {
                        let parts: Vec<types::ContentPart> = serde_json::from_str(&content)
                            .unwrap_or_else(|_| vec![types::ContentPart::Text { text: content }]);
                        types::ConversationItem::user_with_parts(parts)
                    }
                    "assistant" => {
                        let tool_calls_json: Option<String> = row.get("tool_calls")?;
                        let tool_calls = tool_calls_json
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default();
                        let model: Option<String> = row.get("model")?;
                        let reasoning_content: Option<String> =
                            row.get("reasoning_content").ok().flatten();
                        types::ConversationItem::Assistant(types::AssistantMessage {
                            content,
                            tool_calls,
                            model,
                            usage: Some(types::TokenUsage {
                                prompt_tokens: row
                                    .get::<_, Option<i64>>("prompt_tokens")?
                                    .unwrap_or(0)
                                    as u64,
                                completion_tokens: row
                                    .get::<_, Option<i64>>("completion_tokens")?
                                    .unwrap_or(0)
                                    as u64,
                                cached_read_tokens: row
                                    .get::<_, Option<i64>>("cached_read_tokens")?
                                    .map(|v| v as u64),
                                reasoning_tokens: row
                                    .get::<_, Option<i64>>("reasoning_tokens")?
                                    .map(|v| v as u64),
                                prompt_cache_hit_tokens: row
                                    .get::<_, Option<i64>>("prompt_cache_hit_tokens")?
                                    .map(|v| v as u64),
                                prompt_cache_miss_tokens: row
                                    .get::<_, Option<i64>>("prompt_cache_miss_tokens")?
                                    .map(|v| v as u64),
                            }),
                            reasoning_content,
                        })
                    }
                    "tool_result" => {
                        let tool_call_id: String = row.get("tool_call_id")?;
                        if is_error != 0 {
                            types::ConversationItem::tool_result_error(tool_call_id, content)
                        } else {
                            types::ConversationItem::tool_result(tool_call_id, content)
                        }
                    }
                    "reasoning" => types::ConversationItem::Reasoning(types::ReasoningMessage {
                        content,
                        encrypted_content: None,
                    }),
                    _ => types::ConversationItem::system(content),
                };
                Ok(item)
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(items)
    }

    /// Async wrapper around [`Self::load_messages`] — a full-conversation
    /// reload deserializes every row (thousands on long sessions) and must
    /// not block a tokio worker.
    pub async fn load_messages_async(
        self: &Arc<Self>,
        session_id: String,
    ) -> AppResult<Vec<types::ConversationItem>> {
        let db = self.clone();
        let sid = session_id.clone();
        tokio::task::spawn_blocking(move || db.load_messages(&sid))
            .await
            .map_err(|e| AppError::Internal(format!("load_messages task failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {

    use crate::core::types::ConversationItem;
    use crate::storage::database::Database;

    fn fresh_db() -> (std::sync::Arc<Database>, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("ddc-msg-test-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        let db = Database::open(&path, true).unwrap();
        db.run_migrations().unwrap();
        (std::sync::Arc::new(db), path)
    }

    fn history(items: usize) -> Vec<ConversationItem> {
        (0..items)
            .map(|i| ConversationItem::user(format!("msg-{i}")))
            .collect()
    }

    /// Create a session row (messages.session_id has a foreign key onto it).
    fn seed_session(db: &Database) -> String {
        let session = crate::core::types::Session::new("m1", "p");
        db.upsert_session(&session).unwrap();
        session.id
    }

    #[test]
    fn append_continues_orders_after_existing_rows() {
        let (db, _) = fresh_db();
        let sid = seed_session(&db);
        db.replace_messages(&sid, &history(3)).unwrap();
        db.append_messages(&sid, 3, &history(2)).unwrap();

        let loaded = db.load_messages(&sid).unwrap();
        assert_eq!(loaded.len(), 5);
        assert!(matches!(&loaded[3], ConversationItem::User(u)
            if !u.content.is_empty()));
    }

    #[test]
    fn append_orders_are_contiguous() {
        let (db, _) = fresh_db();
        let sid = seed_session(&db);
        db.replace_messages(&sid, &history(2)).unwrap();
        db.append_messages(&sid, 2, &history(2)).unwrap();

        let conn = db.conn().unwrap();
        let orders: Vec<i64> = conn
            .prepare("SELECT conversation_order FROM messages WHERE session_id = ?1 ORDER BY conversation_order")
            .unwrap()
            .query_map([&sid], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(orders, vec![0, 1, 2, 3]);
    }

    #[test]
    fn append_matches_replace_roundtrip() {
        let (db, _) = fresh_db();
        let sid = seed_session(&db);
        // append-only persistence must reproduce the same rows as a full
        // rewrite of the same conversation.
        let full = history(4);
        db.replace_messages(&sid, &full).unwrap();
        let replaced = db.load_messages(&sid).unwrap();

        db.replace_messages(&sid, &[]).unwrap();
        db.append_messages(&sid, 0, &full).unwrap();
        let appended = db.load_messages(&sid).unwrap();

        assert_eq!(appended.len(), replaced.len());
        for (a, b) in appended.iter().zip(replaced.iter()) {
            assert_eq!(format!("{a:?}"), format!("{b:?}"));
        }
    }

    #[test]
    fn replace_clears_previous_history() {
        let (db, _) = fresh_db();
        let sid = seed_session(&db);
        db.replace_messages(&sid, &history(5)).unwrap();
        db.replace_messages(&sid, &history(1)).unwrap();
        assert_eq!(db.load_messages(&sid).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn async_load_offload_matches_sync() {
        // The offloaded wrapper must return exactly what the sync path does.
        let (db, _) = fresh_db();
        let sid = seed_session(&db);
        db.replace_messages(&sid, &history(5)).unwrap();
        let sync = db.load_messages(&sid).unwrap();
        let offloaded = db.load_messages_async(sid.clone()).await.unwrap();
        assert_eq!(offloaded.len(), sync.len());
        for (a, b) in offloaded.iter().zip(sync.iter()) {
            assert_eq!(format!("{a:?}"), format!("{b:?}"));
        }
    }
}
