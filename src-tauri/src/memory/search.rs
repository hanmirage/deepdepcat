//! Memory searcher — hybrid search with BM25 keyword scoring + cosine similarity.
//!
//! Combines:
//! - SQLite FTS5 BM25 for keyword relevance
//! - Cosine similarity on vector embeddings for semantic relevance
//! - Access frequency and recency for long-term relevance
//!
//! The final score is a weighted merge:
//! `final = w_bm25 * bm25_score + w_cosine * cosine_score + w_recency * recency_score`

use crate::core::error::AppResult;
use crate::memory::embedding::EmbeddingProvider;
use crate::memory::store::{Memory, MemoryStore};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Multiplier applied to durable (global, session-independent) memories in
/// hybrid scoring. Guards against long sessions' many high-recency session
/// entries crowding durable knowledge out of the top-N.
const GLOBAL_SOURCE_BOOST: f32 = 1.2;

/// Maximum durable memories guaranteed into the result even when session logs
/// dominate the keyword ranking (evergreen supplement).
const MIN_GLOBAL_RESULTS: usize = 2;

/// Search result with relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: Memory,
    pub score: f32,
    pub matched_terms: Vec<String>,
}

/// Weights for hybrid search scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchWeights {
    /// Weight for BM25 keyword score (0.0–1.0).
    pub bm25: f32,
    /// Weight for cosine similarity score (0.0–1.0).
    pub cosine: f32,
    /// Weight for access frequency and recency (0.0–1.0).
    pub recency: f32,
}

impl Default for SearchWeights {
    fn default() -> Self {
        Self {
            bm25: 0.4,
            cosine: 0.4,
            recency: 0.2,
        }
    }
}

/// The memory searcher — provides hybrid relevance-scored search.
#[derive(Clone)]
pub struct MemorySearcher {
    store: Arc<MemoryStore>,
    embedding_provider: Arc<EmbeddingProvider>,
    min_score: f32,
    max_results: u32,
    weights: Arc<RwLock<SearchWeights>>,
    /// MMR diversity-vs-relevance tradeoff (0 = max diversity, 1 = max relevance).
    mmr_lambda: f32,
    /// Half-life (hours) of the recency decay component.
    recency_half_life_hours: f64,
    /// Recency temperature — the time-decay component is raised to this
    /// power before merging. > 1.0 sharpens recency dominance (recent
    /// memories dominate), < 1.0 flattens it (older memories stay relevant).
    /// Stored as f32 bits for interior-mutability hot updates.
    recency_temperature: Arc<std::sync::atomic::AtomicU32>,
}

impl MemorySearcher {
    /// Create a new memory searcher with the given store and embedding provider.
    pub fn new(
        store: Arc<MemoryStore>,
        embedding_provider: Arc<EmbeddingProvider>,
        min_score: f32,
        max_results: u32,
    ) -> Self {
        Self {
            store,
            embedding_provider,
            min_score,
            max_results,
            weights: Arc::new(RwLock::new(SearchWeights::default())),
            mmr_lambda: 0.7,
            recency_half_life_hours: DEFAULT_HALF_LIFE_HOURS,
            recency_temperature: Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
        }
    }

    /// Set custom search weights (used at startup from config).
    pub fn with_weights(self, weights: SearchWeights) -> Self {
        *self.weights.write().unwrap_or_else(|e| e.into_inner()) = weights;
        self
    }

    /// Hot-update the search weights at runtime (config UI sliders).
    pub fn update_weights(&self, weights: SearchWeights) {
        *self.weights.write().unwrap_or_else(|e| e.into_inner()) = weights;
    }

