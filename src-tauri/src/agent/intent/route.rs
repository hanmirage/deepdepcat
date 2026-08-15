//! LLM routing — one bounded call that upgrades an uncertain heuristic.
use super::classify::*;
use super::signals::*;
use super::types::*;


/// Whether the heuristic result is worth upgrading with one bounded LLM
/// routing call.
///
/// Any substantive message (≥16 chars) gets a single routing call: for
/// Chat/Question it rescues a likely misclassification, and for actionable
/// work it produces the complexity/planning/delegation decision that steers
/// the whole turn. Short messages are unambiguous (greetings, one-word
/// questions) — the heuristic is enough. Light single-purpose actionable
/// work also skips the call: the heuristic decision is already unambiguous
/// (low complexity, direct execution, no planning / no subagents) and is
/// exactly the fallback the router uses on failure — routing would only
/// confirm it at the cost of an extra LLM round trip and up to 4s of
/// sequential latency on the most common kind of request.
pub fn needs_llm_routing(message: &str, intent: UserIntent) -> bool {
    // A short code-EXECUTION request with large-scope wording ("全面重构数据层",
    // 7 chars) is high-scope but the raw heuristic under-rates it — route it.
    // Planning ("给一个重构方案") is excluded: the heuristic already labels it
    // correctly and its deliverable IS a plan, so routing only re-confirms it.
    let large_scope = matches!(
        intent,
        UserIntent::CodingTask | UserIntent::DebuggingTask
    ) && has_large_scope_wording(message);
    // Short messages are unambiguous (greetings, one-word questions) — EXCEPT
    // the large-scope code case above.
    if message.chars().count() < 16 && !large_scope {
        return false;
    }
    // Routing is only worth it when the text has real substance.
    let substantive = message.contains(char::is_alphabetic)
        && (message.chars().filter(|c| !c.is_whitespace()).count() >= 12 || large_scope);
    if !substantive {
        return false;
    }
    // No-upside case: a small single-purpose edit's execution profile is
    // already deterministic from the heuristic — skip the extra call.
    if intent.is_actionable() && light_task_signal(message, intent) {
        return false;
    }
    matches!(
        intent,
        UserIntent::Chat
            | UserIntent::Question
            | UserIntent::CodingTask
            | UserIntent::DebuggingTask
            | UserIntent::Documentation
            | UserIntent::Planning
            | UserIntent::Review
            | UserIntent::Exploration
            | UserIntent::Research
            | UserIntent::ContentCreation
    )
}

/// Upgrade an uncertain heuristic classification with one small LLM call.
///
/// Fires only when the heuristic is uncertain (`needs_llm_routing`), is
/// bounded by a short timeout, and falls back to the heuristic decision on
/// any failure (network, parse, timeout) — the fast path never regresses.
///
/// `model` / `provider` come from the session's chat state so the call routes
/// to the same configured model as the main loop. Returns the full routing
/// decision (intent + complexity + planning/subagent needs) plus the usage
/// of the routing call itself — the caller records it into the session
/// accounting so per-message routing tokens are not invisible to usage
/// stats, the session budget, or the cost/token limits (audit H7 residual).
pub async fn route_with_llm(
    llm: &crate::llm::client::LlmClient,
    message: &str,
    model: &str,
    provider: Option<&str>,
    current: UserIntent,
) -> (IntentDecision, crate::core::types::TokenUsage) {
    let fallback = heuristic_decision(message, current);
    let request = crate::llm::provider::LlmRequest {
        model: model.to_string(),
        provider: provider.map(|s| s.to_string()),
        messages: vec![
            crate::core::types::ConversationItem::system(
                "Classify the user message and return ONLY one JSON object, no \
                 prose:\n\
                 {\"intent\": \"chat|question|exploration|coding_task|debugging_task|\
                 documentation|planning|review|research|content_creation\", \
                 \"complexity\": \"low|medium|high\", \
                 \"needs_planning\": true|false, \
                 \"needs_subagents\": true|false}\n\
                 Intent rules:\n\
                 - coding_task: wants code written/edited/built/refactored\n\
                 - debugging_task: reports an error/bug/failure or wants it fixed\n\
                 - exploration: wants to understand the project/codebase first\n\
                 - documentation: wants docs/comments/README written\n\
                 - planning: wants a plan/design/proposal as the OUTPUT\n\
                 - review: wants existing code reviewed\n\
                 - research: wants investigation/source gathering (调研/文献/市场)\n\
                 - content_creation: wants creative or content output (文案/脚本/PPT/卡片)\n\
                 - question: asks a question without requesting code work\n\
                 - chat: casual talk, greeting, thanks\n\
                 Complexity rules:\n\
                 - low: one file / one concern / a simple question\n\
                 - medium: multiple parts, still tractable in one pass\n\
                 - high: large cross-module work, many independent parts\n\
                 needs_planning: true ONLY when the user asks for concrete \
                 multi-step implementation work — the agent should lay out a \
                 todo plan before touching files. false for pure plans, \
                 reviews, questions, and single-file edits.\n\
                 needs_subagents: true ONLY for genuinely large work with \
                 several INDEPENDENT parts that parallel subagents would \
                 speed up. false for sequential/dependent work and small tasks.\n\
                 When in doubt, prefer the SMALLER answer (low / false) — \
                 over-delegation costs tokens and over-planning wastes a turn.",
            ),
            crate::core::types::ConversationItem::user(message.to_string()),
        ],
        tools: vec![],
        system_prompt: String::new(),
        temperature: Some(0.0),
        top_p: None,
        max_tokens: Some(64),
        stream: false,
        reasoning_effort: None,
        response_format: Some(crate::llm::provider::ResponseFormat::JsonObject),
        cache_control: None,
        user_id: None,
    };

    // Bounded routing — a slow/failed call falls back to the heuristic so
    // the turn is never delayed by the classifier.
    let parsed = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        crate::llm::provider::LlmProvider::complete(llm, &request),
    )
    .await;

    let Ok(Ok(response)) = parsed else {
        return (fallback, crate::core::types::TokenUsage::default());
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&response.content) else {
        return (fallback, crate::core::types::TokenUsage::default());
    };

    let intent = match json.get("intent").and_then(|v| v.as_str()) {
        Some("chat") => UserIntent::Chat,
        Some("question") => UserIntent::Question,
        Some("exploration") => UserIntent::Exploration,
        Some("coding_task") => UserIntent::CodingTask,
        Some("debugging_task") => UserIntent::DebuggingTask,
        Some("documentation") => UserIntent::Documentation,
        Some("planning") => UserIntent::Planning,
        Some("review") => UserIntent::Review,
        Some("research") => UserIntent::Research,
        Some("content_creation") => UserIntent::ContentCreation,
        _ => current,
    };
    let complexity = match json.get("complexity").and_then(|v| v.as_str()) {
        Some("medium") => TaskComplexity::Medium,
        Some("high") => TaskComplexity::High,
        _ => TaskComplexity::Low,
    };
    // The LLM may misjudge scale; trust it only within the heuristic's own
    // complexity envelope so a small task can never be over-delegated.
    let signals = task_complexity_signal(message, intent);
    let needs_subagents = json
        .get("needs_subagents")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && (signals >= 1 || complexity == TaskComplexity::High);
    let needs_planning = json
        .get("needs_planning")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && intent != UserIntent::Planning
        && intent.is_actionable();

    (
        IntentDecision {
            intent,
            complexity,
            needs_planning,
            needs_subagents,
            multi_intent: split_sub_asks(message).len() > 1,
        },
        response.usage,
    )
}
