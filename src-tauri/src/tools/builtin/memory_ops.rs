//! Memory tools — search and store memories.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use crate::hooks::{HookContext, HookEvent};
use crate::memory::embedding::EmbeddingProvider;
use crate::memory::memory_file;
use crate::memory::search::MemorySearcher;
use crate::memory::store::MemoryStore;
use crate::toolkit::ToolScope;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;

// ── Memory Search Tool ────────────────────────────────────────────────────────

pub struct MemorySearchTool {
    searcher: Arc<MemorySearcher>,
}

impl MemorySearchTool {
    pub fn new(searcher: Arc<MemorySearcher>) -> Self {
        Self { searcher }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search through stored memories (notes, facts, and context from previous conversations). Returns relevant memories based on the search query."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "category": {
                    "type": "string",
                    "description": "If provided, returns memories in this category ONLY (e.g. 'project', 'preference', 'fact') — the query is ignored and every memory in the category is returned, recency-scored and capped by limit. Omit for semantic search on the query."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results. Defaults to 5."
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'query'".into()))?;
        let category = args.get("category").and_then(|c| c.as_str());
        let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(5) as usize;

        let results = if let Some(cat) = category {
            self.searcher.search_by_category(cat)?
        } else {
            self.searcher.search(query).await?
        };
        // MemorySearched hook — observability for memory retrieval.
        context
            .app
            .state::<AppState>()
            .hook_executor
            .execute_observe(
                &HookContext::new(HookEvent::MemorySearched, &context.session_id)
                    .with_data("query", json!(query)),
            )
            .await;

        let limited: Vec<_> = results.into_iter().take(limit).collect();

        if limited.is_empty() {
            return Ok(ToolResult::success(format!(
                "No memories found for query '{}'.",
                query
            )));
        }

        let mut output = String::new();
        for (i, result) in limited.iter().enumerate() {
            output.push_str(&format!(
                "{}. [score: {:.2}] {}\n",
                i + 1,
                result.score,
                result.memory.content.chars().take(200).collect::<String>()
            ));
        }

        Ok(ToolResult::success(output))
    }
}

// ── Memory Store Tool ──────────────────────────────────────────────────────────

pub struct MemoryStoreTool {
    store: Arc<MemoryStore>,
    embedding_provider: Arc<EmbeddingProvider>,
}

impl MemoryStoreTool {
    pub fn new(store: Arc<MemoryStore>, embedding_provider: Arc<EmbeddingProvider>) -> Self {
        Self {
            store,
            embedding_provider,
        }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store a memory (note, fact, or context) for future reference. Memories are persisted and can be searched later."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The memory content to store"
                },
                "category": {
                    "type": "string",
                    "description": "Category for the memory (e.g. 'project', 'preference', 'fact')"
                }
            },
            "required": ["content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'content'".into()))?;
        let category = args
            .get("category")
            .and_then(|c| c.as_str())
            .unwrap_or("general");

        let id = self
            .store
            .store(content, category, Some(&context.session_id), None)?;

        // Generate + persist the embedding so tool-written memories are
        // findable by vector search (parity with the store_memory command).
        // Non-fatal — keyword search still works without an embedding.
        match self.embedding_provider.embed(content).await {
            Ok(embedding) => {
                if let Err(e) = self.store.store_embedding(id, &embedding) {
                    tracing::warn!(memory_id = id, error = %e, "Failed to store embedding");
                } else if let Ok(superseded) = self.store.supersede_similar(id, &embedding) {
                    if superseded > 0 {
                        tracing::info!(
                            memory_id = id,
                            superseded,
                            "Memory superseded semantically-similar memories"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(memory_id = id, error = %e, "Failed to generate embedding");
            }
        }
        // MemoryStored hook — observability for memory writes.
        context
            .app
            .state::<AppState>()
            .hook_executor
            .execute_observe(
                &HookContext::new(HookEvent::MemoryStored, &context.session_id)
                    .with_data("memory_id", json!(id))
                    .with_data("category", json!(category)),
            )
            .await;

        Ok(ToolResult::success(format!(
            "Memory #{} stored in category '{}': {}",
            id,
            category,
            content.chars().take(100).collect::<String>()
        )))
    }
}

// ── Memory Learn Tool ─────────────────────────────────────────────────────

/// Extract NON-OBVIOUS learnings from the current session and persist them
/// into memory (`learning` category) + the workspace learnings file — the
/// self-evolution path, runnable on demand.
pub struct MemoryLearnTool;

impl MemoryLearnTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for MemoryLearnTool {
    fn name(&self) -> &str {
        "memory_learn"
    }

    fn description(&self) -> &str {
        "Extract non-obvious learnings from the current session (hidden \
         relationships, tool quirks, workarounds, build commands, \
         architectural decisions) and persist them into memory (category \
         learning) plus the workspace .deepdepcat/learnings.md — the \
         self-evolution path. Use after a turn that revealed something \
         worth remembering."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, _args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let state = context.app.state::<crate::bootstrap::AppState>();
        let learnings = crate::memory::learning::run_learning_pass(
            &state.llm_client,
            &context.model,
            context.provider.as_deref(),
            &context.conversation,
            &state.memory,
            context.workspace.as_deref(),
        )
        .await?;
        if learnings.is_empty() {
            return Ok(ToolResult::success(
                "本次会话没有值得沉淀的非显然经验。".to_string(),
            ));
        }
        let list = learnings
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ToolResult::success(format!(
            "已沉淀 {} 条学习经验（记忆库 + learnings.md）：\n{list}",
            learnings.len()
        )))
    }
}

