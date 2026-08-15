//! Memory store — persistent storage with FTS5 full-text search.
//!
//! Uses SQLite's FTS5 virtual table for efficient keyword search.
//! Each memory has:
//! - content (the text)
//! - metadata (JSON: source, tags, etc.)
//! - category (project, preference, fact, etc.)
//! - session_id (which session created it)
//! - access_count / last_accessed (for relevance scoring)

use crate::core::error::AppResult;
use crate::storage::database::Database;
use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// A stored memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub metadata: Value,
    pub category: String,
    pub session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub access_count: u64,
    pub last_accessed: Option<String>,
    /// Relevance decay factor (1.0 = full relevance; dream decay lowers it).
    /// Applied as a multiplier in search scoring.
    pub decay_factor: Option<f32>,
}

/// The memory store — wraps the database for memory operations.
pub struct MemoryStore {
    db: Arc<Database>,
}

impl MemoryStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Store a new memory.
    pub fn store(
        &self,
        content: &str,
        category: &str,
        session_id: Option<&str>,
        metadata: Option<Value>,
    ) -> AppResult<i64> {
        let now = Utc::now().to_rfc3339();
        let metadata_str = serde_json::to_string(&metadata.unwrap_or(json!({})))?;

        let mut conn = self.db.conn()?;

        // The memory row and its FTS row commit atomically — a failure
        // between the two inserts must not leave an orphaned FTS row
        // (invisible to search) or a memory row without an index.
        let tx = conn.transaction()?;
        tx.execute(
            r#"INSERT INTO memory (content, metadata, category, session_id, created_at, updated_at, access_count, last_accessed)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL)"#,
            rusqlite::params![content, metadata_str, category, session_id, now, now],
        )?;

        let id = tx.last_insert_rowid();

        // Insert into FTS table. Indexed text is CJK-bigram-spaced so the
        // unicode61 tokenizer can match Chinese terms (see cjk_bigram_spacing).
        let fts_content = cjk_bigram_spacing(content);
        let fts_metadata = cjk_bigram_spacing(&metadata_str);
        tx.execute(
            r#"INSERT INTO memory_fts (rowid, content, metadata, category, session_id, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            rusqlite::params![id, fts_content, fts_metadata, category, session_id, now],
        )?;
        tx.commit()?;

        tracing::debug!(id, category, "Memory stored");
        Ok(id)
    }

    /// Search memories using FTS5.
    /// Returns memories paired with their raw BM25 score from SQLite.
    pub fn search(&self, query: &str, limit: u32) -> AppResult<Vec<(Memory, f64)>> {
        let conn = self.db.conn()?;

        // Sanitize the raw query into a syntactically safe FTS5 MATCH
        // expression. User/agent text routinely contains backticks, quotes,
        // parens, operators (`Start-Sleep -Seconds 45; Write-Output ...`) —
        // feeding it to MATCH verbatim raises "fts5: syntax error". Each
        // whitespace-separated token becomes a double-quoted phrase; a
        // token with zero valid terms means "nothing to match" → empty result.
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            r#"SELECT m.id, m.content, m.metadata, m.category, m.session_id,
                      m.created_at, m.updated_at, m.access_count, m.last_accessed,
                      m.decay_factor,
                      bm25(memory_fts) as score
               FROM memory_fts
               JOIN memory m ON m.id = memory_fts.rowid
               WHERE memory_fts MATCH ?1
               ORDER BY score
               LIMIT ?2"#,
        )?;

        let results = stmt
            .query_map(rusqlite::params![fts_query, limit as i64], |row| {
                let mem = Memory {
                    id: row.get("id")?,
                    content: row.get("content")?,
                    metadata: serde_json::from_str(&row.get::<_, String>("metadata")?)
                        .unwrap_or(json!({})),
                    category: row.get("category")?,
                    session_id: row.get("session_id")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    access_count: row.get::<_, i64>("access_count")? as u64,
                    last_accessed: row.get("last_accessed")?,
                    decay_factor: row.get("decay_factor").ok(),
                };
                let bm25_score: f64 = row.get("score")?;
                Ok((mem, bm25_score))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Update access counts on the SAME connection we already hold.
        // Calling self.record_access() here would re-lock the database mutex
        // (std::sync::Mutex is not reentrant) and deadlock the search.
        let ids: Vec<i64> = results.iter().map(|(mem, _)| mem.id).collect();
        record_access_with_conn(&conn, &ids)?;

        Ok(results)
    }

    /// Search global (session-independent) memories only.
    ///
    /// Used by the searcher's evergreen supplement: long sessions produce many
    /// high-recency session memories that can crowd durable knowledge out of
    /// the top-N. This query guarantees durable (global) memories are still
    /// found even when session logs dominate the keyword ranking.
    pub fn search_global(&self, query: &str, limit: u32) -> AppResult<Vec<(Memory, f64)>> {
        let conn = self.db.conn()?;

        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            r#"SELECT m.id, m.content, m.metadata, m.category, m.session_id,
                      m.created_at, m.updated_at, m.access_count, m.last_accessed,
                      m.decay_factor,
                      bm25(memory_fts) as score
               FROM memory_fts
               JOIN memory m ON m.id = memory_fts.rowid
               WHERE memory_fts MATCH ?1 AND m.session_id IS NULL
               ORDER BY score
               LIMIT ?2"#,
        )?;

        let results = stmt
            .query_map(rusqlite::params![fts_query, limit as i64], |row| {
                let mem = Memory {
                    id: row.get("id")?,
                    content: row.get("content")?,
                    metadata: serde_json::from_str(&row.get::<_, String>("metadata")?)
                        .unwrap_or(json!({})),
                    category: row.get("category")?,
                    session_id: row.get("session_id")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    access_count: row.get::<_, i64>("access_count")? as u64,
                    last_accessed: row.get("last_accessed")?,
                    decay_factor: row.get("decay_factor").ok(),
                };
                let bm25_score: f64 = row.get("score")?;
                Ok((mem, bm25_score))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Record access on the same connection (see search() reentrancy note).
        let ids: Vec<i64> = results.iter().map(|(mem, _)| mem.id).collect();
        record_access_with_conn(&conn, &ids)?;

        Ok(results)
    }

    /// Search by category.
    pub fn search_by_category(&self, category: &str, limit: u32) -> AppResult<Vec<Memory>> {
        let conn = self.db.conn()?;

        let mut stmt = conn.prepare(
            r#"SELECT * FROM memory WHERE category = ?1
               ORDER BY access_count DESC, created_at DESC
               LIMIT ?2"#,
        )?;

        let memories = stmt
            .query_map(rusqlite::params![category, limit as i64], |row| {
                Ok(Memory {
                    id: row.get("id")?,
                    content: row.get("content")?,
                    metadata: serde_json::from_str(&row.get::<_, String>("metadata")?)
                        .unwrap_or(json!({})),
                    category: row.get("category")?,
                    session_id: row.get("session_id")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    access_count: row.get::<_, i64>("access_count")? as u64,
                    last_accessed: row.get("last_accessed")?,
                    decay_factor: row.get("decay_factor").ok(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(memories)
    }

    /// Search by category prefix (e.g. all `synthesized_*` memories).
    pub fn search_by_category_prefix(&self, prefix: &str, limit: u32) -> AppResult<Vec<Memory>> {
        let conn = self.db.conn()?;

        let mut stmt = conn.prepare(
            r#"SELECT * FROM memory WHERE category LIKE ?1
               ORDER BY access_count DESC, created_at DESC
               LIMIT ?2"#,
        )?;

        let pattern = format!("{}%", prefix);
        let memories = stmt
            .query_map(rusqlite::params![pattern, limit as i64], |row| {
                Ok(Memory {
                    id: row.get("id")?,
                    content: row.get("content")?,
                    metadata: serde_json::from_str(&row.get::<_, String>("metadata")?)
                        .unwrap_or(json!({})),
                    category: row.get("category")?,
                    session_id: row.get("session_id")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    access_count: row.get::<_, i64>("access_count")? as u64,
                    last_accessed: row.get("last_accessed")?,
                    decay_factor: row.get("decay_factor").ok(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(memories)
    }

    /// Delete a memory by ID.
    pub fn delete(&self, id: i64) -> AppResult<()> {
        let mut conn = self.db.conn()?;
        // Both deletes commit atomically — a failure between them must not
        // leave an orphaned FTS row (search would return a missing memory).
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM memory WHERE id = ?1", rusqlite::params![id])?;
        tx.execute(
            "DELETE FROM memory_fts WHERE rowid = ?1",
            rusqlite::params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Get all memories (for management UI).
    pub fn list_all(&self, limit: u32) -> AppResult<Vec<Memory>> {
        let conn = self.db.conn()?;

        let mut stmt = conn.prepare("SELECT * FROM memory ORDER BY created_at DESC LIMIT ?1")?;

        let memories = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(Memory {
                    id: row.get("id")?,
                    content: row.get("content")?,
                    metadata: serde_json::from_str(&row.get::<_, String>("metadata")?)
                        .unwrap_or(json!({})),
                    category: row.get("category")?,
                    session_id: row.get("session_id")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    access_count: row.get::<_, i64>("access_count")? as u64,
                    last_accessed: row.get("last_accessed")?,
                    decay_factor: row.get("decay_factor").ok(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(memories)
    }

    /// Get memory count.
    pub fn count(&self) -> AppResult<u64> {
        let conn = self.db.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memory", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Store an embedding for a memory.
    pub fn store_embedding(&self, memory_id: i64, embedding: &[f32]) -> AppResult<()> {
        let conn = self.db.conn()?;
        // Serialize embedding as a byte blob (little-endian f32)
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        conn.execute(
            "UPDATE memory SET embedding = ?1 WHERE id = ?2",
            rusqlite::params![bytes, memory_id],
        )?;
        Ok(())
    }

    /// Get the embedding for a memory (if stored).
    pub fn get_embedding(&self, memory_id: i64) -> AppResult<Option<Vec<f32>>> {
        let conn = self.db.conn()?;
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM memory WHERE id = ?1",
                rusqlite::params![memory_id],
                |row| row.get(0),
            )
            .optional()?;

        Ok(blob.map(|bytes| {
            bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()
        }))
    }

    /// Get all memories that have embeddings.
    pub fn list_with_embeddings(&self, limit: u32) -> AppResult<Vec<(i64, Vec<f32>, Memory)>> {
        let conn = self.db.conn()?;
        let mut stmt = conn.prepare(
            r#"SELECT id, embedding, content, metadata, category, session_id,
                      created_at, updated_at, access_count, last_accessed, decay_factor
               FROM memory WHERE embedding IS NOT NULL
               ORDER BY created_at DESC LIMIT ?1"#,
        )?;

        let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
            let blob: Vec<u8> = row.get(1)?;
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            let memory = Memory {
                id: row.get(0)?,
                content: row.get(2)?,
                metadata: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(json!({})),
                category: row.get(4)?,
                session_id: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                access_count: row.get::<_, i64>(8)? as u64,
                last_accessed: row.get(9)?,
                decay_factor: row.get(10).ok(),
            };

            Ok((memory.id, embedding, memory))
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Async wrapper around [`Self::search`] — the FTS5 MATCH scan plus
    /// per-row access updates block the caller; offload off the tokio
    /// worker.
    pub async fn search_async(
        self: &Arc<Self>,
        query: String,
        limit: u32,
    ) -> AppResult<Vec<(Memory, f64)>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.search(&query, limit))
            .await
            .map_err(|e| {
                crate::core::error::AppError::Internal(format!("memory search task failed: {e}"))
            })?
    }

    /// Async wrapper around [`Self::list_with_embeddings`] — a FULL scan of
    /// every embedded memory; must not block a tokio worker.
    pub async fn list_with_embeddings_async(
        self: &Arc<Self>,
        limit: u32,
    ) -> AppResult<Vec<(i64, Vec<f32>, Memory)>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.list_with_embeddings(limit))
            .await
            .map_err(|e| {
                crate::core::error::AppError::Internal(format!(
                    "memory embedding scan task failed: {e}"
                ))
            })?
    }

    /// Apply a decay factor to a memory (reduces its relevance).
    pub fn decay(&self, memory_id: i64, factor: f32) -> AppResult<()> {
        let conn = self.db.conn()?;
        conn.execute(
            "UPDATE memory SET decay_factor = decay_factor * ?1 WHERE id = ?2",
            rusqlite::params![factor, memory_id],
        )?;
        Ok(())
    }

    /// Supersede memories that describe the SAME fact as `new_embedding`.
    /// When a tool writes a correction ("index is now a dark glass DeepDepCat
    /// landing page"), the older contradictory memory ("index is a light
    /// DeepSeek clone") must not keep ranking alongside it — find
    /// semantically-near memories by cosine similarity and decay them so the
    /// fresh fact wins. Returns the number of superseded memories.
    pub fn supersede_similar(&self, new_id: i64, new_embedding: &[f32]) -> AppResult<usize> {
        const SIMILARITY_THRESHOLD: f32 = 0.88;
        const SUPERSEDED_DECAY: f32 = 0.05;

        let candidates = self.list_with_embeddings(5000)?;
        let mut superseded = 0;
        for (id, emb, _mem) in candidates {
            if id == new_id {
                continue;
            }
            let sim = crate::memory::embedding::EmbeddingProvider::cosine_similarity(
                new_embedding,
                &emb,
            );
            if sim >= SIMILARITY_THRESHOLD {
                self.decay(id, SUPERSEDED_DECAY)?;
                superseded += 1;
            }
        }
        Ok(superseded)
    }
}

/// Build a syntactically safe FTS5 MATCH expression from arbitrary user/
/// agent text. Every whitespace-separated token becomes a double-quoted
/// phrase (inner quotes escaped by doubling), joined with OR — backticks,
/// parens, `-`, `;`, operators etc. can no longer break the query grammar.
/// CJK runs are bigram-spaced so partial Chinese terms match indexed text
/// ("技术" hits "技术 术栈"). Returns an empty string when there is nothing
/// safe to match.
fn sanitize_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|t| cjk_bigram_spacing(t).replace('"', "\"\""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// True for CJK ideographs, kana, and hangul (the ranges unicode61 can't
/// split on whitespace alone).
fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'     // CJK Extension A
        | '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}'   // CJK Compatibility Ideographs
        | '\u{20000}'..='\u{2A6DF}' // CJK Extension B
        | '\u{3040}'..='\u{30FF}'   // Hiragana + Katakana
        | '\u{AC00}'..='\u{D7AF}') // Hangul syllables
}

/// Insert a space between every adjacent pair of CJK characters, turning
/// an unbroken Chinese sentence into 2-char sliding windows that the
/// unicode61 tokenizer can index: "技术栈" → "技术 术栈". Latin/digit runs
/// are preserved verbatim. Applied symmetrically to indexed text and to
/// query terms, so both full-word and partial-term searches hit.
fn cjk_bigram_spacing(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut prev_cjk = false;
    for ch in text.chars() {
        let is_cjk = is_cjk_char(ch);
        if is_cjk && prev_cjk {
            out.push(' ');
        }
        out.push(ch);
        prev_cjk = is_cjk;
    }
    out
}

/// Apply an access-count bump on an ALREADY-LOCKED connection.
///
/// Never calls `self.db.conn()` — callers must hold the connection (or lock
/// it themselves via [`MemoryStore::record_access`]). Using this from code
/// that already holds `db.conn()` avoids re-locking the non-reentrant
/// `std::sync::Mutex`, which would deadlock.
fn record_access_with_conn(conn: &rusqlite::Connection, ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    let mut stmt = conn.prepare(
        "UPDATE memory SET access_count = access_count + 1, last_accessed = ?1 WHERE id = ?2",
    )?;
    for id in ids {
        stmt.execute(rusqlite::params![now, id])?;
    }
    Ok(())
}

use serde_json::json;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ids::generate_id;

    #[test]
    fn search_with_record_access_does_not_deadlock() {
        let dir = std::env::temp_dir().join(format!("ddc-store-test-{}", generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("t.db"), true).unwrap());
        db.run_migrations().unwrap();
        let store = MemoryStore::new(db);

        for i in 0..5 {
            store
                .store(
                    &format!("memory number {i} about project planning"),
                    "test",
                    None,
                    None,
                )
                .unwrap();
        }
        eprintln!("[step1] seeded 5 memories");

        let results = store.search("project planning", 10).unwrap();
        eprintln!("[step2] search ok, {} results", results.len());
        assert!(!results.is_empty(), "search must return results");
        assert_eq!(results.len(), 5);

        let results2 = store.search("planning", 10).unwrap();
        eprintln!("[step3] second search ok, {} results", results2.len());
        assert_eq!(results2.len(), 5);
    }

    #[test]
    fn supersede_similar_decays_semantically_near_memories() {
        let dir = std::env::temp_dir().join(format!("ddc-store-supersede-{}", generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("t.db"), true).unwrap());
        db.run_migrations().unwrap();
        let store = MemoryStore::new(db);

        // Two memories describing the SAME fact (the index page) at different
        // points in time — the fresh one must supersede the stale one.
        let old_id = store
            .store(
                "index page is a single-file light DeepSeek clone",
                "project",
                None,
                None,
            )
            .unwrap();
        let new_id = store
            .store(
                "index page is a multi-file dark DeepDepCat landing page",
                "project",
                None,
                None,
            )
            .unwrap();

        // Identical embeddings stand in for "semantically near" (cosine of
        // identical vectors is exactly 1.0).
        let emb = vec![0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        store.store_embedding(old_id, &emb).unwrap();
        store.store_embedding(new_id, &emb).unwrap();

        let superseded = store.supersede_similar(new_id, &emb).unwrap();
        assert_eq!(superseded, 1, "exactly the old memory should be superseded");

        let all = store.list_with_embeddings(10).unwrap();
        let old = all.iter().find(|(id, _, _)| *id == old_id).unwrap();
        assert!(
            old.2.decay_factor.unwrap_or(1.0) < 0.1,
            "old memory must be decayed"
        );
    }

    #[test]
    fn search_global_only_returns_session_independent_memories() {
        let dir = std::env::temp_dir().join(format!("ddc-store-global-{}", generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("t.db"), true).unwrap());
        db.run_migrations().unwrap();
        // Create a real session row so the session-scoped memory's FK passes.
        db.conn()
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, title, model, provider, created_at, updated_at) \
                 VALUES ('session-abc', 't', 'm', 'p', 'now', 'now')",
                [],
            )
            .unwrap();
        let store = MemoryStore::new(db);

        // A durable (global) memory and a session-scoped one with the same term.
        store
            .store("project uses Rust and Tauri", "fact", None, None)
            .unwrap();
        store
            .store(
                "project uses Rust for the backend",
                "session-note",
                Some("session-abc"),
                None,
            )
            .unwrap();

        let global = store.search_global("project uses Rust", 10).unwrap();
        assert_eq!(
            global.len(),
            1,
            "search_global must only return global memories"
        );
        assert!(
            global.iter().all(|(m, _)| m.session_id.is_none()),
            "no session-scoped memory may leak into search_global"
        );

        // The plain search sees both.
        let all = store.search("project uses Rust", 10).unwrap();
        assert_eq!(all.len(), 2, "plain search sees global + session memories");
    }

    #[test]
    fn plain_text_quoted() {
        assert_eq!(
            sanitize_fts_query("fix the bug"),
            "\"fix\" OR \"the\" OR \"bug\""
        );
    }

    #[test]
    fn backticks_and_parens_are_neutralized() {
        // The exact text that crashed production: a subagent task containing
        // a shell command in backticks.
        let q = sanitize_fts_query("Start-Sleep -Seconds 45; Write-Output 'worker-alive'");
        // Every token must be wrapped in a double-quoted phrase so special
        // FTS5 characters (backticks, `;`, `-`, parens) are literal.
        assert!(q.starts_with('"'));
        assert!(q.contains("\"Start-Sleep\""));
        assert!(q.contains("\"-Seconds\""));
        assert!(q.contains("\"45;\""));
        assert!(q.contains("\"'worker-alive'\""));
        // Balanced quotes only (no unquoted remainder).
        assert_eq!(q.matches('"').count() % 2, 0);
    }

    #[test]
    fn inner_quotes_escaped() {
        let q = sanitize_fts_query("say \"hello\"");
        assert!(q.contains("\"say\""));
        assert!(q.contains("\"\"hello\"\""));
    }

    #[test]
    fn empty_query_is_empty_result() {
        assert_eq!(sanitize_fts_query(""), "");
        assert_eq!(sanitize_fts_query("   "), "");
    }

    #[test]
    fn cjk_query_is_bigram_spaced() {
        assert_eq!(sanitize_fts_query("技术栈"), "\"技 术 栈\"");
        assert_eq!(sanitize_fts_query("技术"), "\"技 术\"");
        assert_eq!(
            sanitize_fts_query("帮我记住技术栈"),
            "\"帮 我 记 住 技 术 栈\""
        );
    }

    #[test]
    fn cjk_bigram_spacing_preserves_latin() {
        assert_eq!(
            cjk_bigram_spacing("GitHub Actions 技术栈"),
            "GitHub Actions 技 术 栈"
        );
        assert_eq!(cjk_bigram_spacing("rust"), "rust");
        assert_eq!(cjk_bigram_spacing("混合mix中文"), "混 合mix中 文");
    }

    #[test]
    fn cjk_search_finds_partial_word_after_bigram_migration() {
        let dir = std::env::temp_dir().join(format!("ddc-store-cjk-{}", generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(&dir.join("t.db"), true).unwrap());
        db.run_migrations().unwrap();
        let store = MemoryStore::new(db);

        store
            .store(
                "项目使用 Rust 和 Tauri 技术栈，部署到 Debian 服务器",
                "test",
                None,
                None,
            )
            .unwrap();
        store
            .store("每日构建脚本通过 GitHub Actions 运行", "test", None, None)
            .unwrap();

        // unicode61 would treat the whole sentence as one token and miss
        // this; bigram spacing must hit on a partial CJK term.
        let results = store.search("技术栈", 10).unwrap();
        assert!(
            results.iter().any(|(m, _)| m.content.contains("技术栈")),
            "CJK search for 技术栈 must hit the memory containing it"
        );

        let results = store.search("技术", 10).unwrap();
        assert!(
            results.iter().any(|(m, _)| m.content.contains("技术栈")),
            "partial CJK term 技术 must still hit 技术栈"
        );

        // Latin still works with exact phrase semantics.
        let results = store.search("GitHub Actions", 10).unwrap();
        assert!(
            results
                .iter()
                .any(|(m, _)| m.content.contains("GitHub Actions")),
            "latin search must still work after bigram migration"
        );
    }
}
