//! REAL DeepSeek memory smokes (ignored by default — require a live
//! `DEEPSEEK_API_KEY`): learning extraction, procedure capture, and dream
//! synthesis against the production prompts.
//!
//! The self-evolution path previously had only parser unit tests; these
//! run the real prompts over crafted sessions and assert the production
//! parsers/guards accept the results.

use crate::core::config::ProviderConfig;
use crate::core::types::{AssistantMessage, ConversationItem};
use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::llm::client::LlmClient;
use crate::llm::retry::RetryConfig;
use crate::memory::dream::{DreamConfig, DreamEngine};
use crate::memory::learning::{extract_learnings, MAX_LEARNINGS_PER_TURN};
use crate::memory::procedure_capture::capture_procedure;
use crate::memory::store::MemoryStore;
use crate::storage::database::Database;
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
async fn real_deepseek_dream_smoke() {
    let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("SKIP: DEEPSEEK_API_KEY not set");
        return;
    };

    let tmp = tempfile::tempdir().expect("temp dir");
    let db = Arc::new(
        Database::open(&tmp.path().join("dream.db"), false).expect("open db"),
    );
    let _ = db.run_migrations();
    let store = Arc::new(MemoryStore::new(db));
    // Three related raw memories — a consolidation cycle must cluster them
    // into at least one synthesized summary.
    store
        .store(
            "项目用 shadow 插件打 fat jar，来自 maven-shade 迁移",
            "project",
            None,
            None,
        )
        .expect("store");
    store
        .store(
            "shadow 插件需要单独配置 ServiceLoader 合并规则",
            "project",
            None,
            None,
        )
        .expect("store");
    store
        .store(
            "迁移后用 ./gradlew test 与 ./gradlew build 验证产物",
            "project",
            None,
            None,
        )
        .expect("store");

    let engine = DreamEngine::new(store, live_client(key), "deepseek-v4-flash").with_config(
        DreamConfig {
            enabled: true,
            min_hours: 0,
            min_memories: 1,
            batch_size: 50,
            decay_originals: false,
        },
    );
    let result = engine.dream().await.expect("dream must succeed");
    assert!(result.source_count >= 3, "all raw memories processed");
    assert!(
        result.synthesized_count >= 1,
        "related memories must synthesize at least one summary"
    );
    assert!(
        result.summaries.iter().all(|s| !s.trim().is_empty()),
        "summaries must be substantive"
    );
    eprintln!(
        "[dream-smoke] source={} synthesized={}",
        result.source_count, result.synthesized_count
    );
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY"]
async fn real_deepseek_learning_extraction_smoke() {
    let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("SKIP: DEEPSEEK_API_KEY not set");
        return;
    };

    // A session with genuinely non-obvious findings — the extraction prompt
    // should surface them (hidden build quirk, misleading error, co-change
    // constraint).
    let conversation = vec![
        ConversationItem::user(
            "构建一直报 'linker not found'，但 gcc 明明装了。查一下为什么。",
        ),
        ConversationItem::Assistant(AssistantMessage {
            content: "找到原因：项目 .cargo/config.toml 把 linker 指向了 \
                      /opt/llvm/bin/clang，而那个路径在 CI 上不存在。\
                      真实链接器是系统 gcc。"
                .to_string(),
            tool_calls: vec![],
            model: None,
            usage: None,
            reasoning_content: None,
        }),
        ConversationItem::user("改掉它，并确认本地与 CI 都过。"),
        ConversationItem::Assistant(AssistantMessage {
            content: "已改：移除 .cargo/config.toml 中过期的 linker 覆盖；\
                      本地 cargo build 通过。CI 的 cache 里还有旧 target，\
                      需要手动清一次，否则下次仍会复现。"
                .to_string(),
            tool_calls: vec![],
            model: None,
            usage: None,
            reasoning_content: None,
        }),
    ];

    // Extraction is sampling-dependent: the model may legitimately answer
    // NO_LEARNINGS on any single attempt (production retries across turns).
    // Re-sample up to 4 times and require at least one substantive result —
    // a total pipeline failure (prompt broken, parser dead) still fails.
    let client = live_client(key);
    let mut learnings = Vec::new();
    for attempt in 1..=4 {
        let batch = extract_learnings(
            &client,
            "deepseek-v4-flash",
            Some("deepseek"),
            &conversation,
        )
        .await
        .expect("extraction call must succeed");
        if !batch.is_empty() {
            learnings = batch;
            break;
        }
        eprintln!("[learning-smoke] attempt {attempt}/4 returned no learnings — re-sampling");
    }
    assert!(
        learnings.len() <= MAX_LEARNINGS_PER_TURN,
        "at most {MAX_LEARNINGS_PER_TURN} learnings, got {}",
        learnings.len()
    );
    assert!(
        !learnings.is_empty(),
        "the crafted session must yield non-obvious learnings"
    );
    for item in &learnings {
        assert!(
            item.chars().count() <= 200,
            "learning must stay bounded: {item}"
        );
    }
    eprintln!("[learning-smoke] {} learnings: {learnings:?}", learnings.len());
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY"]
async fn real_deepseek_procedure_capture_smoke() {
    let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("SKIP: DEEPSEEK_API_KEY not set");
        return;
    };

    // A task with a clear repeatable multi-step process and a verified
    // outcome — exactly what the procedure prompt should capture.
    let conversation = vec![
        ConversationItem::user("把项目从 Maven 构建迁移到 Gradle，跑通全部测试。"),
        ConversationItem::Assistant(AssistantMessage {
            content: "开始迁移：先读取 pom.xml 与现有目录结构，确认依赖树与插件。".to_string(),
            tool_calls: vec![],
            model: None,
            usage: None,
            reasoning_content: None,
        }),
        ConversationItem::Assistant(AssistantMessage {
            content: "迁移完成，过程如下：\
                      第一步，读取 pom.xml 导出依赖坐标、版本与插件清单；\
                      第二步，生成 build.gradle.kts 并把依赖逐个迁入，保留原版本；\
                      第三步，把 maven-shade 插件迁移为 shadow 插件并配置合并规则；\
                      第四步，在 sourceSets 显式声明 resources 过滤规则（与 Maven 不同）；\
                      第五步，运行 ./gradlew test 全部通过，./gradlew build 产物可运行。".to_string(),
            tool_calls: vec![],
            model: None,
            usage: None,
            reasoning_content: None,
        }),
    ];

    let procedure = capture_procedure(
        &live_client(key),
        "deepseek-v4-flash",
        Some("deepseek"),
        &conversation,
        "code",
    )
    .await
    .expect("the crafted session must yield a procedure");
    let procedure = procedure.normalized();
    eprintln!(
        "[procedure-smoke] name={} steps={} verify={} lessons={} raw={procedure:?}",
        procedure.name,
        procedure.steps.len(),
        procedure.verify.len(),
        procedure.lessons.len()
    );
    assert!(!procedure.name.is_empty(), "procedure must have a name");
    assert!(
        procedure.steps.len() >= 3,
        "procedure must keep ordered steps, got {}",
        procedure.steps.len()
    );
    assert!(
        !procedure.verify.is_empty(),
        "procedure must record what counts as verified"
    );
}