// ── Memory Write Tool (dual-layer MEMORY.md) ────────────────────────────

/// Write a long-term note into the dual-layer MEMORY.md — the standing
/// memory injected into the system prompt every turn (project-level
/// `.deepdepcat/MEMORY.md` by default, user-level `~/.deepdepcat/MEMORY.md`
/// with `scope=user`). Unlike `memory_store` (searchable snippets), these
/// notes are ALWAYS in context and persist across sessions.
pub struct MemoryWriteTool;

impl MemoryWriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for MemoryWriteTool {
    fn scope(&self) -> ToolScope {
        ToolScope::All
    }

    fn name(&self) -> &str {
        "memory_write"
    }

    fn description(&self) -> &str {
        "Write a long-term memory note into the dual-layer MEMORY.md \
         (project .deepdepcat/MEMORY.md by default, or the user-level \
         ~/.deepdepcat/MEMORY.md with scope=\"user\"). Both files are \
         injected into the system prompt every turn, so these notes persist \
         across sessions and stay always in context — unlike memory_store, \
         which saves searchable snippets. The agent may only manage the \
         section between the <!-- managed:memory --> markers; the user's \
         own hand-written content is preserved. Use for stable project \
         facts, user preferences, architecture decisions — not session \
         trivia. Entries are deduplicated and capped."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The memory note (concise, one or two sentences; max 400 chars)."
                },
                "scope": {
                    "type": "string",
                    "enum": ["project", "user"],
                    "description": "Where to write: 'project' (default) writes .deepdepcat/MEMORY.md in the current workspace; 'user' writes the workspace-independent ~/.deepdepcat/MEMORY.md."
                }
            },
            "required": ["content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// File write — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'content'".into()))?;
        let normalized = memory_file::normalize_entry(content);
        if normalized.is_empty() {
            return Ok(ToolResult::error(
                "memory_write content is empty after trimming.".to_string(),
            ));
        }
        let scope = args
            .get("scope")
            .and_then(|s| s.as_str())
            .unwrap_or("project");
        let path = match scope {
            "project" => {
                let ws = context.workspace.as_ref().ok_or_else(|| {
                    AppError::Other(
                        "memory_write scope='project' needs a workspace — use scope='user' \
                         for workspace-independent notes"
                            .to_string(),
                    )
                })?;
                memory_file::project_memory_path(ws)
            }
            "user" => memory_file::user_memory_path(),
            other => {
                return Err(AppError::Parse(format!(
                    "Invalid scope '{other}' — use 'project' or 'user'"
                )));
            }
        };
        memory_file::write_memory_entry(&path, &normalized)?;
        Ok(ToolResult::success(format!(
            "MEMORY.md updated ({}): {}",
            path.display(),
            normalized.chars().take(120).collect::<String>()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_write_tool_shape_is_stable() {
        let tool = MemoryWriteTool::new();
        assert_eq!(tool.name(), "memory_write");
        assert!(!tool.is_read_only());
        assert!(!tool.is_concurrency_safe());
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert_eq!(required[0], "content");
        assert_eq!(params["properties"]["scope"]["enum"][0], "project");
        assert_eq!(params["properties"]["scope"]["enum"][1], "user");
        assert_eq!(tool.scope(), ToolScope::All);
    }

    #[test]
    fn memory_write_rejects_invalid_scope_value() {
        let tool = MemoryWriteTool::new();
        let err = tool
            .validate_args(&json!({ "content": "x", "scope": "global" }))
            .unwrap_err();
        assert!(err.contains("scope"));
    }

    #[test]
    fn memory_write_requires_content() {
        let tool = MemoryWriteTool::new();
        let err = tool.validate_args(&json!({ "scope": "user" })).unwrap_err();
        assert!(err.contains("content"));
    }
}
