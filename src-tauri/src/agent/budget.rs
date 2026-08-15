//! Per-turn and per-session token budget tracker.
//!
//! Tracks turn count, token usage, and estimated cost within a single agent
//! loop invocation. The loop terminates when `max_turns` is reached or when
//! the session-level budget is exhausted.
//!
//! Per-turn token limits are intentionally NOT enforced as auto-continue:
//! a single LLM response may legitimately use most of the context window for
//! the prompt (the conversation itself), so counting prompt tokens against
//! a per-turn budget would prematurely terminate valid long-context turns.
//! The context window is managed by compaction, not by a per-turn cap.
//!
//! Session-level limits (total tokens, total cost) ARE enforced — when the
//! cumulative cost exceeds the configured limit, the loop forces a final
//! answer via `force_final_answer`.

use crate::core::types::{TokenPricing, TokenUsage};

/// Configuration for the budget tracker.
#[derive(Debug, Clone)]
pub struct BudgetConfig {
    /// Maximum turns allowed in this loop.
    pub max_turns: u32,
    /// Session-level total token limit (0 = unlimited).
    pub session_token_limit: u64,
    /// Session-level total cost limit in USD (0.0 = unlimited).
    pub session_cost_limit: f64,
    /// Wall-clock timeout for ONE loop invocation in seconds
    /// (None = unlimited). Guards against a session that keeps "making
    /// progress" turn after turn while the clock burns.
    pub max_wall_clock_secs: Option<u64>,
    /// Pricing used for cost estimation.
    pub pricing: TokenPricing,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_turns: 50,
            session_token_limit: 0,
            session_cost_limit: 0.0,
            max_wall_clock_secs: None,
            pricing: TokenPricing::deepseek_pro(),
        }
    }
}

/// Tracks iteration count, token usage, and cost for a single agent loop run.
pub struct BudgetTracker {
    config: BudgetConfig,
    /// Current turn number (0-indexed; incremented by `begin_turn`).
    current_turn: u32,
    /// Total tokens used across all turns.
    total_tokens: u64,
    /// Total estimated cost across all turns (USD).
    total_cost: f64,
    /// Loop start instant — wall-clock budget base.
    started_at: std::time::Instant,
}

impl BudgetTracker {
    /// Create a new budget tracker with full configuration.
    pub fn with_config(config: BudgetConfig) -> Self {
        Self {
            config,
            current_turn: 0,
            total_tokens: 0,
            total_cost: 0.0,
            started_at: std::time::Instant::now(),
        }
    }

    /// Create a budget tracker seeded with usage accrued BEFORE this loop
    /// invocation.
    ///
    /// The session-level token/cost limits are declared "session" limits
    /// (mod.rs:187-193) — but each user message spawns a fresh `run_inner`
    /// and a fresh tracker that started at zero, so a $1 session cap gave
    /// every message its own independent $1 (#88 audit H6: the guard was
    /// effectively per-run, letting a session burn N× the configured cap).
    /// Seeding with `chat_state.total_usage` makes the enforcement
    /// cumulative across messages within the session.
    pub fn with_config_seeded(config: BudgetConfig, prior_usage: &TokenUsage) -> Self {
        let mut tracker = Self::with_config(config);
        tracker.record_usage(prior_usage);
        tracker
    }

    /// Start a new turn.
    pub fn begin_turn(&mut self) {
        self.current_turn += 1;
    }

    /// Record usage from a model response.
    pub fn record_usage(&mut self, usage: &TokenUsage) {
        self.total_tokens += usage.total();
        self.total_cost += self.config.pricing.cost(usage);
    }

    /// Record usage priced by the ACTUAL model that produced it — distillation
    /// routing can run a light turn on flash inside a pro session, so the
    /// cost estimate must follow the per-request model, not the session's.
    /// `pricing` is resolved by the caller from the model catalog (or the
    /// heuristic fallback) so the guard uses the real per-model rate.
    pub fn record_usage_for_model(&mut self, usage: &TokenUsage, pricing: &TokenPricing) {
        self.total_tokens += usage.total();
        self.total_cost += pricing.cost(usage);
    }

    /// Whether the max turns limit is reached.
    pub fn max_turns_reached(&self) -> bool {
        self.current_turn >= self.config.max_turns
    }

