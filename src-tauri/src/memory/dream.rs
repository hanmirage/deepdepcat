//! Dream synthesis — background memory consolidation.
//!
//! Periodically compresses raw memories into structured knowledge by:
//! 1. Clustering related memories
//! 2. Generating a summary using the LLM
//! 3. Storing the summary as a new "synthesized" memory
//! 4. Optionally decaying the original memories
//!
//! Gating mirrors the upstream pattern: `enabled` → `min_hours` since last
//! consolidation → `min_memories` accumulated. The LLM may answer `NO_REPLY`
//! when there is nothing worth persisting.

use crate::core::error::AppResult;
use crate::core::types::ConversationItem;
use crate::llm::client::LlmClient;
use crate::llm::provider::LlmProvider;
use crate::llm::provider::LlmRequest;
use crate::memory::store::{Memory, MemoryStore};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Marker the LLM returns when there is nothing worth consolidating.
const NO_REPLY_MARKER: &str = "NO_REPLY";

/// Tunables for dream synthesis.
#[derive(Debug, Clone)]
pub struct DreamConfig {
    /// Whether dream synthesis is enabled at all (cheapest gate).
    pub enabled: bool,
    /// Minimum hours between consolidation cycles.
    pub min_hours: u64,
    /// Minimum memories accumulated before consolidation is worth it.
    pub min_memories: usize,
    /// Maximum memories processed in one cycle (input cap).
    pub batch_size: usize,
    /// Whether to decay originals after successful synthesis.
    pub decay_originals: bool,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_hours: 24,
            min_memories: 3,
            batch_size: 50,
            decay_originals: true,
        }
    }
}

/// The dream engine — synthesizes raw memories into structured knowledge.
pub struct DreamEngine {
    store: Arc<MemoryStore>,
    llm_client: LlmClient,
    model: String,
    config: DreamConfig,
    /// Durable global usage aggregate — dream synthesis is not tied to a
    /// session, so its billed tokens count toward the app-wide totals
    /// instead of disappearing from the usage page entirely.
    global: Option<crate::storage::database::GlobalUsageStore>,
}

/// The result of a single dream synthesis cycle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DreamResult {
    /// Number of source memories processed.
    pub source_count: usize,
    /// Number of synthesized memories created.
    pub synthesized_count: usize,
    /// The generated summaries.
    pub summaries: Vec<String>,
}

impl DreamEngine {
    /// Create a new dream engine.
    pub fn new(store: Arc<MemoryStore>, llm_client: LlmClient, model: impl Into<String>) -> Self {
        Self {
            store,
            llm_client,
            model: model.into(),
            config: DreamConfig::default(),
            global: None,
        }
    }

