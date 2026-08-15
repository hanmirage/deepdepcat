//! Intent signals — follow-up/split/clarification/complexity heuristics.
use super::text::*;
use super::types::*;

/// Whether a SHORT message is a follow-up continuation of the previous
/// turn ("继续", "然后呢", "再优化一下", "改成 X"…). These inherit the
/// previous intent decision instead of being re-classified as casual chat.
pub fn is_followup_continuation(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() >= 16 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    [
        "继续",
        "然后",
        "接着",
        "下一步",
        "继续做",
        "往下",
        "然后呢",
        "改成",
        "换成",
        "优化一下",
        "再优化",
        "试试",
        "另一种",
        "换个",
        "keep going",
        "continue",
        "next",
        "again",
        "go on",
        "再改",
        "再写",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

/// Split a message into distinct sub-asks (numbered list first, then
/// connector-separated clauses). Returns at most 3 substantive segments.
/// Pure + unit-testable; drives `multi_intent` and the todo-splitting nudge.
pub fn split_sub_asks(message: &str) -> Vec<String> {
    const CONNECTORS: &[&str] = &[
        "另外再",
        "另外还有",
        "再顺便",
        "另外",
        "顺便",
        "同时",
        "还有",
        "以及",
        "其次",
        "最后",
    ];
    let lines: Vec<&str> = message
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let numbered = lines
        .iter()
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()) || l.starts_with("- "))
        .count();

    let mut out: Vec<String> = Vec::new();
    if numbered >= 2 {
        for line in lines {
            let item = strip_numbered_prefix(line)
                .or_else(|| line.strip_prefix("- "))
                .map(str::trim)
                .filter(|s| s.chars().count() >= 2)
                .map(String::from);
            if let Some(item) = item {
                out.push(item);
            }
        }
    } else {
        let mut parts: Vec<String> = vec![message.trim().to_string()];
        for connector in CONNECTORS {
            let mut next: Vec<String> = Vec::new();
            for part in parts {
                let segments: Vec<&str> = part.split(connector).collect();
                if segments.len() > 1 {
                    next.extend(segments.iter().map(|s| s.to_string()));
                } else {
                    next.push(part);
                }
            }
            parts = next;
        }
        for part in parts {
            let part = part.trim();
            if part.chars().count() >= 6 {
                out.push(part.to_string());
            }
        }
    }
    out.truncate(3);
    out
}

/// Strip a leading numbered-list prefix ("1. ", "10) ", "2、") and return the
/// item text. Handles multi-digit numbers ("10.") — the old single-digit
/// strip dropped every item numbered 10 and up.
fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &trimmed[digits..];
    rest.strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .or_else(|| rest.strip_prefix("、"))
}

/// Whether a SHORT message contains an ambiguous reference ("这个/那个/它/
/// 这段/it/this/that") with no concrete anchor (file path, code block, URL,
/// quoted identifier) — the model should ask one clarifying question
/// instead of guessing.
pub fn needs_clarification(message: &str) -> bool {
    let trimmed = message.trim();
    if trimmed.chars().count() > 40 {
        return false;
    }
    let has_anchor = trimmed.contains("```")
        || trimmed.contains("://")
        || contains_file_path(trimmed)
        || contains_any(
            trimmed,
            &[
                "index.", ".rs", ".ts", ".js", ".py", ".md", ".html", ".css", ".toml", ".json",
                ".docx", ".xlsx", ".pptx",
            ],
        )
        || trimmed.contains('"')
        || trimmed.contains('`');
    if has_anchor {
        return false;
    }
    contains_any(
        trimmed,
        &[
            "这个",
            "那个",
            "它",
            "这段",
            "这里",
            "那边",
            "这个 bug",
            "那个问题",
            "这个报错",
            "这个错误",
            "it",
            "this",
            "that",
            "this file",
            "that file",
            "这个文件",
            "那个文件",
        ],
    )
}

pub fn task_complexity_signal(message: &str, intent: UserIntent) -> usize {
    if !intent.is_actionable() {
        return 0;
    }
    let mut signals = 0;
    // Numbered sub-task lists — the strongest signal.
    let numbered = message
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            // Multi-digit items ("10.", "11)") were missed by the old
            // single-char peek — strip ALL leading digits, then check the
            // separator (the same shape split_sub_asks already handles).
            let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
            digits > 0 && t[digits..].trim_start().starts_with(['.', ')', '、', '．'])
        })
        .count();
    if numbered >= 3 {
        signals += 1;
    }
    // Long actionable request with multiple independent clauses.
    if message.chars().count() >= 400 {
        let separators = message.matches("并且").count()
            + message.matches("同时").count()
            + message.matches("还有").count()
            + message.matches("然后").count()
            + message.matches(";").count();
        if separators >= 3 || numbered >= 2 {
            signals += 1;
        }
    }
    signals
}

