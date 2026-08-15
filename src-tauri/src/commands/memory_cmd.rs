//! Memory commands — search and manage memories with hybrid search.

use crate::bootstrap::AppState;
use crate::memory::search::SearchResult;
use crate::memory::store::Memory;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

/// Store a memory. Automatically generates a vector embedding.
#[tauri::command]
pub async fn store_memory(
    content: String,
    category: String,
    session_id: Option<String>,
    metadata: Option<Value>,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    // Store the memory in SQLite
    let id = state
        .memory
        .store(&content, &category, session_id.as_deref(), metadata)
        .map_err(|e| e.to_string())?;

    // Generate and store embedding (non-fatal if it fails)
    match state.embedding_provider.embed(&content).await {
        Ok(embedding) => {
            if let Err(e) = state.memory.store_embedding(id, &embedding) {
                tracing::warn!(memory_id = id, error = %e, "Failed to store embedding");
            } else if let Ok(superseded) = state.memory.supersede_similar(id, &embedding) {
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

    Ok(id)
}

/// Search memories using hybrid BM25 + cosine similarity.
/// Returns results with relevance scores.
#[tauri::command]
pub async fn search_memories(
    query: String,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    // Use the hybrid searcher from AppState
    let searcher = state.memory_searcher.clone();

    let results = searcher.search(&query).await.map_err(|e| e.to_string())?;

    // Truncate if a specific limit was requested
    let truncated = if let Some(l) = limit {
        results.into_iter().take(l as usize).collect()
    } else {
        results
    };

    Ok(truncated)
}

/// List all memories (for management UI).
#[tauri::command]
pub async fn list_memories(
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<Memory>, String> {
    state
        .memory
        .list_all(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

/// Delete a memory by ID.
#[tauri::command]
pub async fn delete_memory(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.memory.delete(id).map_err(|e| e.to_string())
}

/// Get memory count.
#[tauri::command]
pub async fn get_memory_count(state: State<'_, AppState>) -> Result<u64, String> {
    state.memory.count().map_err(|e| e.to_string())
}

/// Trigger dream synthesis — compresses raw memories into structured knowledge.
#[tauri::command]
pub async fn trigger_dream(
    model: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::memory::dream::DreamResult, String> {
    // Extract config values in a block so the RwLockReadGuard is dropped before await
    let (providers, retry_config, prompt_caching_enabled, dream_config, default_model) = {
        let config = state.config().map_err(|e| e.to_string())?;
        let retry = crate::llm::retry::RetryConfig::from_llm_config(&config.llm);
        (
            config.llm.providers.clone(),
            retry,
            config.llm.prompt_caching_enabled,
            crate::memory::dream::DreamConfig {
                enabled: config.memory.dream_enabled,
                min_hours: config.memory.dream_min_hours,
                min_memories: config.memory.dream_min_memories,
                ..Default::default()
            },
            config.app.default_model.clone(),
        )
    };

    // No hardcoded model: dream uses the configured default so non-DeepSeek
    // setups (Ollama, OpenAI-compatible, ...) work out of the box.
    let model_name = model.unwrap_or(default_model);
    let llm_client = crate::llm::client::LlmClient::new(
        providers,
        retry_config,
        prompt_caching_enabled,
        state.circuit_breaker.clone(),
    );

    let dream =
        crate::memory::dream::DreamEngine::new(state.memory.clone(), llm_client, model_name)
            .with_config(dream_config)
            .with_global(crate::storage::database::GlobalUsageStore::new(
                state.db.clone(),
            ));

    dream.dream().await.map_err(|e| e.to_string())
}

/// Info about one MEMORY.md layer (the standing, always-in-context memory
/// managed by `memory_write`).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryFileInfo {
    pub path: String,
    pub exists: bool,
    pub chars: usize,
    pub entries: usize,
    pub modified_at_ms: Option<u64>,
}

/// View of both MEMORY.md layers for the settings page.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryFilesView {
    pub user: MemoryFileInfo,
    pub project: Option<MemoryFileInfo>,
}

fn describe_memory_file(path: &std::path::Path) -> MemoryFileInfo {
    let content = crate::memory::memory_file::read_memory_file(path);
    let exists = content.is_some();
    let modified_at_ms = path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    let (chars, entries) = content
        .map(|c| {
            (
                c.chars().count(),
                c.lines()
                    .filter(|l| l.trim_start().starts_with("- "))
                    .count(),
            )
        })
        .unwrap_or((0, 0));
    MemoryFileInfo {
        path: path.to_string_lossy().to_string(),
        exists,
        chars,
        entries,
        modified_at_ms,
    }
}

/// Return the user-level and (when a workspace is open) project-level
/// MEMORY.md status — file path, existence, entry count, last modified.
#[tauri::command]
pub fn get_memory_files(state: State<'_, AppState>) -> Result<MemoryFilesView, String> {
    let user = describe_memory_file(&crate::memory::memory_file::user_memory_path());
    let project = state
        .workspace
        .read()
        .map(|w| w.clone())
        .unwrap_or(None)
        .map(|ws| describe_memory_file(&crate::memory::memory_file::project_memory_path(&ws)));
    Ok(MemoryFilesView { user, project })
}

/// View of both procedures.md layers for the settings page.
#[derive(Debug, Clone, Serialize)]
pub struct ProcedureFilesView {
    pub user: MemoryFileInfo,
    pub project: Option<MemoryFileInfo>,
}

fn describe_procedure_file(path: &std::path::Path) -> MemoryFileInfo {
    let content = crate::memory::memory_file::read_memory_file(path);
    let exists = content.is_some();
    let modified_at_ms = path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    let (chars, entries) = content
        .map(|c| {
            (
                c.chars().count(),
                crate::memory::procedure::parse_procedures(&c).len(),
            )
        })
        .unwrap_or((0, 0));
    MemoryFileInfo {
        path: path.to_string_lossy().to_string(),
        exists,
        chars,
        entries,
        modified_at_ms,
    }
}

/// Return the user-level and (when a workspace is open) project-level
/// procedures.md status — file path, existence, procedure count, last
/// modified.
#[tauri::command]
pub fn get_procedure_files(state: State<'_, AppState>) -> Result<ProcedureFilesView, String> {
    let user = describe_procedure_file(&crate::memory::procedure::user_procedures_path());
    let project = state
        .workspace
        .read()
        .map(|w| w.clone())
        .unwrap_or(None)
        .map(|ws| {
            describe_procedure_file(&crate::memory::procedure::project_procedures_path(&ws))
        });
    Ok(ProcedureFilesView { user, project })
}