    /// Override the dream tunables.
    pub fn with_config(mut self, config: DreamConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach the durable global usage store — every synthesis call then
    /// records its billed tokens into the app-wide aggregate.
    pub fn with_global(mut self, global: crate::storage::database::GlobalUsageStore) -> Self {
        self.global = Some(global);
        self
    }

    /// Run a single dream cycle.
    ///
    /// Gated (cheapest first): config enabled → hours since last synthesis →
    /// minimum memory count. Returns a no-op result when any gate closes.
    pub async fn dream(&self) -> AppResult<DreamResult> {
        if !self.config.enabled {
            debug!("Dream synthesis disabled by config");
            return Ok(DreamResult {
                source_count: 0,
                synthesized_count: 0,
                summaries: vec![],
            });
        }

        if let Some(hours) = self.hours_since_last_synthesis()? {
            if hours < self.config.min_hours {
                debug!(
                    hours_since = hours,
                    min_hours = self.config.min_hours,
                    "Dream synthesis gated by time"
                );
                return Ok(DreamResult {
                    source_count: 0,
                    synthesized_count: 0,
                    summaries: vec![],
                });
            }
        }

        info!("Starting dream synthesis cycle");

        // Get recent memories
        let memories = self.store.list_all(self.config.batch_size as u32)?;

        if memories.len() < self.config.min_memories {
            debug!(
                "Not enough memories for dream synthesis (got {}, min {})",
                memories.len(),
                self.config.min_memories
            );
            return Ok(DreamResult {
                source_count: memories.len(),
                synthesized_count: 0,
                summaries: vec![],
            });
        }

        // Group by category
        let groups = Self::group_by_category(memories);

        let mut summaries = Vec::new();
        let mut source_count = 0;

        for (category, group) in groups {
            if group.len() < 2 {
                continue; // Skip groups too small to synthesize
            }

            source_count += group.len();

            match self.synthesize_group(&category, &group).await {
                Ok(summary) => {
                    // Honor the NO_REPLY convention: nothing worth persisting.
                    if summary.trim() == NO_REPLY_MARKER {
                        debug!(category = %category, "Dream NO_REPLY — nothing to persist");
                        continue;
                    }

                    // Store the synthesized memory
                    let metadata = serde_json::json!({
                        "synthesized": true,
                        "source_count": group.len(),
                        "source_ids": group.iter().map(|m| m.id).collect::<Vec<_>>(),
                    });

                    self.store.store(
                        &summary,
                        &format!("synthesized_{}", category),
                        None,
                        Some(metadata),
                    )?;

                    // Decay originals
                    if self.config.decay_originals {
                        for mem in &group {
                            let _ = self.store.decay(mem.id, 0.5);
                        }
                    }

                    summaries.push(summary);
                    info!(
                        category = %category,
                        source_count = group.len(),
                        "Synthesized memory group"
                    );
                }
                Err(e) => {
                    warn!(
                        category = %category,
                        error = %e,
                        "Failed to synthesize memory group"
                    );
                }
            }
        }

        info!(
            source_count = source_count,
            synthesized_count = summaries.len(),
            "Dream synthesis complete"
        );

        Ok(DreamResult {
            source_count,
            synthesized_count: summaries.len(),
            summaries,
        })
    }

    /// Hours since the last synthesized memory was created.
    ///
    /// Returns `Ok(None)` when no synthesis has ever run (gate is open).
    ///
    /// Synthesized memories are stored with `synthesized_<category>` names,
    /// so the gate looks up the `synthesized_` prefix (a plain "synthesized"
    /// category query would never match and the time gate would stay open).
    fn hours_since_last_synthesis(&self) -> AppResult<Option<u64>> {
        let last = self
            .store
            .search_by_category_prefix("synthesized_", 1)?
            .into_iter()
            .next();
        let Some(mem) = last else {
            return Ok(None);
        };
        let created = chrono::DateTime::parse_from_rfc3339(&mem.created_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());
        let hours = (chrono::Utc::now() - created).num_hours();
        Ok(Some(hours.max(0) as u64))
    }

    /// Synthesize a group of memories into a single summary.
    async fn synthesize_group(&self, category: &str, memories: &[Memory]) -> AppResult<String> {
        let mut context = format!("Category: {}\n\nMemories to synthesize:\n\n", category);

        for (i, mem) in memories.iter().enumerate() {
            context.push_str(&format!("{}. {}\n", i + 1, mem.content));
        }

        let request = LlmRequest {
            model: self.model.clone(),
            provider: None,
            messages: vec![ConversationItem::user(format!(
                "Synthesize the following memories into a single concise knowledge entry. \
                Remove duplicates, extract key insights, and present the information clearly.\n\n{}",
                context
            ))],
            tools: vec![],
            system_prompt: "You are a memory synthesis assistant. Create concise, \
                deduplicated summaries of related memories. Focus on actionable knowledge. \
                If the memories contain nothing worth persisting, respond with exactly: NO_REPLY"
                .to_string(),
            temperature: Some(0.3),
            top_p: None,
            max_tokens: Some(1000),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let response = self.llm_client.complete(&request).await?;
        if let Some(global) = &self.global {
            global.add_llm(&response.usage);
        }
        Ok(response.content.trim().to_string())
    }

    /// Group memories by their category.
    fn group_by_category(memories: Vec<Memory>) -> Vec<(String, Vec<Memory>)> {
        let mut groups: std::collections::HashMap<String, Vec<Memory>> =
            std::collections::HashMap::new();

        for mem in memories {
            groups.entry(mem.category.clone()).or_default().push(mem);
        }

        groups.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_by_category() {
        let memories = vec![
            Memory {
                id: 1,
                content: "test1".to_string(),
                metadata: serde_json::json!({}),
                category: "project".to_string(),
                session_id: None,
                created_at: "2024-01-01".to_string(),
                updated_at: "2024-01-01".to_string(),
                access_count: 0,
                last_accessed: None,
                decay_factor: None,
            },
            Memory {
                id: 2,
                content: "test2".to_string(),
                metadata: serde_json::json!({}),
                category: "project".to_string(),
                session_id: None,
                created_at: "2024-01-02".to_string(),
                updated_at: "2024-01-02".to_string(),
                access_count: 1,
                last_accessed: None,
                decay_factor: None,
            },
            Memory {
                id: 3,
                content: "test3".to_string(),
                metadata: serde_json::json!({}),
                category: "preference".to_string(),
                session_id: None,
                created_at: "2024-01-03".to_string(),
                updated_at: "2024-01-03".to_string(),
                access_count: 0,
                last_accessed: None,
                decay_factor: None,
            },
        ];

        let groups = DreamEngine::group_by_category(memories);
        assert_eq!(groups.len(), 2);

        let project_group = groups.iter().find(|(cat, _)| cat == "project").unwrap();
        assert_eq!(project_group.1.len(), 2);
    }
}
