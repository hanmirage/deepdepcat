//! Intent + complexity-driven reasoning-effort selection for DeepSeek's
//! "auto" (自动优化) mode.
//!
//! The `auto` reasoning mode used to map unconditionally to `max` — every
//! request paid for a full-strength thinking chain, even a greeting. This
//! module turns `auto` into an actual optimization:
//! - heavy work (coding, debugging, planning, docs, review) keeps `max`
//!   ONLY for genuinely large (High-complexity) tasks; medium/small heavy
//!   work drops to `high` (a real session spent 27k reasoning tokens on a
//!   single-file CSS polish task — that is medium work, not max work);
//! - light turns (chat, questions, exploration) drop to the cheapest
//!   tier (`low` on flash, `high` on pro).
//!
//! DeepSeek effort mapping (V4 docs): `flash` honors low/high/max; `pro`
//! currently folds `low` into `high`, so lowering effort only saves money on
//! flash. We never fully disable thinking here — thinking mode is on by
//! default at the API, and turning it off (thinking:disabled) trades tool
//! accuracy for tokens, which the plan deliberately avoids.

use crate::agent::intent::{TaskComplexity, UserIntent};

/// Resolve the effective DeepSeek effort for an auto-mode request.
///
/// Heavy intents keep `max` only for High complexity; Medium/Low heavy work
/// drops to `high` (both models honor it) — the cost/quality sweet spot.
/// Light intents drop to the cheapest effort the model actually honors:
/// `low` on flash, `high` on pro (pro has no working low tier).
pub fn intent_effort(
    intent: UserIntent,
    is_pro: bool,
    complexity: TaskComplexity,
) -> Option<String> {
    let heavy = matches!(
        intent,
        UserIntent::CodingTask
            | UserIntent::DebuggingTask
            | UserIntent::Documentation
            | UserIntent::Planning
            | UserIntent::Review
            | UserIntent::Research
            | UserIntent::ContentCreation
    );
    if heavy {
        return Some(if complexity == TaskComplexity::High {
            "max".to_string()
        } else {
            "high".to_string()
        });
    }
    Some(if is_pro {
        "high".to_string()
    } else {
        "low".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_turns_lower_effort_on_flash() {
        assert_eq!(
            intent_effort(UserIntent::Chat, false, TaskComplexity::Low),
            Some("low".to_string())
        );
        assert_eq!(
            intent_effort(UserIntent::Question, false, TaskComplexity::Low),
            Some("low".to_string())
        );
        assert_eq!(
            intent_effort(UserIntent::Exploration, false, TaskComplexity::Medium),
            Some("low".to_string())
        );
    }

    #[test]
    fn light_turns_keep_high_on_pro() {
        // Pro folds low→high, so lowering is pointless — keep high.
        assert_eq!(
            intent_effort(UserIntent::Chat, true, TaskComplexity::Low),
            Some("high".to_string())
        );
        assert_eq!(
            intent_effort(UserIntent::Exploration, true, TaskComplexity::High),
            Some("high".to_string())
        );
    }

    #[test]
    fn high_complexity_heavy_turns_keep_max_on_both_tiers() {
        for is_pro in [false, true] {
            assert_eq!(
                intent_effort(
                    UserIntent::CodingTask,
                    is_pro,
                    TaskComplexity::High
                ),
                Some("max".to_string())
            );
            assert_eq!(
                intent_effort(
                    UserIntent::DebuggingTask,
                    is_pro,
                    TaskComplexity::High
                ),
                Some("max".to_string())
            );
            assert_eq!(
                intent_effort(
                    UserIntent::Planning,
                    is_pro,
                    TaskComplexity::High
                ),
                Some("max".to_string())
            );
            assert_eq!(
                intent_effort(
                    UserIntent::Documentation,
                    is_pro,
                    TaskComplexity::High
                ),
                Some("max".to_string())
            );
            assert_eq!(
                intent_effort(UserIntent::Review, is_pro, TaskComplexity::High),
                Some("max".to_string())
            );
            assert_eq!(
                intent_effort(UserIntent::Research, is_pro, TaskComplexity::High),
                Some("max".to_string())
            );
            assert_eq!(
                intent_effort(
                    UserIntent::ContentCreation,
                    is_pro,
                    TaskComplexity::High
                ),
                Some("max".to_string())
            );
        }
    }

    #[test]
    fn medium_or_low_complexity_heavy_turns_drop_to_high() {
        // Real session evidence: a single-file CSS polish task (medium
        // work) burned 27k reasoning tokens at max. Medium/Low heavy work
        // now caps at high on BOTH tiers.
        for is_pro in [false, true] {
            for complexity in [TaskComplexity::Low, TaskComplexity::Medium] {
                assert_eq!(
                    intent_effort(UserIntent::CodingTask, is_pro, complexity),
                    Some("high".to_string())
                );
                assert_eq!(
                    intent_effort(UserIntent::DebuggingTask, is_pro, complexity),
                    Some("high".to_string())
                );
            }
        }
    }

    #[test]
    fn complexity_does_not_affect_light_turns() {
        // Light intents stay cheap regardless of complexity estimates —
        // a long question is still just a question.
        for complexity in [TaskComplexity::Low, TaskComplexity::Medium, TaskComplexity::High] {
            assert_eq!(
                intent_effort(UserIntent::Question, false, complexity),
                Some("low".to_string())
            );
        }
    }
}
