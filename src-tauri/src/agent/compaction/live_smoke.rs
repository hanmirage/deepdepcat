//! REAL DeepSeek compaction smoke (ignored by default — requires a live
//! `DEEPSEEK_API_KEY`).
//!
//! Compaction is a core recovery path (threshold passes + prompt-too-long
//! emergency), but until now only mocked-summary unit tests covered it.
//! This runs the REAL summarizer model over a generated conversation and
//! asserts the production quality guards accept the result.
//!
//! Run:
//! `cargo test --lib -- --ignored real_deepseek_compaction_smoke
//! --nocapture` with `DEEPSEEK_API_KEY` set.

use crate::agent::chat_state::ChatState;
use crate::agent::compaction::{templates, Compactor};
use crate::core::config::ProviderConfig;
use crate::core::types::ConversationItem;
use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::llm::client::LlmClient;
use crate::llm::retry::RetryConfig;
use std::sync::Arc;

fn live_client(key: String) -> LlmClient {
    let provider = ProviderConfig {
        name: "deepseek".to_string(),
        api_key_env: String::new(),
        api_key: Some(key),
        base_url: "https://api.deepseek.com/v1".to_string(),
        enabled: true,
        protocol: None,
    };
    LlmClient::new(
        vec![provider],
        RetryConfig {
            max_retries: 1,
            base_delay: std::time::Duration::from_millis(300),
            max_delay: std::time::Duration::from_secs(3),
            fallback_models: vec![],
        },
        true,
        Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            open_timeout_secs: 10,
        })),
    )
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY"]
async fn real_deepseek_compaction_smoke() {
    let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("SKIP: DEEPSEEK_API_KEY not set");
        return;
    };

    let compactor = Compactor::new(live_client(key), "deepseek-v4-flash");
    // Build a ~10k-token conversation — comfortably above the 4096
    // emergency target so the compaction pass has real work to do.
    let build_conversation = || {
        let mut cs = ChatState::new("deepseek-v4-flash", 200_000);
        for i in 0..80 {
            cs.push_user_message(format!(
                "任务 {i}: 请实现一个带参数校验的 HTTP 接口，包含错误处理、超时重试和单元测试，并说明边界条件。"
            ));
            cs.push_assistant_message(
                format!(
                    "已完成任务 {i}：实现接口 src/api_{i}.rs、补充测试、处理超时与边界。注意输入校验与空值场景。"
                ),
                vec![],
                None,
                None,
            );
        }
        cs
    };

    // The summarizer model can transiently produce a rejected/degenerate
    // summary (internal attempts already retry; this adds a whole-pass
    // retry). A real pipeline regression fails both attempts.
    let mut cs = ChatState::new("deepseek-v4-flash", 200_000);
    let mut compacted = None;
    for attempt in 1..=2 {
        let mut attempt_cs = build_conversation();
        match compactor.compact_with_budget(&mut attempt_cs, 4096, 500, None, None).await {
            Ok(Some(tokens)) if tokens > 0 => {
                cs = attempt_cs;
                compacted = Some(tokens);
                break;
            }
            Ok(_) => eprintln!("[compaction-smoke] attempt {attempt}: nothing compactable — re-running"),
            Err(e) => eprintln!("[compaction-smoke] attempt {attempt}: {e}"),
        }
    }
    let compacted = compacted.expect("compaction must succeed within 2 attempts");
    assert!(compacted > 0, "must free tokens");

    // Production quality guards accept the summary (not degenerate/empty).
    let first = cs.conversation.first().expect("summary item");
    match first {
        ConversationItem::System(s) => {
            assert_eq!(
                templates::classify_summary(&s.content),
                templates::SummaryQuality::Ok,
                "summary must pass the production quality guard"
            );
        }
        other => panic!("first item after compaction must be the summary, got: {other:?}"),
    }

    // The original user intents survive (preamble or body).
    let all_text: String = cs
        .conversation
        .iter()
        .map(|item| match item {
            ConversationItem::System(s) => s.content.clone(),
            ConversationItem::User(u) => u
                .content
                .iter()
                .filter_map(|p| match p {
                    crate::core::types::ContentPart::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all_text.contains("HTTP") || all_text.contains("任务"),
        "user intent must survive compaction"
    );
    eprintln!(
        "[compaction-smoke] compacted={compacted} tokens, items now={}",
        cs.conversation.len()
    );
}
