//! Model catalog — maintains the list of available AI models across providers.

use crate::core::types::ModelInfo;
use std::collections::HashMap;
use std::sync::RwLock;

/// The model catalog — a registry of all known models across all providers.
///
/// Internally synchronized so the live `GET /models` refresh (which needs a
/// shared `&self`) can merge discovered models into the static snapshot.
pub struct ModelCatalog {
    models: RwLock<HashMap<String, ModelInfo>>,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelCatalog {
    pub fn new() -> Self {
        let mut catalog = Self {
            models: RwLock::new(HashMap::new()),
        };
        catalog.register_builtins();
        catalog
    }

    /// Register all built-in models.
    fn register_builtins(&mut self) {
        // ── DeepSeek ──────────────────────────────────────────────────────
        // Only V4 Pro / V4 Flash — these are the real models the DeepSeek V4
        // API offers. The retired `deepseek-chat` / `deepseek-reasoner`
        // (V3-era) are no longer registered anywhere.
        self.register(ModelInfo {
            id: "deepseek-v4-pro".to_string(),
            name: "DeepSeek V4 Pro".to_string(),
            provider: "deepseek".to_string(),
            description: "Next-generation flagship model with 1M context".to_string(),
            context_window: 1_000_000,
            max_output_tokens: Some(8_192),
            supports_tools: Some(true),
            supports_vision: Some(false),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.40),
            output_price_per_1m: Some(0.80),
            hidden: false,
        });
        self.register(ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            name: "DeepSeek V4 Flash".to_string(),
            provider: "deepseek".to_string(),
            description: "Fast and affordable model with 1M context".to_string(),
            context_window: 1_000_000,
            max_output_tokens: Some(8_192),
            supports_tools: Some(true),
            supports_vision: Some(false),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.13),
            output_price_per_1m: Some(0.27),
            hidden: false,
        });

        // ── OpenAI ────────────────────────────────────────────────────────
        self.register(ModelInfo {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            provider: "openai".to_string(),
            description: "Most capable OpenAI model with vision".to_string(),
            context_window: 128_000,
            max_output_tokens: Some(16_384),
            supports_tools: Some(true),
            supports_vision: Some(true),
            supports_streaming: Some(true),
            input_price_per_1m: Some(2.50),
            output_price_per_1m: Some(10.00),
            hidden: false,
        });
        self.register(ModelInfo {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o Mini".to_string(),
            provider: "openai".to_string(),
            description: "Affordable and fast model for everyday tasks".to_string(),
            context_window: 128_000,
            max_output_tokens: Some(16_384),
            supports_tools: Some(true),
            supports_vision: Some(true),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.15),
            output_price_per_1m: Some(0.60),
            hidden: false,
        });

        // ── Anthropic ─────────────────────────────────────────────────────
        self.register(ModelInfo {
            id: "claude-sonnet-4-20250514".to_string(),
            name: "Claude Sonnet 4".to_string(),
            provider: "anthropic".to_string(),
            description: "Best balance of intelligence and speed".to_string(),
            context_window: 200_000,
            max_output_tokens: Some(16_384),
            supports_tools: Some(true),
            supports_vision: Some(true),
            supports_streaming: Some(true),
            input_price_per_1m: Some(3.00),
            output_price_per_1m: Some(15.00),
            hidden: false,
        });
        self.register(ModelInfo {
            id: "claude-3-5-haiku-20241022".to_string(),
            name: "Claude 3.5 Haiku".to_string(),
            provider: "anthropic".to_string(),
            description: "Fast and affordable model".to_string(),
            context_window: 200_000,
            max_output_tokens: Some(8_192),
            supports_tools: Some(true),
            supports_vision: Some(true),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.80),
            output_price_per_1m: Some(4.00),
            hidden: false,
        });

        // ── xAI (Grok) ────────────────────────────────────────────────────
        self.register(ModelInfo {
            id: "grok-3".to_string(),
            name: "Grok 3".to_string(),
            provider: "grok".to_string(),
            description: "Most capable Grok model".to_string(),
            context_window: 131_072,
            max_output_tokens: Some(16_384),
            supports_tools: Some(true),
            supports_vision: Some(false),
            supports_streaming: Some(true),
            input_price_per_1m: Some(3.00),
            output_price_per_1m: Some(15.00),
            hidden: false,
        });
        self.register(ModelInfo {
            id: "grok-3-mini".to_string(),
            name: "Grok 3 Mini".to_string(),
            provider: "grok".to_string(),
            description: "Fast and lightweight Grok model".to_string(),
            context_window: 131_072,
            max_output_tokens: Some(16_384),
            supports_tools: Some(true),
            supports_vision: Some(false),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.30),
            output_price_per_1m: Some(0.50),
            hidden: false,
        });

        // ── Ollama (local) ────────────────────────────────────────────────
        self.register(ModelInfo {
            id: "llama3.3".to_string(),
            name: "Llama 3.3 (Local)".to_string(),
            provider: "ollama".to_string(),
            description: "Run Llama 3.3 locally via Ollama".to_string(),
            context_window: 32_000,
            max_output_tokens: Some(4_096),
            supports_tools: Some(true),
            supports_vision: Some(false),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.0),
            output_price_per_1m: Some(0.0),
            hidden: false,
        });
        self.register(ModelInfo {
            id: "qwen2.5".to_string(),
            name: "Qwen 2.5 (Local)".to_string(),
            provider: "ollama".to_string(),
            description: "Run Qwen 2.5 locally via Ollama".to_string(),
            context_window: 32_000,
            max_output_tokens: Some(4_096),
            supports_tools: Some(true),
            supports_vision: Some(false),
            supports_streaming: Some(true),
            input_price_per_1m: Some(0.0),
            output_price_per_1m: Some(0.0),
            hidden: false,
        });
    }

    /// Register a new model.
    pub fn register(&mut self, model: ModelInfo) {
        self.models
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(model.id.clone(), model);
    }

    /// Get the context window size for a model.
    pub fn context_window(&self, model_id: &str) -> u64 {
        self.models
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(model_id)
            .map(|m| m.context_window)
            .unwrap_or(8_192)
    }

    /// Whether a model natively accepts image input. Unknown ids default to
    /// false — never bet a text-only model can see a picture. The agent
    /// relies on automatic transcription (image_transcribe) for such models.
    pub fn supports_vision(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(model_id)
            .and_then(|m| m.supports_vision)
            .unwrap_or(false)
    }

    /// Best-effort pricing for a model id, from the catalog's per-model prices.
    ///
    /// Reads `input_price_per_1m` / `output_price_per_1m` for the exact model
    /// id; falls back to the `TokenPricing::for_model` class heuristic for
    /// unknown/custom ids (and when prices are absent). This is the single
    /// source of truth for the live cost guard — the heuristic alone would
    /// bill GPT-4o/Claude at DeepSeek rates and silently overrun a cost cap.
    pub fn pricing(&self, model_id: &str) -> crate::core::types::TokenPricing {
        let models = self.models.read().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = models.get(model_id) {
            if let (Some(input), Some(output)) = (m.input_price_per_1m, m.output_price_per_1m) {
                return crate::core::types::TokenPricing {
                    prompt_per_million: input,
                    completion_per_million: output,
                };
            }
        }
        crate::core::types::TokenPricing::for_model(model_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_vision_matches_registered_capability() {
        let catalog = ModelCatalog::new();
        // Multimodal models accept images.
        assert!(catalog.supports_vision("gpt-4o"));
        assert!(catalog.supports_vision("claude-sonnet-4-20250514"));
        // Text-only models (DeepSeek) do not — image embedding is skipped.
        assert!(!catalog.supports_vision("deepseek-v4-pro"));
        assert!(!catalog.supports_vision("deepseek-v4-flash"));
        // Unknown ids default to false — never bet on an unknown model.
        assert!(!catalog.supports_vision("some-custom-model"));
    }

    #[test]
    fn pricing_reads_per_model_catalog_prices() {
        let catalog = ModelCatalog::new();
        // GPT-4o must be billed at OpenAI rates, NOT the DeepSeek fallback.
        let gpt = catalog.pricing("gpt-4o");
        assert_eq!(gpt.prompt_per_million, 2.50);
        assert_eq!(gpt.completion_per_million, 10.00);
        // Claude is also priced from the catalog.
        let claude = catalog.pricing("claude-sonnet-4-20250514");
        assert_eq!(claude.prompt_per_million, 3.00);
        // DeepSeek prices come from the catalog (not the stale hardcode).
        let pro = catalog.pricing("deepseek-v4-pro");
        assert_eq!(pro.prompt_per_million, 0.40);
        assert_eq!(pro.completion_per_million, 0.80);
        // Unknown ids fall back to the heuristic.
        let unknown = catalog.pricing("some-custom-model");
        assert_eq!(
            unknown.prompt_per_million,
            crate::core::types::TokenPricing::for_model("some-custom-model").prompt_per_million
        );
    }
}