    /// Current search weights.
    pub fn weights(&self) -> SearchWeights {
        self.weights
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set the recency decay half-life in hours.
    pub fn with_recency_half_life(mut self, hours: f64) -> Self {
        self.recency_half_life_hours = hours.max(0.5);
        self
    }

    /// Set the recency temperature (>1 sharpens recency dominance).
    pub fn with_recency_temperature(self, temperature: f32) -> Self {
        self.set_recency_temperature(temperature);
        self
    }

    /// Hot-update the recency temperature at runtime.
    pub fn set_recency_temperature(&self, temperature: f32) {
        let clamped = temperature.clamp(0.1, 5.0);
        self.recency_temperature
            .store(clamped.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Current recency temperature.
    fn recency_temperature(&self) -> f32 {
        f32::from_bits(
            self.recency_temperature
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Recency score for a memory using the configured half-life and
    /// temperature.
    fn recency_score(&self, memory: &Memory) -> f32 {
        Self::recency_score_with_half_life(memory, self.recency_half_life_hours)
            .powf(self.recency_temperature())
    }

    /// Hybrid search combining BM25 keyword and cosine similarity.
    pub async fn search(&self, query: &str) -> AppResult<Vec<SearchResult>> {
        // Step 1: BM25 keyword search via FTS5 — offloaded (MATCH scan +
        // per-row access updates block the worker).
        let fts_results = self
            .store
            .search_async(query.to_string(), self.max_results)
            .await?;

        // Step 2: Generate query embedding
        let query_embedding = self.embedding_provider.embed(query).await?;

        // Normalize BM25 scores: FTS5 bm25() returns negative values (more negative = more relevant).
        // Invert to positive and scale relative to the maximum in this result set.
        let max_bm25 = fts_results
            .iter()
            .map(|(_, score)| score.abs())
            .fold(0.0_f64, f64::max)
            .max(1e-9);

        // Step 3: For each FTS result, compute cosine similarity if embedding exists
        let mut scored_results: Vec<SearchResult> = Vec::new();

        for (mem, raw_bm25) in fts_results {
            // Cosine similarity (0.0 if no embedding)
            let cosine_score = self
                .store
                .get_embedding(mem.id)?
                .map(|emb| EmbeddingProvider::cosine_similarity(&query_embedding, &emb))
                .unwrap_or(0.0);

            // Recency score: higher for recently accessed memories
            let recency_score = self.recency_score(&mem);

            // Normalize BM25 to [0, 1]: invert sign (more negative = more relevant → higher)
            let bm25_score = (raw_bm25.abs() / max_bm25) as f32;

            // Weighted merge
            let weights = self.weights();
            let mut final_score = weights.bm25 * bm25_score
                + weights.cosine * cosine_score
                + weights.recency * recency_score;
            // Source boost: durable (global) memories — session_id IS NULL,
            // written by dream synthesis or stored without a session — get a
            // small multiplier so long sessions' many high-recency entries
            // cannot crowd them out of the top-N.
            if mem.session_id.is_none() {
                final_score *= GLOBAL_SOURCE_BOOST;
            }
            // Apply the relevance decay factor (1.0 = full relevance; dream
            // decay lowers it so consumed originals rank below fresh entries).
            let final_score = final_score * mem.decay_factor.unwrap_or(1.0);

            // Extract matched terms
            let matched_terms = query
                .split_whitespace()
                .filter(|term| mem.content.to_lowercase().contains(&term.to_lowercase()))
                .map(|s| s.to_string())
                .collect();

            scored_results.push(SearchResult {
                memory: mem,
                score: final_score,
                matched_terms,
            });
        }

        // Step 4: Also search via pure vector similarity for memories not caught by FTS
        if !query_embedding.is_empty() {
            let vector_results = self.vector_search(&query_embedding).await?;

            // Merge: only add memories not already in results
            for (mem, sim) in vector_results {
                if !scored_results.iter().any(|r| r.memory.id == mem.id) {
                    let recency_score =
                        Self::recency_score_with_half_life(&mem, self.recency_half_life_hours);
                    let weights = self.weights();
                    let mut final_score = weights.cosine * sim + weights.recency * recency_score;
                    // Apply the relevance decay factor (see search()).
                    final_score *= mem.decay_factor.unwrap_or(1.0);

                    scored_results.push(SearchResult {
                        memory: mem,
                        score: final_score,
                        matched_terms: vec![],
                    });
                }
            }
        }

        // Step 5: Evergreen supplement — durable (global) memories can be
        // crowded out by a session's many high-recency entries. If the result
        // set has fewer than MIN_GLOBAL_RESULTS of them, run a dedicated
        // global-only keyword query and merge in whatever is missing.
        let global_count = scored_results
            .iter()
            .filter(|r| r.memory.session_id.is_none())
            .count();
        if global_count < MIN_GLOBAL_RESULTS {
            let global_fts = self.store.search_global(query, self.max_results)?;
            let query_embedding = &query_embedding;
            for (mem, raw_bm25) in global_fts {
                if scored_results.iter().any(|r| r.memory.id == mem.id) {
                    continue;
                }
                let cosine_score = self
                    .store
                    .get_embedding(mem.id)?
                    .map(|emb| EmbeddingProvider::cosine_similarity(query_embedding, &emb))
                    .unwrap_or(0.0);
                let recency_score = self.recency_score(&mem);
                let max_bm25 = max_bm25.max(raw_bm25.abs());
                let bm25_score = (raw_bm25.abs() / max_bm25) as f32;
                let weights = self.weights();
                let mut final_score = weights.bm25 * bm25_score
                    + weights.cosine * cosine_score
                    + weights.recency * recency_score;
                final_score *= GLOBAL_SOURCE_BOOST;
                final_score *= mem.decay_factor.unwrap_or(1.0);
                scored_results.push(SearchResult {
                    memory: mem,
                    score: final_score,
                    matched_terms: vec![],
                });
            }
        }

        // Sort by score descending
        scored_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Over-fetch for MMR: fetch 2x the desired results, then deduplicate
        let fetch_limit = (self.max_results as usize) * 2;
        scored_results.truncate(fetch_limit);

        // Filter by min score
        scored_results.retain(|r| r.score >= self.min_score);

        // Apply MMR to balance relevance and diversity
        let mmr_results = self.mmr_rerank(scored_results);

        Ok(mmr_results)
    }

    /// Pure vector similarity search.
    ///
    /// FULL scan on purpose: the vector library is small (a few thousand
    /// rows at most), so scanning every embedded memory is cheap AND keeps
    /// old memories semantically reachable — the previous 2×limit window
    /// made long-past memories invisible to vector retrieval. Switch to an
    /// ANN index only when the store grows past the full-scan threshold.
    async fn vector_search(&self, query_embedding: &[f32]) -> AppResult<Vec<(Memory, f32)>> {
        let memories_with_embeddings = self.store.list_with_embeddings_async(u32::MAX).await?;

        let mut scored: Vec<(Memory, f32)> = memories_with_embeddings
            .into_iter()
            .map(|(_id, embedding, memory)| {
                let sim = EmbeddingProvider::cosine_similarity(query_embedding, &embedding);
                (memory, sim)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        // Bound the merge work: top-N candidates are enough — everything
        // beyond a generous over-fetch window is irrelevant anyway.
        let merge_cap = (self.max_results as usize) * 4;
        scored.truncate(merge_cap);

        Ok(scored)
    }

    /// Apply Maximal Marginal Relevance (MMR) to balance relevance and diversity.
    ///
    /// Iteratively selects results that are both relevant to the query and
    /// dissimilar from already-selected results. Reduces redundancy when
    /// multiple memories cover similar content.
    fn mmr_rerank(&self, candidates: Vec<SearchResult>) -> Vec<SearchResult> {
        let target = self.max_results as usize;
        if candidates.len() <= target {
            return candidates;
        }

        // Pre-fetch embeddings for all candidates
        let embeddings: Vec<Option<Vec<f32>>> = candidates
            .iter()
            .map(|r| self.store.get_embedding(r.memory.id).unwrap_or(None))
            .collect();

        let mut selected: Vec<usize> = Vec::with_capacity(target);
        let mut remaining: Vec<usize> = (0..candidates.len()).collect();

        // Select the highest-scoring candidate first
        selected.push(remaining.remove(0));

        while selected.len() < target && !remaining.is_empty() {
            let mut best_idx = 0;
            let mut best_mmr = f32::MIN;

            for (i, &cand_idx) in remaining.iter().enumerate() {
                let relevance = candidates[cand_idx].score;

                // Max similarity to any already-selected result
                let max_sim = selected
                    .iter()
                    .filter_map(|&sel_idx| {
                        let sel_emb = embeddings[sel_idx].as_ref()?;
                        let cand_emb = embeddings[cand_idx].as_ref()?;
                        Some(EmbeddingProvider::cosine_similarity(sel_emb, cand_emb))
                    })
                    .fold(0.0_f32, f32::max);

                let mmr = self.mmr_lambda * relevance - (1.0 - self.mmr_lambda) * max_sim;

                if mmr > best_mmr {
                    best_mmr = mmr;
                    best_idx = i;
                }
            }

            selected.push(remaining.remove(best_idx));
        }

        selected
            .into_iter()
            .map(|i| candidates[i].clone())
            .collect()
    }

    /// Search by category.
    pub fn search_by_category(&self, category: &str) -> AppResult<Vec<SearchResult>> {
        let memories = self.store.search_by_category(category, self.max_results)?;

        Ok(memories
            .into_iter()
            .map(|mem| SearchResult {
                score: self.recency_score(&mem),
                memory: mem,
                matched_terms: vec![],
            })
            .collect())
    }

    /// Compute a recency-based score (0.0–1.0).
    ///
    /// Two components:
    /// - **Access frequency**: logarithmic saturation — frequent access
    ///   matters, but the 100th access barely matters more than the 50th.
    /// - **Time decay**: exponential decay on `last_accessed`
    ///   (`exp(-hours_since / half_life_hours)`) — a memory accessed
    ///   minutes ago is much more relevant than one accessed weeks ago.
    ///
    /// Both are normalized into [0, 1] and combined with a floor of 0.5
    /// so recency never zeroes out an otherwise-relevant memory.
    fn recency_score_with_half_life(memory: &Memory, half_life_hours: f64) -> f32 {
        let access_score = if memory.access_count > 0 {
            ((1.0 + memory.access_count as f32).ln() / 10.0).min(1.0)
        } else {
            0.0
        };

        let time_score = match parse_last_accessed(memory.last_accessed.as_deref()) {
            Some(accessed) => {
                let hours = (Utc::now() - accessed).num_minutes() as f64 / 60.0;
                let hours = hours.max(0.0);
                let half_life = half_life_hours.max(0.5);
                (-hours / half_life).exp() as f32
            }
            None => 0.0,
        };

        (1.0 + access_score + time_score).min(2.0) / 2.0
    }
}

/// Default recency half-life: 7 days.
const DEFAULT_HALF_LIFE_HOURS: f64 = 24.0 * 7.0;

/// Parse a `last_accessed` value into a UTC timestamp.
///
/// Storage uses RFC 3339 (`Utc::now().to_rfc3339()`); tests and older
/// rows may carry date-only values (`2024-06-01`), which are treated as
/// midnight UTC of that day.
fn parse_last_accessed(value: Option<&str>) -> Option<chrono::DateTime<Utc>> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(parsed.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recency_score_no_access() {
        let mem = Memory {
            id: 1,
            content: "test".to_string(),
            metadata: serde_json::json!({}),
            category: "test".to_string(),
            session_id: None,
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
            access_count: 0,
            last_accessed: None,
            decay_factor: None,
        };
        let score = MemorySearcher::recency_score_with_half_life(&mem, DEFAULT_HALF_LIFE_HOURS);
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn test_recency_score_with_access() {
        let mem = Memory {
            id: 1,
            content: "test".to_string(),
            metadata: serde_json::json!({}),
            category: "test".to_string(),
            session_id: None,
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
            access_count: 10,
            last_accessed: Some("2024-06-01".to_string()),
            decay_factor: None,
        };
        let score = MemorySearcher::recency_score_with_half_life(&mem, DEFAULT_HALF_LIFE_HOURS);
        // Should be higher than no-access score
        assert!(score > 0.5);
    }

    #[test]
    fn recent_access_outranks_stale_access() {
        let fresh = Memory {
            id: 1,
            content: "fresh".to_string(),
            metadata: serde_json::json!({}),
            category: "test".to_string(),
            session_id: None,
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
            access_count: 1,
            last_accessed: Some(Utc::now().to_rfc3339()),
            decay_factor: None,
        };
        let stale = Memory {
            id: 2,
            content: "stale".to_string(),
            metadata: serde_json::json!({}),
            category: "test".to_string(),
            session_id: None,
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
            access_count: 1,
            last_accessed: Some("2024-01-01".to_string()),
            decay_factor: None,
        };
        let fresh_score =
            MemorySearcher::recency_score_with_half_life(&fresh, DEFAULT_HALF_LIFE_HOURS);
        let stale_score =
            MemorySearcher::recency_score_with_half_life(&stale, DEFAULT_HALF_LIFE_HOURS);
        assert!(
            fresh_score > stale_score,
            "recently accessed memory must outrank a stale one"
        );
    }

    #[test]
    fn shorter_half_life_amplifies_decay() {
        let mem = Memory {
            id: 1,
            content: "test".to_string(),
            metadata: serde_json::json!({}),
            category: "test".to_string(),
            session_id: None,
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
            access_count: 5,
            last_accessed: Some(Utc::now().to_rfc3339()),
            decay_factor: None,
        };
        let long = MemorySearcher::recency_score_with_half_life(&mem, 24.0 * 30.0);
        let short = MemorySearcher::recency_score_with_half_life(&mem, 0.5);
        assert!(long >= short);
    }

    #[test]
    fn test_search_weights_default() {
        let w = SearchWeights::default();
        assert_eq!(w.bm25, 0.4);
        assert_eq!(w.cosine, 0.4);
        assert_eq!(w.recency, 0.2);
    }
}