/// Build the decomposition-suggestion guidance, or `None` when the task is
/// small enough to handle directly.
pub fn suggest_decompose(message: &str, intent: UserIntent) -> Option<String> {
    let signals = task_complexity_signal(message, intent);
    if signals == 0 {
        return None;
    }
    Some(
        "This request looks like a multi-part task. Per <task_completion_discipline> \
         TASK RULE 3, consider structuring the work:\n\
         - If the parts are independent, use the `agent` tool with \
         `decompose: true` to split the task into parallel sub-agents.\n\
         - If the parts depend on each other, work through them sequentially \
         and track each with `todo_write`: order steps by dependency (set each \
         step's depends_on), give each a verify command, and build in vertical \
         slices so each phase leaves the project runnable.\n\
         Either way, explore the relevant files first so each sub-task has a \
         concrete target."
            .to_string(),
    )
}

/// Whether the request asks to CREATE a substantial standalone artifact
/// (a game/system/app/site/tool/framework/service) rather than a small edit
/// or a snippet. A terse create request like "写一个贪吃蛇小游戏" (12 chars)
/// is under the LLM-routing gate and was otherwise classified Low — a real
/// multi-file artifact needs the Medium budget, not the 25-turn Low cap.
pub fn creation_implies_large(message: &str) -> bool {
    const ARTIFACTS: &[&str] = &[
        "游戏", "系统", "应用", "app", "网站", "官网", "项目", "工具", "框架",
        "爬虫", "服务", "后端", "前端", "全栈", "博客", "商城", "后台", "小程序",
        "桌面端", "客户端", "数据库", "组件库",
    ];
    let lower = message.to_lowercase();
    ARTIFACTS.iter().any(|a| lower.contains(a))
}

/// Whether the request is a SMALL, single-purpose change that must NOT
/// start a research pipeline.
///
/// The anti-overreach counterpart of `suggest_decompose`: a single-file
/// restyle ("改一下这个 html 像官网一样") must not escalate into downloading
/// reference sites, multi-round extraction, and scratch files in the user's
/// workspace. Signs: an edit-style verb, no complexity signals, no
/// large-scope wording, and a short message.
pub fn light_task_signal(message: &str, intent: UserIntent) -> bool {
    if !matches!(
        intent,
        UserIntent::CodingTask | UserIntent::DebuggingTask | UserIntent::Documentation
    ) {
        return false;
    }
    if task_complexity_signal(message, intent) > 0 {
        return false;
    }
    // Multiple explicit file references mean the change spans files.
    if count_file_refs(message) > 1 {
        return false;
    }
    // Explicitly large-scope wording disqualifies a light label.
    if has_large_scope_wording(message) {
        return false;
    }
    message.chars().count() <= 120 && is_edit_request(message)
}

/// Explicitly large-scope wording — disqualifies a "light" label and, when
/// present in a SHORT message, justifies an LLM routing upgrade (a 7-char
/// "全面重构数据层" is a high-scope request, not a one-word question).
pub(crate) fn has_large_scope_wording(message: &str) -> bool {
    [
        "重构",
        "重新设计",
        "迁移",
        "整个项目",
        "所有页面",
        "从头",
        "实现一个完整",
        "大改",
        // Multi-step verbs that imply read → understand → change across
        // files — NOT a light one-off edit. "优化"/"收尾" on a directory
        // was misclassified as light (12-turn budget) and cut off mid-task.
        "优化",
        "收尾",
        "完善",
        "全面",
    ]
    .iter()
    .any(|w| message.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimize_sweep_is_not_light() {
        // "优化"/"收尾" imply read → understand → change across files, so a
        // directory sweep must NOT land in the 12-turn light-task budget.
        // (Real session 1f4c7e0e: "继续优化目录下网页" was cut off mid-task
        // at turn 12 with "已到轮次上限".)
        assert!(!light_task_signal(
            "继续优化目录下网页看看那些还没有收尾的",
            UserIntent::CodingTask
        ));
        assert!(!light_task_signal(
            "把这几页的收尾工作完善一下",
            UserIntent::CodingTask
        ));
    }

    #[test]
    fn goodbye_and_negation_are_not_followups() {
        // The bare "再" substring matched "再见" (goodbye) and "不再"
        // (no-longer), inheriting the previous coding intent instead of
        // re-classifying the message.
        assert!(!is_followup_continuation("再见"));
        assert!(!is_followup_continuation("不再需要这个功能了"));
        // Genuine follow-ups still match.
        assert!(is_followup_continuation("再优化"));
        assert!(is_followup_continuation("继续"));
    }

    #[test]
    fn multi_digit_numbered_lists_are_counted() {
        // "10."/"11."/"12." items were missed by the old single-char peek,
        // silently demoting a multi-part task to the light 12-turn budget.
        let message = "10. 修复登录 bug\n11. 加测试\n12. 更新文档";
        let signals = task_complexity_signal(message, UserIntent::CodingTask);
        assert!(signals >= 1, "multi-digit list must signal complexity: {signals}");
    }
}
