//! Intent tests.

use super::*;

    #[test]
    fn long_multiclause_request_signal() {
        let mut msg = String::from("请重构这个模块");
        while msg.chars().count() < 450 {
            msg.push_str("，同时调整相关的错误处理路径，并且更新文档注释");
        }
        assert_eq!(task_complexity_signal(&msg, UserIntent::CodingTask), 1);
    }

    #[test]
    fn actionable_intents_never_need_routing() {
        // Short, unambiguous actionable requests skip the LLM call entirely.
        assert!(!needs_llm_routing(
            "帮我写一个斐波那契函数",
            UserIntent::CodingTask
        ));
        assert!(!needs_llm_routing(
            "修复这个报错",
            UserIntent::DebuggingTask
        ));
        assert!(!needs_llm_routing(
            "给这个项目写 README",
            UserIntent::Documentation
        ));
        assert!(!needs_llm_routing("给一个重构方案", UserIntent::Planning));
    }

    #[test]
    fn short_uncertain_messages_skip_routing() {
        // Short questions/greetings are unambiguous — no LLM call needed.
        assert!(!needs_llm_routing("你好", UserIntent::Chat));
        assert!(!needs_llm_routing("ok", UserIntent::Chat));
        assert!(!needs_llm_routing("什么是闭包？", UserIntent::Question));
    }

    #[test]
    fn substantive_messages_route_even_when_actionable() {
        // A long actionable request gets one routing call: the complexity /
        // planning / subagent decision steers the whole turn.
        assert!(needs_llm_routing(
            "帮我重构这个项目，拆分成三个独立模块，每个模块都要有完整的测试覆盖",
            UserIntent::CodingTask
        ));
        assert!(needs_llm_routing(
            "这个项目的数据库层为什么要用 SQLite 而不是 PostgreSQL，能详细讲讲各自的权衡吗",
            UserIntent::Question
        ));
        assert!(needs_llm_routing(
            "我注意到项目里有几个模块的代码风格不太一致，想了解一下整体情况",
            UserIntent::Chat
        ));
    }

    #[test]
    fn light_actionable_messages_skip_routing() {
        // A single-purpose edit has an unambiguous heuristic execution
        // profile (low complexity, direct, no planning) — the extra routing
        // LLM call would only confirm it at the cost of one round trip and
        // up to 4s of sequential latency on the most common request kind.
        assert!(!needs_llm_routing(
            "修改一下目录的html 让他像你的官网一样",
            UserIntent::CodingTask
        ));
        assert!(!needs_llm_routing(
            "把 index.html 的标题改成新的样式，字体用系统默认",
            UserIntent::CodingTask
        ));
        assert!(!needs_llm_routing(
            "update the README to mention the new flag and keep it short",
            UserIntent::Documentation
        ));
        assert!(!needs_llm_routing(
            "修复一下登录页的样式问题，按钮颜色改成品牌色",
            UserIntent::DebuggingTask
        ));
    }

    #[test]
    fn complex_or_ambiguous_messages_still_route() {
        // Multi-part / large-scope work and questions keep the routing call:
        // their execution profile or classification genuinely benefits.
        assert!(needs_llm_routing(
            "帮我重构这个项目，拆分成三个独立模块，每个模块都要有完整的测试覆盖",
            UserIntent::CodingTask
        ));
        assert!(needs_llm_routing(
            "帮我做三件事：创建 API、写数据库层、接前端、写测试",
            UserIntent::CodingTask
        ));
        assert!(needs_llm_routing(
            "这个项目的数据库层为什么要用 SQLite 而不是 PostgreSQL，能详细讲讲各自的权衡吗",
            UserIntent::Question
        ));
        assert!(needs_llm_routing(
            "我注意到项目里有几个模块的代码风格不太一致，想了解一下整体情况",
            UserIntent::Chat
        ));
    }

    #[test]
    fn whitespace_only_message_never_triggers() {
        assert!(!needs_llm_routing("   ", UserIntent::Chat));
    }

    #[test]
    fn heuristic_decision_flags_multi_part_tasks() {
        let msg = "帮我做三件事：\n1. 创建 API\n2. 写数据库层\n3. 接前端\n4. 写测试";
        let d = heuristic_decision(msg, UserIntent::CodingTask);
        assert_eq!(d.complexity, TaskComplexity::Medium);
        assert!(d.needs_planning, "numbered multi-part work must plan first");
        assert!(d.needs_subagents);
        assert!(build_task_spec(&classify(msg), msg, &d)
            .unwrap()
            .contains("<planning_required>"));
    }

    #[test]
    fn heuristic_decision_keeps_light_tasks_direct() {
        let msg = "把 index.html 的标题改成新的";
        let d = heuristic_decision(msg, UserIntent::CodingTask);
        assert_eq!(d.complexity, TaskComplexity::Low);
        assert!(!d.needs_planning);
        assert!(!d.needs_subagents);
        let spec = build_task_spec(&classify(msg), msg, &d).unwrap();
        assert!(spec.contains("<delegation>direct</delegation>"), "{spec}");
        assert!(!spec.contains("<planning_required>"), "{spec}");
        assert!(!spec.contains("<complexity>"), "{spec}");
    }

    #[test]
    fn heuristic_decision_planning_intent_never_gated() {
        // A plan request produces the plan — no pre-plan gate needed.
        let msg = "给一个重构方案";
        let d = heuristic_decision(msg, UserIntent::Planning);
        assert!(!d.needs_planning);
    }

    #[test]
    fn decision_drives_delegation_tier() {
        let mut d = IntentDecision::of(UserIntent::CodingTask);
        d.needs_subagents = true;
        d.complexity = TaskComplexity::High;
        assert_eq!(
            delegation_advice(&d, "任意消息"),
            Some((DelegationTier::Parallel3_5, DELEGATION_REASON_PARALLEL_3_5))
        );

        d.complexity = TaskComplexity::Medium;
        assert_eq!(
            delegation_advice(&d, "任意消息"),
            Some((DelegationTier::Parallel2_3, DELEGATION_REASON_PARALLEL_2_3))
        );

        // Router says delegate, but the message is a tiny edit → direct wins.
        let mut light = IntentDecision::of(UserIntent::CodingTask);
        light.needs_subagents = true;
        light.complexity = TaskComplexity::Medium;
        assert_eq!(
            delegation_advice(&light, "把 index.html 的标题改成新的"),
            Some((DelegationTier::Direct, DELEGATION_REASON_DIRECT))
        );

        let quiet = IntentDecision::of(UserIntent::Review);
        assert!(delegation_advice(&quiet, "帮我审查一下登录模块").is_none());
    }

    #[test]
    fn non_actionable_gets_no_delegation_advice() {
        let d = IntentDecision::of(UserIntent::Chat);
        assert!(delegation_advice(&d, "你好").is_none());
        let d = IntentDecision::of(UserIntent::Question);
        assert!(delegation_advice(&d, "什么是闭包？").is_none());
    }

    #[test]
    fn classifies_depwork_research_and_content() {
        assert_eq!(
            classify("调研一下 xx 的资料并整理成报告").intent,
            UserIntent::Research
        );
        assert_eq!(
            classify("找几篇文献做文献综述").intent,
            UserIntent::Research
        );
        assert_eq!(
            classify("写一篇小红书文案").intent,
            UserIntent::ContentCreation
        );
        assert_eq!(
            classify("做一个产品发布 PPT").intent,
            UserIntent::ContentCreation
        );
        // Code words still win — 写代码 stays a coding task.
        assert_eq!(
            classify("写一段 Python 代码").intent,
            UserIntent::CodingTask
        );
    }

    #[test]
    fn followup_continuation_detection() {
        for short in [
            "继续",
            "再优化一下",
            "改成蓝色",
            "继续做",
            "然后呢",
            "keep going",
        ] {
            assert!(is_followup_continuation(short), "{short} is a follow-up");
        }
        assert!(
            !is_followup_continuation("继续把这段代码重构并补充测试，完成后跑全量回归"),
            "substantive messages route normally"
        );
        assert!(!is_followup_continuation("你好"));
        assert!(!is_followup_continuation(""));
    }

    #[test]
    fn research_forces_planning_in_heuristic() {
        let decision = heuristic_decision("调研一下竞品定价并整理成表", UserIntent::Research);
        assert!(decision.needs_planning, "research always plans first");
        assert_eq!(decision.complexity, TaskComplexity::Medium);
        assert!(decision.intent.is_actionable());
    }

    #[test]
    fn split_sub_asks_handles_numbered_and_connectors() {
        let numbered = split_sub_asks("1. 修复登录 bug\n2. 加测试\n3. 更新文档");
        assert_eq!(numbered.len(), 3);
        assert!(numbered[0].contains("修复登录"));

        let clauses = split_sub_asks("重构一下 API 层，另外顺便把 README 更新了");
        assert_eq!(clauses.len(), 2);
        assert!(clauses[0].contains("重构"));
        assert!(clauses[1].contains("README"));

        let single = split_sub_asks("帮我修复这个报错");
        assert!(single.len() <= 1);
    }

    #[test]
    fn multi_intent_forces_planning_and_spec_note() {
        let msg = "1. 写接口\n2. 接前端";
        let result = classify(msg);
        let decision = heuristic_decision(msg, result.intent);
        assert!(decision.multi_intent);
        assert!(decision.needs_planning, "multi-intent always plans");
        let spec = build_task_spec(&result, msg, &decision).expect("spec");
        assert!(spec.contains("<multi_intent>"), "{spec}");
    }

    #[test]
    fn clarification_detection_is_conservative() {
        assert!(needs_clarification("帮我看看这个"));
        assert!(needs_clarification("这个怎么改"));
        assert!(!needs_clarification("帮我看看这个 index.html"));
        assert!(!needs_clarification("修复这个报错：TypeError 在 main.ts"));
        assert!(!needs_clarification(
            "这句话很长，超过了四十个字符所以不会被误判为歧义引用"
        ));
    }
