//! Heuristic classification — deterministic zero-latency understanding.
use super::signals::*;
use super::text::*;
use super::types::*;

pub fn heuristic_decision(message: &str, intent: UserIntent) -> IntentDecision {
    let signals = task_complexity_signal(message, intent);
    let light = light_task_signal(message, intent);
    let long = message.chars().count() >= 400;
    let multi = split_sub_asks(message).len() > 1;
    let complexity = if signals >= 2 {
        TaskComplexity::High
    } else if signals >= 1 || intent == UserIntent::Research {
        TaskComplexity::Medium
    } else if intent.is_actionable() && !light && creation_implies_large(message) {
        // A terse CREATE request ("写一个贪吃蛇小游戏", 12 chars, under the
        // LLM-routing gate) builds a real multi-file artifact — the Medium
        // budget, not the 25-turn Low cap (which previously truncated it).
        // Light edit requests still stay Low.
        TaskComplexity::Medium
    } else if light {
        TaskComplexity::Low
    } else if long {
        TaskComplexity::Medium
    } else {
        TaskComplexity::Low
    };
    // A numbered sub-task list, or a long actionable request, needs a plan
    // before execution. `Planning` intent is the plan itself — no gate.
    let needs_planning = intent != UserIntent::Planning
        && intent != UserIntent::Research
        && (signals >= 1 || (intent.is_actionable() && !light && long) || multi);
    let needs_subagents = signals >= 1;
    // Research is a multi-source pipeline: always plan first; it may also
    // benefit from parallel gathering when the message is long.
    let needs_planning = if intent == UserIntent::Research {
        true
    } else {
        needs_planning
    };
    let needs_subagents = needs_subagents || (intent == UserIntent::Research && long);
    IntentDecision {
        intent,
        complexity,
        needs_planning,
        needs_subagents,
        multi_intent: multi,
    }
}