    /// Whether the session token budget is exceeded.
    pub fn session_token_limit_reached(&self) -> bool {
        self.config.session_token_limit > 0 && self.total_tokens >= self.config.session_token_limit
    }

    /// Whether the session cost budget is exceeded.
    pub fn session_cost_limit_reached(&self) -> bool {
        self.config.session_cost_limit > 0.0 && self.total_cost >= self.config.session_cost_limit
    }

    /// Whether the wall-clock budget expired.
    pub fn wall_clock_expired(&self) -> bool {
        self.config.max_wall_clock_secs.is_some_and(|secs| {
            self.started_at.elapsed().as_secs() >= secs
        })
    }

    /// Whether any budget limit is reached (turns, tokens, or cost).
    pub fn budget_exceeded(&self) -> bool {
        self.max_turns_reached()
            || self.session_token_limit_reached()
            || self.session_cost_limit_reached()
            || self.wall_clock_expired()
    }

    /// Whether the loop should continue to the next turn.
    pub fn should_continue(&self) -> bool {
        !self.budget_exceeded()
    }

    /// Which budget limit tripped, for the replay event log. Order matters:
    /// `turns` is the common "ran out of iterations" fuse, so it wins over
    /// the session-scoped limits when several are over at once.
    pub fn exceeded_reason(&self) -> Option<&'static str> {
        if self.max_turns_reached() {
            Some("turns")
        } else if self.session_token_limit_reached() {
            Some("tokens")
        } else if self.session_cost_limit_reached() {
            Some("cost")
        } else if self.wall_clock_expired() {
            Some("wall_clock")
        } else {
            None
        }
    }

    /// Get the current turn number.
    pub fn current_turn(&self) -> u32 {
        self.current_turn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_turns_reached() {
        let mut tracker = BudgetTracker::with_config(BudgetConfig {
            max_turns: 3,
            ..Default::default()
        });
        tracker.begin_turn();
        tracker.begin_turn();
        tracker.begin_turn();
        assert!(tracker.max_turns_reached());
        assert!(!tracker.should_continue());
    }

    #[test]
    fn session_token_limit_reached() {
        let config = BudgetConfig {
            max_turns: 100,
            session_token_limit: 1000,
            session_cost_limit: 0.0,
            max_wall_clock_secs: None,
            pricing: TokenPricing::deepseek_pro(),
        };
        let mut tracker = BudgetTracker::with_config(config);
        let usage = TokenUsage {
            prompt_tokens: 600,
            completion_tokens: 500,
            ..Default::default()
        };
        tracker.record_usage(&usage);
        assert!(tracker.session_token_limit_reached());
        assert!(tracker.budget_exceeded());
    }

    #[test]
    fn session_cost_limit_reached() {
        let config = BudgetConfig {
            max_turns: 100,
            session_token_limit: 0,
            session_cost_limit: 0.01, // 1 cent
            max_wall_clock_secs: None,
            pricing: TokenPricing::deepseek_pro(),
        };
        let mut tracker = BudgetTracker::with_config(config);
        // 10k prompt + 10k completion tokens = ~0.012 USD (exceeds 0.01)
        let usage = TokenUsage {
            prompt_tokens: 10_000,
            completion_tokens: 10_000,
            ..Default::default()
        };
        tracker.record_usage(&usage);
        assert!(tracker.session_cost_limit_reached());
        assert!(tracker.budget_exceeded());
    }

    #[test]
    fn zero_limits_are_unlimited() {
        let config = BudgetConfig {
            max_turns: 100,
            session_token_limit: 0,
            session_cost_limit: 0.0,
            max_wall_clock_secs: None,
            pricing: TokenPricing::deepseek_pro(),
        };
        let mut tracker = BudgetTracker::with_config(config);
        let usage = TokenUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 1_000_000,
            ..Default::default()
        };
        tracker.record_usage(&usage);
        assert!(!tracker.session_token_limit_reached());
        assert!(!tracker.session_cost_limit_reached());
    }

    #[test]
    fn cost_estimation_is_reasonable() {
        let pricing = TokenPricing::deepseek_pro();
        let usage = TokenUsage {
            prompt_tokens: 100_000,
            completion_tokens: 50_000,
            ..Default::default()
        };
        let cost = pricing.cost(&usage);
        // 100k prompt * $0.40/M = $0.04
        // 50k completion * $0.80/M = $0.04
        // Total ≈ $0.08
        assert!(cost > 0.07 && cost < 0.09);
    }

    #[test]
    fn cost_discounts_cached_prompt_tokens() {
        let pricing = TokenPricing::deepseek_pro();
        let uncached = TokenUsage {
            prompt_tokens: 100_000,
            completion_tokens: 0,
            ..Default::default()
        };
        let cached = TokenUsage {
            prompt_tokens: 100_000,
            completion_tokens: 0,
            prompt_cache_hit_tokens: Some(90_000),
            ..Default::default()
        };
        let full = pricing.cost(&uncached);
        let discounted = pricing.cost(&cached);
        assert!(
            discounted < full,
            "cached prompt tokens must be billed at a discount: {discounted} vs {full}"
        );
        assert!(
            discounted > full * 0.05,
            "the discount must not become a full write-off: {discounted}"
        );
        // A fully cached prompt bills only the discounted rate.
        let all_cached = TokenUsage {
            prompt_tokens: 100_000,
            completion_tokens: 0,
            prompt_cache_hit_tokens: Some(100_000),
            ..Default::default()
        };
        assert!(
            pricing.cost(&all_cached) < full * 0.5,
            "a fully cached prompt must be far cheaper than a cold one"
        );
    }

    #[test]
    fn seeded_budget_counts_prior_session_usage() {
        // Session limits must be cumulative across user messages: a $0.01
        // cap with $0.02 already spent must trip immediately on the next
        // message — not get a fresh $0.01 budget (#88 audit H6).
        let config = BudgetConfig {
            max_turns: 100,
            session_token_limit: 0,
            session_cost_limit: 0.01,
            max_wall_clock_secs: None,
            pricing: TokenPricing::deepseek_pro(),
        };
        let prior = TokenUsage {
            prompt_tokens: 10_000,
            completion_tokens: 10_000, // ≈ $0.012 already spent
            ..Default::default()
        };
        let tracker = BudgetTracker::with_config_seeded(config.clone(), &prior);
        // No new usage recorded — the seed alone must trip the cap.
        assert!(
            tracker.session_cost_limit_reached(),
            "prior session usage must count toward the session cap"
        );
        assert!(!tracker.should_continue());

        // And with zero prior usage the same config still allows work.
        let fresh = BudgetTracker::with_config_seeded(config, &TokenUsage::default());
        assert!(fresh.should_continue());
    }

    #[test]
    fn wall_clock_timeout_stops_the_loop() {
        let config = BudgetConfig {
            max_turns: 100,
            session_token_limit: 0,
            session_cost_limit: 0.0,
            max_wall_clock_secs: Some(0), // expires immediately
            pricing: TokenPricing::deepseek_pro(),
        };
        let tracker = BudgetTracker::with_config(config);
        assert!(tracker.wall_clock_expired());
        assert!(tracker.budget_exceeded());
        assert!(!tracker.should_continue());
    }

    #[test]
    fn exceeded_reason_reports_the_tripped_limit() {
        let turns = BudgetTracker::with_config(BudgetConfig {
            max_turns: 1,
            ..Default::default()
        });
        assert_eq!(turns.exceeded_reason(), None);
        let mut turns = turns;
        turns.begin_turn();
        assert_eq!(turns.exceeded_reason(), Some("turns"));

        let wall = BudgetTracker::with_config(BudgetConfig {
            max_wall_clock_secs: Some(0),
            ..Default::default()
        });
        assert_eq!(wall.exceeded_reason(), Some("wall_clock"));
    }

    #[test]
    fn flash_class_models_use_flash_pricing() {
        assert_eq!(
            TokenPricing::for_model("deepseek-v4-pro").prompt_per_million,
            TokenPricing::deepseek_pro().prompt_per_million
        );
        assert_eq!(
            TokenPricing::for_model("deepseek-v4-flash").prompt_per_million,
            TokenPricing::deepseek_flash().prompt_per_million
        );
        assert_eq!(
            TokenPricing::for_model("some-provider/mini-1.5").prompt_per_million,
            TokenPricing::deepseek_flash().prompt_per_million
        );
        // Unknown/premium names must not be silently priced as cheap —
        // the cost guard errs toward the expensive rate.
        assert_eq!(
            TokenPricing::for_model("gpt-4-turbo").prompt_per_million,
            TokenPricing::deepseek_pro().prompt_per_million
        );
    }
}
