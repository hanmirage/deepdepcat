//! Embedding provider — generates vector embeddings for text.
//!
//! Supports two modes:
//! 1. **API-based** — calls an LLM embedding endpoint (OpenAI-compatible)
//! 2. **Local hash-based** — a deterministic fallback that produces a fixed-dimension
//!    vector from text hashing. Not semantically meaningful but enables testing
//!    and basic deduplication without an API key.

use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// The dimensionality of embeddings.
pub const EMBEDDING_DIM: usize = 256;

/// A vector embedding — a fixed-size array of f32 values.
pub type Embedding = Vec<f32>;

/// Configuration for the embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// The provider type: "api" or "local".
    pub provider: String,
    /// The model name for API-based embeddings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The API base URL (for API-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    /// The API key (for API-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            model: None,
            api_base: None,
            api_key: None,
        }
    }
}

/// The embedding provider — generates vector embeddings for text.
pub struct EmbeddingProvider {
    config: EmbeddingConfig,
    http: reqwest::Client,
}

impl EmbeddingProvider {
    /// Create a new embedding provider with the given config.
    pub fn new(config: EmbeddingConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { config, http }
    }

    /// Create a local-only embedding provider (no API calls).
    pub fn local() -> Self {
        Self::new(EmbeddingConfig::default())
    }

    /// Generate an embedding for the given text.
    pub async fn embed(&self, text: &str) -> AppResult<Embedding> {
        if self.config.provider == "local" {
            return Ok(Self::local_embed(text));
        }

        self.api_embed(text).await
    }

    /// Local hash-based embedding — deterministic but not semantically meaningful.
    fn local_embed(text: &str) -> Embedding {
        let mut embedding = vec![0.0f32; EMBEDDING_DIM];

        // Use a simple hash-based approach: hash each token and accumulate
        // into the embedding vector. This produces a deterministic "bag of words"
        // representation that captures term frequency.
        let tokens: Vec<&str> = text.split_whitespace().collect();

        for token in &tokens {
            let lower = token.to_lowercase();
            let hash = Self::fnv1a_hash(&lower);

            // Distribute the token's weight across multiple dimensions
            // using the hash as a seed. This creates a sparse representation.
            for i in 0..3 {
                let idx = ((hash.wrapping_mul((i as u64 + 1).wrapping_mul(0x9E3779B97F4A7C15))
                    >> 32) as usize)
                    % EMBEDDING_DIM;
                embedding[idx] += 1.0 / (tokens.len() as f32).sqrt();
            }
        }

        // L2 normalize
        let norm: f32 = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut embedding {
                *v /= norm;
            }
        }

        embedding
    }

    /// Call an API endpoint to generate an embedding.
    async fn api_embed(&self, text: &str) -> AppResult<Embedding> {
        let api_base = self
            .config
            .api_base
            .as_deref()
            .ok_or_else(|| AppError::Memory("API base not configured for embeddings".into()))?;

        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| AppError::Memory("API key not configured for embeddings".into()))?;

        let model = self
            .config
            .model
            .as_deref()
            .unwrap_or("text-embedding-3-small");

        let url = format!("{}/embeddings", api_base.trim_end_matches('/'));

        let body = serde_json::json!({
            "model": model,
            "input": text,
        });

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Memory(format!("Embedding API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::Memory(format!(
                "Embedding API error {}: {}",
                status, text
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Memory(format!("Failed to parse embedding response: {}", e)))?;

        // Extract the embedding vector from the response
        let embedding_data = json
            .get("data")
            .and_then(|d| d.get(0))
            .and_then(|d| d.get("embedding"))
            .ok_or_else(|| AppError::Memory("No embedding in API response".into()))?;

        let embedding: Vec<f32> = serde_json::from_value(embedding_data.clone())
            .map_err(|e| AppError::Memory(format!("Failed to parse embedding vector: {}", e)))?;

        Ok(embedding)
    }

    /// FNV-1a hash — fast, deterministic, good distribution for hashing.
    fn fnv1a_hash(s: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in s.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Compute cosine similarity between two embeddings.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_embed_deterministic() {
        let e1 = EmbeddingProvider::local_embed("hello world");
        let e2 = EmbeddingProvider::local_embed("hello world");
        assert_eq!(e1, e2);
    }

    #[test]
    fn test_local_embed_different_text() {
        let e1 = EmbeddingProvider::local_embed("hello world");
        let e2 = EmbeddingProvider::local_embed("goodbye universe");
        assert_ne!(e1, e2);
    }

    #[test]
    fn test_local_embed_dimension() {
        let e = EmbeddingProvider::local_embed("test");
        assert_eq!(e.len(), EMBEDDING_DIM);
    }

    #[test]
    fn test_local_embed_normalized() {
        let e = EmbeddingProvider::local_embed("test embedding normalization");
        let norm: f32 = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        // Should be approximately 1.0 (or 0.0 for empty)
        assert!(norm < 1.01);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let e = EmbeddingProvider::local_embed("hello");
        let sim = EmbeddingProvider::cosine_similarity(&e, &e);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_different() {
        let e1 = EmbeddingProvider::local_embed("hello world foo bar");
        let e2 = EmbeddingProvider::local_embed("goodbye universe baz qux");
        let sim = EmbeddingProvider::cosine_similarity(&e1, &e2);
        // Different texts should have lower similarity
        assert!(sim < 0.9);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let sim = EmbeddingProvider::cosine_similarity(&[], &[]);
        assert_eq!(sim, 0.0);
    }

    #[tokio::test]
    async fn test_local_provider_embed() {
        let provider = EmbeddingProvider::local();
        let embedding = provider.embed("test text").await.unwrap();
        assert_eq!(embedding.len(), EMBEDDING_DIM);
    }
}
