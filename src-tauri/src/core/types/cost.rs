//! Model cost estimation — pricing for token accounting.
//!
//! Used by the agent budget tracker (cost guard) and the session persistence
//! layer (stored estimated cost). Pure type: lives in infra so both harness
//! (agent) and infra (storage) depend on it, never the other way.

use super::TokenUsage;

/// Pricing for a single provider (per million tokens, in USD).
#[derive(Debug, Clone)]
pub struct TokenPricing {
    /// Cost per million prompt tokens (input).
    pub prompt_per_million: f64,
    /// Cost per million completion tokens (output).
    pub completion_per_million: f64,
}

impl TokenPricing {
    /// DeepSeek V4 Pro pricing — mirrors the model catalog (`llm/models.rs`),
    /// which is the single source of truth for per-model prices. This constant
    /// exists only so the `for_model` heuristic fallback (used by storage and
    /// unknown/custom model ids) stays consistent with the catalog.
    pub fn deepseek_pro() -> Self {
        Self {
            prompt_per_million: 0.40,
            completion_per_million: 0.80,
        }
    }

    /// DeepSeek V4 Flash pricing — mirrors the model catalog.
    pub fn deepseek_flash() -> Self {
        Self {
            prompt_per_million: 0.13,
            completion_per_million: 0.27,
        }
    }

    /// Best-effort pricing for a model id, used ONLY as a fallback when the
    /// model is not in the catalog (unknown/custom id). The live cost guard
    /// prefers `ModelCatalog::pricing`, which reads the exact per-model price.
    ///
    /// Flash-class model names (flash/mini/lite — DeepSeek's cheap tier, an
    /// order of magnitude below pro) get flash pricing; everything else
    /// defaults to pro. Hardcoding pro for every session priced flash runs
    /// at ~8x, tripping the session cost limit ~8x early; the reverse would
    /// silently overrun the limit. This is a heuristic for a cost guard,
    /// not a billing ledger.
    pub fn for_model(model: &str) -> Self {
        let lower = model.to_lowercase();
        if ["flash", "mini", "lite"].iter().any(|k| lower.contains(k)) {
            Self::deepseek_flash()
        } else {
            Self::deepseek_pro()
        }
    }

    /// Discount applied to prompt tokens served from the provider's prefix
    /// cache (DeepSeek KV cache hit ≈ 10x cheaper than a miss). Session
    /// cost accounting must not bill cached tokens at full price — a
    /// cache-first session would otherwise trip the cost limit ~10x early.
    pub const CACHE_HIT_DISCOUNT: f64 = 0.1;

    /// Compute the cost for a given usage.
    ///
    /// Cached prompt tokens are billed at the discounted rate (M16 cache
    /// discount, see [`Self::CACHE_HIT_DISCOUNT`]); `prompt_tokens` includes
    /// the cache-hit tokens, so they are split off here instead of being
    /// counted twice. Reasoning tokens are NOT priced separately — they are
    /// part of `completion_tokens` (the provider reports them as a detail of
    /// the same output billing), so no additional accounting is needed.
    pub fn cost(&self, usage: &TokenUsage) -> f64 {
        // The cached portion of the prompt is billed at the discounted rate;
        // `prompt_tokens` includes the cache-hit tokens, so they are split
        // off here instead of being counted twice.
        let hit_tokens = usage
            .prompt_cache_hit_tokens
            .or(usage.cached_read_tokens)
            .unwrap_or(0)
            .min(usage.prompt_tokens) as f64;
        let uncached_prompt = usage.prompt_tokens as f64 - hit_tokens;
        let prompt = uncached_prompt / 1_000_000.0 * self.prompt_per_million
            + hit_tokens / 1_000_000.0 * self.prompt_per_million * Self::CACHE_HIT_DISCOUNT;
        let completion = usage.completion_tokens as f64 / 1_000_000.0 * self.completion_per_million;
        prompt + completion
    }
}
