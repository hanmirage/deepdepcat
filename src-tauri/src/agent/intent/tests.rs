//! Intent tests.

use super::*;

    #[test]
    fn greeting_is_chat() {
        assert_eq!(classify("你好").intent, UserIntent::Chat);
        assert_eq!(classify("thanks!").intent, UserIntent::Chat);
    }

    #[test]
    fn greeting_does_not_fire_on_substrings_or_actions() {
        // "hi" inside "which" must not be read as a greeting — this is a
        // question ("which" is a question word).
        assert_eq!(classify("which file").intent, UserIntent::Question);
        // "ok" followed by an action is not a greeting.
        assert_ne!(classify("ok fix the bug").intent, UserIntent::Chat);
        // A pure greeting still is.
        assert_eq!(classify("hi").intent, UserIntent::Chat);
        assert_eq!(classify("ok thanks").intent, UserIntent::Chat);
    }

    #[test]
    fn script_phrased_code_request_is_not_content_creation() {
        // "脚本"/"script" are ambiguous (screenplay vs code script); a
        // code-language qualifier must route to CodingTask, not Depwork
        // content_creation.
        assert_eq!(
            classify("帮我写个 python 脚本定时清理日志").intent,
            UserIntent::CodingTask
        );
        assert_eq!(
            classify("写个 shell script 处理数据").intent,
            UserIntent::CodingTask
        );
    }

    #[test]
    fn question_is_question() {
        assert_eq!(classify("什么是闭包？").intent, UserIntent::Question);
        assert_eq!(
            classify("how does tauri work?").intent,
            UserIntent::Question
        );
    }

    #[test]
    fn code_request_is_coding_task() {
        assert_eq!(
            classify("帮我写一个 Rust 的斐波那契函数").intent,
            UserIntent::CodingTask
        );
        assert_eq!(
            classify("implement the sort function in src/utils.rs").intent,
            UserIntent::CodingTask
        );
    }

    #[test]
    fn code_block_is_coding_task() {
        assert_eq!(
            classify("看看这段：```rust\nfn main() {}\n``` 有什么问题").intent,
            UserIntent::DebuggingTask
        );
    }

    #[test]
    fn bug_report_is_debugging() {
        assert_eq!(
            classify("为什么程序崩溃了？").intent,
            UserIntent::DebuggingTask
        );
        assert_eq!(
            classify("修复这个报错：TypeError: x is undefined").intent,
            UserIntent::DebuggingTask
        );
    }

    #[test]
    fn doc_request_is_documentation() {
        assert_eq!(
            classify("给这个项目写 README").intent,
            UserIntent::Documentation
        );
    }

    #[test]
    fn plan_request_is_planning() {
        assert_eq!(classify("给一个重构方案").intent, UserIntent::Planning);
    }

    #[test]
    fn review_request_is_review() {
        assert_eq!(classify("帮我审查这段代码").intent, UserIntent::Review);
    }

    #[test]
    fn exploration_is_exploration() {
        assert_eq!(
            classify("看看这个项目的结构").intent,
            UserIntent::Exploration
        );
    }

    #[test]
    fn actionable_intents_draft_goal() {
        let msg = "帮我写一个 Rust 的斐波那契函数，测试通过";
        let r = classify(msg);
        assert!(r.goal_draft.is_some());
        assert!(r.intent.is_actionable());
        let d = heuristic_decision(msg, r.intent);
        let spec = build_task_spec(&r, msg, &d).unwrap();
        assert!(spec.contains("<intent>coding_task</intent>"));
        assert!(spec.contains("<acceptance>tests pass</acceptance>"));
    }

    #[test]
    fn chat_has_no_task_spec() {
        let r = classify("你好");
        assert!(!r.intent.is_actionable());
        let d = heuristic_decision("你好", r.intent);
        assert!(build_task_spec(&r, "你好", &d).is_none());
    }

    #[test]
    fn task_spec_does_not_duplicate_goal() {
        // The goal lives ONLY in the per-request <current-goal> tail — a
        // second copy in task-spec would waste ~50-100 tokens per request
        // and go stale when update_goal changes it mid-run.
        let msg = "帮我写一个 Rust 的斐波那契函数，测试通过";
        let r = classify(msg);
        let d = heuristic_decision(msg, r.intent);
        let spec = build_task_spec(&r, msg, &d).unwrap();
        assert!(spec.contains("<intent>"));
        assert!(spec.contains("<acceptance>"));
        assert!(
            !spec.contains("<goal>"),
            "goal must not be duplicated into task-spec: {spec}"
        );
    }

    #[test]
    fn goal_draft_is_truncated() {
        let long = "帮我实现一个非常非常非常非常非常非常非常非常非常非常非常非常非常长的功能描述句子用来测试截断";
        let r = classify(long);
        assert!(r.goal_draft.unwrap().chars().count() <= 97);
    }

    #[test]
    fn complexity_signal_detects_numbered_sublist() {
        let msg = "帮我做三件事：\n1. 创建 API\n2. 写数据库层\n3. 接前端\n4. 写测试";
        assert_eq!(task_complexity_signal(msg, UserIntent::CodingTask), 1);
        assert!(suggest_decompose(msg, UserIntent::CodingTask).is_some());
    }

    #[test]
    fn split_sub_asks_handles_double_digit_numbers() {
        // "10. 11. 12." — the old single-digit strip dropped every item ≥ 10.
        let asks = split_sub_asks("10. 修复登录\n11. 修复注册\n12. 修复登出");
        assert_eq!(asks.len(), 3);
        assert!(asks.iter().any(|a| a.contains("修复登录")));
        // Mixed single- and double-digit lists keep all items (each item is
        // ≥2 chars to clear the substantive-segment filter).
        let mixed = split_sub_asks("1. 修登录\n2. 修注册\n10. 修登出");
        assert_eq!(mixed.len(), 3);
        assert!(mixed.iter().any(|a| a.contains("修登出")));
    }

    #[test]
    fn terse_create_request_gets_medium_budget() {
        // "写一个贪吃蛇小游戏" (12 chars, under the LLM-routing gate) must
        // NOT be classified Low — a real multi-file artifact needs the Medium
        // budget, not the 25-turn Low cap that truncated it mid-build.
        let msg = "写一个贪吃蛇小游戏 测试一下";
        let r = classify(msg);
        assert_eq!(r.intent, UserIntent::CodingTask);
        let d = heuristic_decision(msg, r.intent);
        assert_eq!(
            d.complexity,
            TaskComplexity::Medium,
            "a terse create request must get the Medium budget"
        );
        // Creating a large artifact is NOT a delegation signal.
        assert!(!d.needs_subagents);
    }

    #[test]
    fn small_edit_stays_low_even_with_artifact_word() {
        // A light EDIT that merely mentions an artifact word ("改一下游戏的
        // 颜色") must stay Low — the creation signal only fires on creating.
        let edit = heuristic_decision("改一下游戏的颜色", UserIntent::CodingTask);
        assert_eq!(edit.complexity, TaskComplexity::Low);
        // Non-actionable intents (chat/exploration) never escalate on the
        // artifact word either.
        let chat = heuristic_decision("打开游戏看看", UserIntent::Exploration);
        assert_eq!(chat.complexity, TaskComplexity::Low);
    }

    #[test]
    fn no_signal_for_small_tasks() {
        let msg = "修复 main.rs 里的 bug";
        assert_eq!(task_complexity_signal(msg, UserIntent::DebuggingTask), 0);
        assert!(suggest_decompose(msg, UserIntent::DebuggingTask).is_none());
    }

    #[test]
    fn short_large_scope_message_still_routes() {
        // A 7-char large-scope request must still route via LLM (the raw
        // heuristic would under-rate it to Low/no-planning/25 turns).
        assert!(needs_llm_routing("全面重构数据层", UserIntent::CodingTask));
        // A short unambiguous greeting does not route.
        assert!(!needs_llm_routing("你好", UserIntent::Chat));
    }

    #[test]
    fn light_task_marks_single_edit_requests() {
        // The production overreach case: one file, restyle wording.
        assert!(light_task_signal(
            "修改一下目录的html 让他像你的官网一样",
            UserIntent::CodingTask
        ));
        assert!(light_task_signal(
            "把 index.html 的标题改成新的",
            UserIntent::CodingTask
        ));
        assert!(light_task_signal(
            "update the README to mention the new flag",
            UserIntent::Documentation
        ));
        // Small fixes count too.
        assert!(light_task_signal(
            "修复一下登录页的样式问题",
            UserIntent::DebuggingTask
        ));
    }

    #[test]
    fn light_task_rejects_large_scope_and_new_work() {
        // Large-scope wording disqualifies.
        assert!(!light_task_signal(
            "重构整个项目的架构",
            UserIntent::CodingTask
        ));
        assert!(!light_task_signal(
            "迁移所有页面到新框架",
            UserIntent::CodingTask
        ));
        // Creating something new is not a light edit.
        assert!(!light_task_signal(
            "写一个完整的登录系统",
            UserIntent::CodingTask
        ));
        // Multi-part requests never get the light label.
        assert!(!light_task_signal(
            "修改 index.html 和 style.css 还有 main.js 三个文件",
            UserIntent::CodingTask
        ));
        // Pure Q&A is out of scope entirely.
        assert!(!light_task_signal("修改是什么意思？", UserIntent::Question));
    }

    #[test]
    fn file_ref_counting_is_anchored() {
        // `.h` must not double-count inside `index.html`; prose dots don't count.
        assert_eq!(count_file_refs("把 index.html 的标题改成新的"), 1);
        assert_eq!(count_file_refs("修改一下目录的html 让他像你的官网一样"), 0);
        assert_eq!(count_file_refs("改 index.html 和 style.css"), 2);
        assert_eq!(count_file_refs("update the README"), 1);
        assert_eq!(count_file_refs("版本是 3.14 而已"), 0);
    }

    #[test]
    fn chat_never_decomposes() {
        let msg = "1. 你好\n2. 谢谢\n3. 再见\n4. 辛苦了";
        assert_eq!(task_complexity_signal(msg, UserIntent::Chat), 0);
    }

    #[test]
    fn light_task_gets_direct_delegation_advice() {
        let msg = "把 index.html 的标题改成新的";
        let r = classify(msg);
        let d = heuristic_decision(msg, r.intent);
        assert_eq!(
            delegation_advice(&d, msg),
            Some((DelegationTier::Direct, DELEGATION_REASON_DIRECT))
        );
        let spec = build_task_spec(&r, msg, &d).unwrap();
        assert!(spec.contains("<delegation>direct</delegation>"), "{spec}");
    }

    #[test]
    fn multi_part_task_gets_parallel_2_3_advice() {
        let msg = "帮我做三件事：\n1. 创建 API\n2. 写数据库层\n3. 接前端\n4. 写测试";
        let r = classify(msg);
        let d = heuristic_decision(msg, r.intent);
        // The heuristic signal (≥1 numbered sub-list) meets the subagent
        // gate; the tier comes from the decision's complexity.
        assert_eq!(
            delegation_advice(&d, msg).map(|(t, _)| t),
            Some(DelegationTier::Parallel2_3)
        );
        let spec = build_task_spec(&r, msg, &d).unwrap();
        assert!(
            spec.contains("<delegation>parallel_2_3</delegation>"),
            "{spec}"
        );
        assert!(spec.contains("<planning_required>"), "{spec}");
    }

    #[test]
    fn large_task_gets_parallel_3_5_advice() {
        // Both complexity signals fire (numbered sub-list AND long
        // multi-clause message) → the largest delegation tier.
        let mut msg =
            String::from("帮我重构这个项目：\n1. 拆分 API 层\n2. 重写数据库层\n3. 重建前端结构");
        while msg.chars().count() < 420 {
            msg.push_str("，同时调整相关的错误处理路径，并且更新文档注释");
        }
        let r = classify(&msg);
        let d = heuristic_decision(&msg, r.intent);
        assert_eq!(
            delegation_advice(&d, &msg).map(|(t, _)| t),
            Some(DelegationTier::Parallel3_5)
        );
        let spec = build_task_spec(&r, &msg, &d).unwrap();
        assert!(
            spec.contains("<delegation>parallel_3_5</delegation>"),
            "{spec}"
        );
    }

    #[test]
    fn unremarkable_task_gets_no_delegation_advice() {
        // A plain review request with no complexity signals and no
        // light-task label: no advice beats noise.
        let msg = "帮我审查一下登录模块";
        let r = classify(msg);
        assert_eq!(r.intent, UserIntent::Review);
        let d = heuristic_decision(msg, r.intent);
        assert!(delegation_advice(&d, msg).is_none());
        let spec = build_task_spec(&r, msg, &d).unwrap();
        assert!(!spec.contains("<delegation>"), "{spec}");
    }

    #[test]
    fn non_actionable_gets_no_delegation_advice_legacy() {
        let d = IntentDecision::of(UserIntent::Chat);
        assert!(delegation_advice(&d, "你好").is_none());
        let d = IntentDecision::of(UserIntent::Question);
        assert!(delegation_advice(&d, "什么是闭包？").is_none());
    }

    /// REAL DeepSeek smoke test for the delegation advice — runs only when
    /// DEEPSEEK_API_KEY is set
    /// (`cargo test --lib -- --ignored real_deepseek_delegation_smoke --nocapture`).
    ///
    /// Verifies end-to-end with the live API: a large-task message whose
    /// `<task-spec>` carries a `<delegation>parallel_3_5</delegation>`
    /// suggestion must elicit a normal completion — the harness fragment
    /// must not confuse the model.
    #[tokio::test]
    #[ignore = "requires a real DEEPSEEK_API_KEY"]
    async fn real_deepseek_delegation_smoke() {
        use crate::core::config::ProviderConfig;
        use crate::core::types::ConversationItem;
        use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
        use crate::llm::client::LlmClient;
        use crate::llm::provider::{LlmProvider, LlmRequest};
        use crate::llm::retry::RetryConfig;
        use std::sync::Arc;

        let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        };

        let provider = ProviderConfig {
            name: "deepseek".to_string(),
            api_key_env: String::new(),
            api_key: Some(key),
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            protocol: None,
        };
        let client = LlmClient::new(
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
        );

        let mut msg =
            String::from("帮我重构这个项目：\n1. 拆分 API 层\n2. 重写数据库层\n3. 重建前端结构");
        while msg.chars().count() < 420 {
            msg.push_str("，同时调整相关的错误处理路径，并且更新文档注释");
        }
        let r = classify(&msg);
        let d = heuristic_decision(&msg, r.intent);
        let spec = build_task_spec(&r, &msg, &d).expect("actionable task must build a spec");
        assert!(
            spec.contains("<delegation>parallel_3_5</delegation>"),
            "delegation advice must be injected: {spec}"
        );

        let augmented = format!("{msg}\n\n{spec}");
        let req = LlmRequest {
            model: "deepseek-chat".to_string(),
            provider: Some("deepseek".to_string()),
            messages: vec![ConversationItem::user(augmented)],
            tools: vec![],
            system_prompt: String::new(),
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(120),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };
        let resp = client
            .complete(&req)
            .await
            .expect("live DeepSeek call with delegation advice must succeed");
        eprintln!("reply: {:?}", resp.content.trim());
        assert!(
            !resp.content.trim().is_empty(),
            "model must respond normally to a task-spec with delegation advice"
        );
    }

    /// A live DeepSeek client for the ignored smoke tests — cheap retry,
    /// tolerant circuit so a flaky network never wedges the test run.
    fn smoke_client() -> crate::llm::client::LlmClient {
        use crate::core::config::ProviderConfig;
        use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
        use crate::llm::client::LlmClient;
        use crate::llm::retry::RetryConfig;
        use std::sync::Arc;

        let key = std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set");
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

    /// REAL DeepSeek smoke test for the LLM router — runs only when
    /// DEEPSEEK_API_KEY is set
    /// (`cargo test --lib -- --ignored real_deepseek_route_smoke --nocapture`).
    ///
    /// Routes a representative mix (Chinese/English, fuzzy, explicit) through
    /// `route_with_llm` and verifies every decision builds a legal task-spec
    /// — the JSON contract must hold on real model output, and actionable
    /// decisions must not lose the task-spec.
    #[tokio::test]
    #[ignore = "requires a real DEEPSEEK_API_KEY"]
    async fn real_deepseek_route_smoke() {
        let Ok(_) = std::env::var("DEEPSEEK_API_KEY") else {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        };
        let client = smoke_client();

        let cases: &[(&str, UserIntent)] = &[
            (
                "帮我重构这个项目，拆分成三个独立模块，每个模块都要有完整的测试覆盖",
                UserIntent::CodingTask,
            ),
            (
                "这个项目的数据库层为什么要用 SQLite 而不是 PostgreSQL，能详细讲讲吗",
                UserIntent::Question,
            ),
            ("给一个重构方案，先别动手", UserIntent::Planning),
            (
                "我注意到项目里有几个模块的代码风格不太一致，想了解一下整体情况",
                UserIntent::Exploration,
            ),
            (
                "修复登录接口的 401 报错，改完跑一遍测试",
                UserIntent::DebuggingTask,
            ),
            (
                "update the README to mention the new flag and add a changelog entry",
                UserIntent::Documentation,
            ),
        ];

        for (msg, heuristic) in cases {
            let current = classify(msg).intent;
            assert_eq!(&current, heuristic, "heuristic baseline: {msg}");
            let (d, _usage) =
                route_with_llm(&client, msg, "deepseek-chat", Some("deepseek"), current).await;
            eprintln!(
                "route({msg:?}) -> intent={:?} complexity={:?} planning={} subagents={}",
                d.intent, d.complexity, d.needs_planning, d.needs_subagents
            );
            if d.intent.is_actionable() {
                let spec = build_task_spec(&classify(msg), msg, &d);
                assert!(
                    spec.is_some(),
                    "actionable decision must still build a task-spec: {msg}"
                );
            }
        }
    }

    /// REAL DeepSeek smoke: a task-spec carrying `<planning_required>` must
    /// elicit a plan-first response from the live model — not confusion
    /// (runs only when DEEPSEEK_API_KEY is set).
    #[tokio::test]
    #[ignore = "requires a real DEEPSEEK_API_KEY"]
    async fn real_deepseek_planning_required_smoke() {
        use crate::core::types::ConversationItem;
        use crate::llm::provider::{LlmProvider, LlmRequest};

        let Ok(_) = std::env::var("DEEPSEEK_API_KEY") else {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        };
        let client = smoke_client();

        let msg = "帮我做一个用户注册功能：后端接口、数据库表、前端表单，还要写测试";
        let r = classify(msg);
        // The heuristic cannot see the multi-part shape of a short message —
        // the LLM router can. Report its verdict, then force the gate to
        // verify the prompt's effect on the live model.
        let (routed, _usage) =
            route_with_llm(&client, msg, "deepseek-chat", Some("deepseek"), r.intent).await;
        eprintln!(
            "route({msg:?}) -> intent={:?} complexity={:?} planning={}",
            routed.intent, routed.complexity, routed.needs_planning
        );
        let mut d = IntentDecision::of(r.intent);
        d.needs_planning = true;
        d.complexity = TaskComplexity::Medium;
        let spec = build_task_spec(&r, msg, &d).expect("actionable task must build a spec");
        assert!(
            spec.contains("<planning_required>"),
            "multi-part task must carry the planning gate: {spec}"
        );

        let augmented = format!("{msg}\n\n{spec}");
        let req = LlmRequest {
            model: "deepseek-chat".to_string(),
            provider: Some("deepseek".to_string()),
            messages: vec![ConversationItem::user(augmented)],
            tools: vec![],
            system_prompt: String::new(),
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(200),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };
        let resp = client
            .complete(&req)
            .await
            .expect("live DeepSeek call with planning_required must succeed");
        let reply = resp.content.trim();
        eprintln!("reply: {reply:?}");
        assert!(!reply.is_empty(), "model must respond to the planning gate");
        let lower = reply.to_lowercase();
        assert!(
            lower.contains("todo")
                || lower.contains("计划")
                || lower.contains("步骤")
                || lower.contains("step")
                || lower.contains("先")
                || lower.contains("1.")
                || lower.contains("1、"),
            "model should acknowledge a plan-first instruction, got: {reply}"
        );
    }
