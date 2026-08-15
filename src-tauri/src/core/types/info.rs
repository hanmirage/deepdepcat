use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_hit_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_miss_tokens: Option<u64>,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }

    pub fn add(&mut self, other: &TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        if let Some(c) = other.cached_read_tokens {
            *self.cached_read_tokens.get_or_insert(0) += c;
        }
        if let Some(r) = other.reasoning_tokens {
            *self.reasoning_tokens.get_or_insert(0) += r;
        }
        if let Some(c) = other.prompt_cache_hit_tokens {
            *self.prompt_cache_hit_tokens.get_or_insert(0) += c;
        }
        if let Some(c) = other.prompt_cache_miss_tokens {
            *self.prompt_cache_miss_tokens.get_or_insert(0) += c;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub cpu_count: usize,
    pub total_memory_mb: u64,
    pub app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_data_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub description: String,
    pub context_window: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_price_per_1m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_price_per_1m: Option<f64>,
    /// Hidden from the user model picker. Legacy/deleted models (e.g. the
    /// retired DeepSeek chat/reasoner) stay in the catalog for backward
    /// compatibility but must not appear in the selector.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}