/// Classify a user message. Pure heuristic — no I/O, no LLM call.
pub fn classify(message: &str) -> IntentResult {
    let trimmed = message.trim();
    let has_code_block = trimmed.contains("```")
        || trimmed.contains("```rs")
        || trimmed.contains("```ts")
        || trimmed.contains("```py");
    let has_question_mark = trimmed.ends_with('?') || trimmed.ends_with('？');
    let is_question_word = contains_any(
        trimmed,
        &[
            "是什么",
            "为什么",
            "怎么回事",
            "怎么用",
            "如何",
            "哪些",
            "有什么区别",
            "讲讲",
            "介绍一下",
            "解释",
            "what",
            "why",
            "which",
            "how",
            "explain",
            "tell me",
            "can you explain",
        ],
    );
    // A greeting is a short message that is ONLY pleasantries. A short message
    // that carries an action ("ok fix the bug") or where the English word is
    // only a substring of a longer word ("this"/"which" containing "hi") must
    // NOT be forced to Chat. Word-boundary matching stops the substring case;
    // the action-word guard stops the "ok, do X" case.
    let has_action = contains_any(
        trimmed,
        &[
            "修复", "报错", "出错", "崩溃", "失败", "bug", "error", "写", "改", "加", "添加",
            "实现", "删除", "创建", "重构", "优化", "fix", "write", "add", "create",
            "implement", "modify", "change", "帮我", "怎么做", "如何", "为什么", "什么",
        ],
    );
    let is_greeting = trimmed.len() < 20
        && !has_action
        && (contains_any(
            trimmed,
            &["你好", "您好", "嗨", "谢谢", "感谢", "辛苦了", "好的", "嗯"],
        ) || contains_word(trimmed, "hello")
            || contains_word(trimmed, "hi")
            || contains_word(trimmed, "hey")
            || contains_word(trimmed, "ok")
            || contains_word(trimmed, "thanks")
            || contains_word(trimmed, "thank you")
            || contains_word(trimmed, "ty"));

    let intent = if is_greeting {
        UserIntent::Chat
    } else if contains_any(
        trimmed,
        &[
            "修复",
            "修好",
            "报错",
            "出错",
            "崩溃",
            "异常",
            "bug",
            "bug修复",
            "错误",
            "失败",
            "不工作",
            "挂了",
            "不生效",
            "没效果",
            "没反应",
            "坏了",
            "失灵",
            "不能用",
            "error",
            "crash",
            "panic",
            "failing",
            "broken",
            "not working",
            "fix the",
            "debug",
            "调试",
        ],
    ) && (has_code_block
        || has_question_mark
        || contains_any(trimmed, &["为什么", "why"]))
    {
        UserIntent::DebuggingTask
    } else if contains_any(
        trimmed,
        &[
            "调研",
            "研究",
            "查资料",
            "搜集",
            "文献",
            "资料整理",
            "市场分析",
            "竞品",
            "research",
            "investigate",
            "literature",
            "gather sources",
        ],
    ) && !contains_any(trimmed, &["代码", "code", "实现", "implement"])
    {
        UserIntent::Research
    } else if contains_any(
        trimmed,
        &[
            "文案",
            "脚本",
            "分镜",
            "封面",
            "海报",
            "公众号",
            "小红书",
            "抖音",
            "视频",
            "创作",
            "选题",
            "ppt",
            "幻灯片",
            "deck",
            "script",
            "copywriting",
            "推文",
        ],
    ) && !contains_any(
        trimmed,
        &[
            "代码", "code", "实现", "implement", "写代码",
            // "脚本"/"script" are ambiguous: a screenplay is content, a
            // "python 脚本"/"shell script" is code. A code-language qualifier
            // routes to CodingTask, not ContentCreation.
            "python", "shell", "bash", "powershell", "javascript", "typescript",
            "脚本语言", "批处理",
        ],
    )
    {
        UserIntent::ContentCreation
    } else if contains_any(
        trimmed,
        &[
            "写文档",
            "写注释",
            "文档",
            "documentation",
            "doc",
            "readme",
            "注释",
        ],
    ) && !contains_any(trimmed, &["代码", "code", "实现", "implement"])
    {
        UserIntent::Documentation
    } else if contains_any(
        trimmed,
        &[
            "方案",
            "计划",
            "设计",
            "思路",
            "规划",
            "proposal",
            "plan",
            "design",
            "architecture",
            "架构",
        ],
    ) && !contains_any(trimmed, &["实现", "implement", "写代码", "改代码"])
    {
        UserIntent::Planning
    } else if contains_any(
        trimmed,
        &["审查", "review", "code review", "帮我看看这段", "检查代码"],
    ) {
        UserIntent::Review
    } else if contains_any(
        trimmed,
        &[
            "看看这个项目",
            "了解",
            "探索",
            "找到",
            "定位",
            "在哪里",
            "在哪",
            "找出",
            "explore",
            "find",
            "where is",
            "locate",
            "项目结构",
            "目录结构",
            "有哪些文件",
        ],
    ) && !has_code_block
    {
        UserIntent::Exploration
    } else if has_code_block
        || contains_any(
            trimmed,
            &[
                "写",
                "改",
                "加",
                "添加",
                "创建",
                "新建",
                "实现",
                "重构",
                "更新",
                "删除",
                "增加",
                "修复",
                "调整",
                "优化",
                "完成",
                "添加",
                "write",
                "add",
                "create",
                "implement",
                "refactor",
                "update",
                "delete",
                "remove",
                "fix",
                "change",
                "modify",
                "build",
                "实现",
            ],
        )
        || contains_file_path(trimmed)
    {
        if (has_code_block
            && contains_any(
                trimmed,
                &["问题", "哪里不对", "wrong", "issue", "bug", "报错", "error"],
            ))
            || (contains_any(
                trimmed,
                &[
                    "报错",
                    "error",
                    "bug",
                    "崩溃",
                    "crash",
                    "失败",
                    "broken",
                    "不生效",
                    "没效果",
                    "没反应",
                    "坏了",
                    "失灵",
                    "不能用",
                ],
            ) && !has_code_block)
        {
            UserIntent::DebuggingTask
        } else {
            UserIntent::CodingTask
        }
    } else if has_question_mark || is_question_word {
        UserIntent::Question
    } else {
        UserIntent::Chat
    };

    IntentResult {
        intent,
        goal_draft: draft_goal(trimmed, intent),
        acceptance_hint: extract_acceptance_hint(trimmed),
    }
}
fn draft_goal(text: &str, intent: UserIntent) -> Option<String> {
    if !intent.is_actionable() {
        return None;
    }
    let cleaned = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("```") && !l.starts_with('#'))?
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string();
    if cleaned.is_empty() {
        return None;
    }
    let truncated: String = cleaned.chars().take(96).collect();
    Some(if truncated != cleaned {
        format!("{truncated}…")
    } else {
        truncated
    })
}

/// Extract a completion signal (definition of done) if the user mentioned one.
fn extract_acceptance_hint(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let hints: &[(&str, &str)] = &[
        ("测试通过", "tests pass"),
        ("编译通过", "build compiles"),
        ("无报错", "no errors"),
        ("没有报错", "no errors"),
        ("不报错", "no errors"),
        ("通过测试", "tests pass"),
        ("typecheck", "typecheck passes"),
        ("lint", "lint passes"),
        ("test pass", "tests pass"),
        ("tests pass", "tests pass"),
        ("build pass", "build passes"),
        ("编译", "build succeeds"),
        ("能跑起来", "it runs"),
        ("正常工作", "it works"),
    ];
    for (zh, en) in hints {
        if lower.contains(zh) {
            return Some((*en).to_string());
        }
    }
    None
}

