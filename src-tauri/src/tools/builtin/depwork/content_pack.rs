//! content_pack — 多平台内容一键导出（公众号 / 小红书 / 知乎）。
//!
//! 按平台硬性格式规则做确定性校验（标题长度 / 段落行数 / emoji / 小标题 /
//! 结尾号召 / 溯源），把每版文本写入工作区导出目录并返回结构化合规报告。
//! 只 flag 不 auto-fix —— 创作性改动（缩短标题、改口吻）留给 agent 改后重跑。

use crate::core::error::{AppError, AppResult};
use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashSet;

pub const PLATFORM_WECHAT: &str = "wechat";
pub const PLATFORM_XIAOHONGSHU: &str = "xiaohongshu";
pub const PLATFORM_ZHIHU: &str = "zhihu";
pub const PLATFORMS: [&str; 3] = [PLATFORM_WECHAT, PLATFORM_XIAOHONGSHU, PLATFORM_ZHIHU];

/// 手机正文一行的 CJK 字符估宽（~17px 字在 ~343px 内容列）。启发式常量。
const LINE_CHARS_CJK: f64 = 24.0;
const XHS_TITLE_MAX: usize = 20;
const WECHAT_TITLE_MIN: usize = 8;
const WECHAT_TITLE_MAX: usize = 20;
const XHS_MAX_LINES: f64 = 3.0;
const LONG_MAX_LINES: f64 = 6.0;
/// 一段需要带 emoji 的最短长度（短 CTA 行不强制 emoji）。
const EMOJI_MIN_CHARS: usize = 12;

/// 公众号/知乎开头的套话反例（钩子要求具体冲突/反常识/数字）。
const FILLER_OPENERS: [&str; 6] = [
    "在当今社会",
    "众所周知",
    "随着时代的发展",
    "随着社会的进步",
    "随着科技的发展",
    "近年来",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleLevel {
    Fail,
    Warn,
}

struct RuleResult {
    id: &'static str,
    label: String,
    level: RuleLevel,
    ok: bool,
    detail: String,
}

impl RuleResult {
    fn new(
        id: &'static str,
        label: impl Into<String>,
        level: RuleLevel,
        ok: bool,
        detail: impl Into<String>,
    ) -> Self {
        RuleResult {
            id,
            label: label.into(),
            level,
            ok,
            detail: detail.into(),
        }
    }

    fn fail(
        id: &'static str,
        label: impl Into<String>,
        ok: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(id, label, RuleLevel::Fail, ok, detail)
    }

    fn warn(
        id: &'static str,
        label: impl Into<String>,
        ok: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(id, label, RuleLevel::Warn, ok, detail)
    }
}

struct PlatformReport {
    platform: String,
    file: String,
    passed: bool,
    warnings: usize,
    rules: Vec<RuleResult>,
}

impl PlatformReport {
    fn new(platform: &str, file: String, rules: Vec<RuleResult>) -> Self {
        let passed = rules.iter().all(|r| r.level != RuleLevel::Fail || r.ok);
        let warnings = rules
            .iter()
            .filter(|r| r.level == RuleLevel::Warn && !r.ok)
            .count();
        PlatformReport {
            platform: platform.to_string(),
            file,
            passed,
            warnings,
            rules,
        }
    }
}

struct ParsedItem {
    platform: String,
    title: String,
    content: String,
}

fn platform_label(platform: &str) -> &'static str {
    match platform {
        PLATFORM_XIAOHONGSHU => "小红书",
        PLATFORM_ZHIHU => "知乎",
        _ => "公众号",
    }
}

fn is_emoji(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x1F000..=0x1FAFF
            | 0x2600..=0x27BF
            | 0x2B00..=0x2BFF
            | 0xFE0F
            | 0x00A9
            | 0x00AE
            | 0x2122
            | 0x2190..=0x21FF
    )
}

fn count_emoji(s: &str) -> usize {
    s.chars().filter(|&c| is_emoji(c)).count()
}

fn effective_width(s: &str) -> f64 {
    s.chars()
        .map(|c| {
            if is_emoji(c) {
                2.0
            } else if c.is_ascii() {
                0.5
            } else {
                1.0
            }
        })
        .sum()
}

fn paragraph_lines(paragraph: &[&str]) -> f64 {
    let total: f64 = paragraph.iter().map(|l| effective_width(l)).sum();
    if total <= 0.0 {
        0.0
    } else {
        (total / LINE_CHARS_CJK).ceil()
    }
}

fn paragraph_emoji(paragraph: &[&str]) -> usize {
    paragraph.iter().map(|l| count_emoji(l)).sum()
}

/// 把内容按空行分成段落（每段 = 连续非空行，去首尾空白）。
fn paragraphs_of(content: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(t);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 标题 → 文件名安全片段：保留 CJK/字母数字，分隔符转 `_`，空则回落 untitled。
fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.trim().chars() {
        if c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            out.push('_');
        } else if c.is_alphanumeric() || !c.is_ascii_control() {
            out.push(c);
        }
    }
    let trimmed = out.trim_matches(['_', '.']).to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

fn normalize_content(content: &str) -> String {
    let normalized = content.replace("\r\n", "\n");
    let trimmed = normalized.trim_end();
    format!("{trimmed}\n")
}

fn has_source(content: &str) -> bool {
    content.contains("http://")
        || content.contains("https://")
        || content.contains("未证实")
        || content.contains("待证实")
        || content.contains("数据缺失")
}

fn ends_with_question(text: &str) -> bool {
    let t = text.trim();
    t.ends_with('?')
        || t.ends_with('？')
        || t.ends_with("怎么")
        || t.ends_with('吗')
        || t.ends_with('呢')
}

fn has_action_call(text: &str) -> bool {
    ["总结", "行动", "建议", "试试", "关注", "点赞", "收藏", "留言", "欢迎", "分享"].iter()
        .any(|k| text.contains(k))
}

fn has_filler_opening(text: &str) -> bool {
    FILLER_OPENERS.iter().any(|f| text.trim_start().starts_with(f))
}

fn has_subheading(content: &str) -> bool {
    content.lines().any(|l| l.trim_start().starts_with('#'))
}

fn over_long_paragraphs(paras: &[Vec<&str>], max: f64) -> usize {
    paras.iter().filter(|p| paragraph_lines(p) > max).count()
}

fn emoji_issue_paragraphs(paras: &[Vec<&str>]) -> usize {
    paras
        .iter()
        .filter(|p| {
            let n = paragraph_emoji(p);
            let len: usize = p.iter().map(|l| l.chars().count()).sum();
            n > 3 || (n == 0 && len >= EMOJI_MIN_CHARS)
        })
        .count()
}

fn last_line(content: &str) -> &str {
    content.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("")
}

fn check_xiaohongshu(title: &str, content: &str) -> Vec<RuleResult> {
    let title_len = title.chars().count();
    let paras = paragraphs_of(content);
    let last_line = last_line(content);
    let title_ok = title_len <= XHS_TITLE_MAX;
    let paras_ok = !paras.is_empty();
    let over = over_long_paragraphs(&paras, XHS_MAX_LINES);
    let emoji_issues = emoji_issue_paragraphs(&paras);
    let cta_ok = ends_with_question(last_line);
    vec![
        RuleResult::fail(
            "title_length",
            "标题 ≤20 字",
            title_ok,
            format!(
                "标题 {title_len} 字，{}",
                if title_ok {
                    "符合".to_string()
                } else {
                    format!("超出 {XHS_TITLE_MAX} 字上限")
                }
            ),
        ),
        RuleResult::fail(
            "has_paragraph",
            "至少一个非空段落",
            paras_ok,
            if paras_ok {
                format!("{} 个段落", paras.len())
            } else {
                "正文为空".to_string()
            },
        ),
        RuleResult::warn(
            "paragraph_lines",
            "每段 ≤3 行",
            over == 0,
            if over == 0 {
                "所有段落行数合规".to_string()
            } else {
                format!("{over} 个段落超 {XHS_MAX_LINES} 行")
            },
        ),
        RuleResult::warn(
            "emoji_balance",
            "每段 1–2 个 emoji",
            emoji_issues == 0,
            if emoji_issues == 0 {
                "emoji 用量合规".to_string()
            } else {
                format!("{emoji_issues} 段 emoji 数量异常（0 或 >3）")
            },
        ),
        RuleResult::warn(
            "cta_question",
            "结尾互动问题",
            cta_ok,
            if cta_ok {
                "结尾有互动问题".to_string()
            } else {
                "结尾缺少互动问题（建议以「…你们怎么选？」收尾）".to_string()
            },
        ),
    ]
}

fn check_wechat(title: &str, content: &str) -> Vec<RuleResult> {
    let title_len = title.chars().count();
    let paras = paragraphs_of(content);
    let first_para = paras.first().map(|p| p.join("")).unwrap_or_default();
    let last_line = last_line(content);
    let title_ok = (WECHAT_TITLE_MIN..=WECHAT_TITLE_MAX).contains(&title_len);
    let sub_ok = has_subheading(content);
    let over = over_long_paragraphs(&paras, LONG_MAX_LINES);
    let filler = has_filler_opening(&first_para);
    let source_ok = has_source(content);
    let cta_ok = has_action_call(last_line);
    vec![
        RuleResult::fail(
            "title_length",
            "标题 8–20 字",
            title_ok,
            format!("标题 {title_len} 字（需 {WECHAT_TITLE_MIN}–{WECHAT_TITLE_MAX} 字）"),
        ),
        RuleResult::fail(
            "min_paragraphs",
            "≥3 个段落",
            paras.len() >= 3,
            format!("{} 个段落（需 ≥3）", paras.len()),
        ),
        RuleResult::fail(
            "subheadings",
            "有小标题",
            sub_ok,
            if sub_ok {
                "含 # 小标题".to_string()
            } else {
                "缺少小标题（用 # 分段）".to_string()
            },
        ),
        RuleResult::warn(
            "paragraph_lines",
            "每段 ≤6 行",
            over == 0,
            if over == 0 {
                "所有段落行数合规".to_string()
            } else {
                format!("{over} 个段落超 {LONG_MAX_LINES} 行")
            },
        ),
        RuleResult::warn(
            "hook",
            "开头是钩子（非套话）",
            !filler,
            if filler {
                "开头疑似套话，改为具体冲突/反常识/数字".to_string()
            } else {
                "开头有钩子".to_string()
            },
        ),
        RuleResult::warn(
            "traceable",
            "数据有来源",
            source_ok,
            if source_ok {
                "含来源或未证实标注".to_string()
            } else {
                "关键事实需可溯源（URL 或「未证实」）".to_string()
            },
        ),
        RuleResult::warn(
            "cta",
            "结尾行动号召",
            cta_ok,
            if cta_ok {
                "结尾有行动号召".to_string()
            } else {
                "结尾缺少可截图引用的总结 + 具体行动建议".to_string()
            },
        ),
    ]
}

fn check_zhihu(title: &str, content: &str) -> Vec<RuleResult> {
    let title_ok = !title.trim().is_empty();
    let paras = paragraphs_of(content);
    let paras_ok = !paras.is_empty();
    let first_para_lines = paras.first().map(|p| paragraph_lines(p)).unwrap_or(0.0);
    let over = over_long_paragraphs(&paras, LONG_MAX_LINES);
    let source_ok = has_source(content);
    vec![
        RuleResult::fail(
            "title_nonempty",
            "标题非空",
            title_ok,
            if title_ok {
                "标题已提供".to_string()
            } else {
                "标题为空".to_string()
            },
        ),
        RuleResult::fail(
            "has_paragraph",
            "至少一个非空段落",
            paras_ok,
            if paras_ok {
                format!("{} 个段落", paras.len())
            } else {
                "正文为空".to_string()
            },
        ),
        RuleResult::warn(
            "conclusion_first",
            "前置结论",
            first_para_lines <= 4.0,
            if first_para_lines > 4.0 {
                format!("开头过长（约 {first_para_lines} 行），建议 3 句内给结论")
            } else {
                "开头紧凑，有前置结论".to_string()
            },
        ),
        RuleResult::warn(
            "paragraph_lines",
            "每段 ≤6 行",
            over == 0,
            if over == 0 {
                "所有段落行数合规".to_string()
            } else {
                format!("{over} 个段落超 {LONG_MAX_LINES} 行")
            },
        ),
        RuleResult::warn(
            "traceable",
            "引用/数据有来源",
            source_ok,
            if source_ok {
                "含来源或未证实标注".to_string()
            } else {
                "关键论断需来源（URL 或「未证实」）".to_string()
            },
        ),
    ]
}

fn check_platform(platform: &str, title: &str, content: &str) -> Vec<RuleResult> {
    match platform {
        PLATFORM_XIAOHONGSHU => check_xiaohongshu(title, content),
        PLATFORM_WECHAT => check_wechat(title, content),
        _ => check_zhihu(title, content),
    }
}

fn build_manifest(reports: &[PlatformReport]) -> String {
    let mut platforms = serde_json::Map::new();
    for r in reports {
        let violations: Vec<String> = r
            .rules
            .iter()
            .filter(|x| !x.ok)
            .map(|x| format!("[{}] {}: {}", x.id, x.label, x.detail))
            .collect();
        platforms.insert(
            r.platform.clone(),
            json!({
                "status": if r.passed { "pass" } else { "fail" },
                "file": r.file,
                "warnings": r.warnings,
                "violations": violations,
            }),
        );
    }
    serde_json::to_string_pretty(&json!({
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "platforms": platforms,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

fn format_report(reports: &[PlatformReport], out_dir: &std::path::Path) -> String {
    let mut lines = vec![format!("多平台文本包已导出到 {}", out_dir.display())];
    for r in reports {
        let mark = if r.passed { "✅" } else { "⚠️" };
        let mut header = format!(
            "{mark} {}（{}）— {}",
            platform_label(&r.platform),
            r.file,
            if r.passed { "通过" } else { "未通过" }
        );
        if r.warnings > 0 {
            header.push_str(&format!(" + {} 条建议", r.warnings));
        }
        lines.push(header);
        for x in &r.rules {
            if !x.ok {
                let lvl = if x.level == RuleLevel::Fail { "硬伤" } else { "建议" };
                lines.push(format!("   - [{lvl}] {}: {}", x.label, x.detail));
            }
        }
    }
    lines.push("合规清单: manifest.json".to_string());
    lines.join("\n")
}

pub struct ContentPackTool;

impl ContentPackTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ContentPackTool {
    fn name(&self) -> &str {
        "content_pack"
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "多平台内容一键导出：把同一内容的不同平台版本（公众号/小红书/知乎）\
         写入工作区文本包，并按各平台硬性格式规则做合规校验（标题长度、段落行数、\
         emoji、小标题、结尾号召、溯源）。返回每个平台的通过/未通过报告与违规明细；\
         只校验不改写，创作性调整由 agent 完成后再调用。\
         Parameters: items (required, 1-3 个 {platform, title, content}), \
         output_dir (可选，默认 content-pack)。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "platform": {"type": "string", "enum": ["wechat", "xiaohongshu", "zhihu"], "description": "目标平台。"},
                            "title": {"type": "string", "description": "该平台版本的标题。"},
                            "content": {"type": "string", "description": "该平台版本的内容（Markdown/纯文本）。"}
                        }
                    },
                    "description": "1–3 个平台版本，每个平台一版（platform 不重复）。"
                },
                "output_dir": {"type": "string", "description": "工作区导出目录（默认 content-pack）。"}
            },
            "required": ["items"]
        })
    }

    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let raw = args
            .get("output_dir")
            .and_then(|d| d.as_str())
            .unwrap_or("content-pack");
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, None);
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let items = args
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "Missing required parameter: items".to_string())?;
        if items.is_empty() {
            return Ok(ToolResult::error(
                "items 为空 — 至少需要一个平台版本".to_string(),
            ));
        }

        let mut seen = HashSet::new();
        let mut parsed: Vec<ParsedItem> = Vec::new();
        for item in items {
            let platform = item
                .get("platform")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            if !PLATFORMS.contains(&platform.as_str()) {
                return Ok(ToolResult::error(format!(
                    "未知平台: {platform}（可选 wechat/xiaohongshu/zhihu）"
                )));
            }
            if !seen.insert(platform.clone()) {
                return Ok(ToolResult::error(format!(
                    "平台重复: {platform} — 每个平台只能提交一版"
                )));
            }
            let title = item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let content = item
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if title.is_empty() {
                return Ok(ToolResult::error(format!("{platform} 的标题为空")));
            }
            if content.trim().is_empty() {
                return Ok(ToolResult::error(format!("{platform} 的正文为空")));
            }
            parsed.push(ParsedItem {
                platform,
                title,
                content,
            });
        }

        let raw_dir = args
            .get("output_dir")
            .and_then(|d| d.as_str())
            .unwrap_or("content-pack");
        let out_dir =
            super::permissions::resolve_target(context.workspace.as_deref(), raw_dir, None);
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| AppError::Internal(format!("Failed to create output dir: {e}")))?;

        let mut reports = Vec::with_capacity(parsed.len());
        for item in &parsed {
            let file_name = format!("{}_{}.md", item.platform, slugify(&item.title));
            let file_path = out_dir.join(&file_name);
            let rules = check_platform(&item.platform, &item.title, &item.content);
            tokio::fs::write(&file_path, normalize_content(&item.content).as_bytes())
                .await
                .map_err(|e| AppError::Internal(format!("Failed to write {}: {e}", file_path.display())))?;
            reports.push(PlatformReport::new(&item.platform, file_name, rules));
        }
        super::permissions::record_output(context, &out_dir);

        let manifest_path = out_dir.join("manifest.json");
        tokio::fs::write(&manifest_path, build_manifest(&reports).as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("Failed to write {}: {e}", manifest_path.display())))?;

        Ok(ToolResult::success(format_report(&reports, &out_dir)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule<'a>(report: &'a [RuleResult], id: &str) -> &'a RuleResult {
        report.iter().find(|r| r.id == id).expect("rule present")
    }

    #[test]
    fn xhs_title_length_boundary() {
        let ok19 = check_xiaohongshu(&"字".repeat(19), "内容\n\n你们怎么选？");
        assert!(rule(&ok19, "title_length").ok);
        let ok20 = check_xiaohongshu(&"字".repeat(20), "内容\n\n你们怎么选？");
        assert!(rule(&ok20, "title_length").ok);
        let fail21 = check_xiaohongshu(&"字".repeat(21), "内容\n\n你们怎么选？");
        assert!(!rule(&fail21, "title_length").ok);
    }

    #[test]
    fn wechat_title_range() {
        let short = check_wechat(&"字".repeat(7), "标题\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(!rule(&short, "title_length").ok);
        let ok = check_wechat(&"字".repeat(8), "标题\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(rule(&ok, "title_length").ok);
        let long = check_wechat(&"字".repeat(21), "标题\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(!rule(&long, "title_length").ok);
    }

    #[test]
    fn wechat_requires_subheadings_and_three_paragraphs() {
        let no_sub = check_wechat("标题", "一段\n\n二段");
        assert!(!rule(&no_sub, "subheadings").ok);
        assert!(!rule(&no_sub, "min_paragraphs").ok);
        let ok = check_wechat("标题", "一段\n\n二段\n\n# 小标题\n\n三段");
        assert!(rule(&ok, "subheadings").ok);
        assert!(rule(&ok, "min_paragraphs").ok);
    }

    #[test]
    fn wechat_filler_opener_warns() {
        let filled = check_wechat("标题", "在当今社会，竞争日益激烈\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(!rule(&filled, "hook").ok);
        let good = check_wechat("标题", "我花 3 天跑了 20 个资料源\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(rule(&good, "hook").ok);
    }

    #[test]
    fn xhs_paragraph_line_estimate() {
        let long_para = "长".repeat(120);
        let bad = check_xiaohongshu("标题", &format!("{long_para}\n\n你们怎么选？"));
        assert!(!rule(&bad, "paragraph_lines").ok);
        let short = check_xiaohongshu("标题", "短句\n\n你们怎么选？");
        assert!(rule(&short, "paragraph_lines").ok);
    }

    #[test]
    fn emoji_counting_and_balance() {
        assert_eq!(count_emoji("👍 不错 🚀"), 2);
        assert_eq!(count_emoji("纯文字没有表情"), 0);
        let balanced = check_xiaohongshu("标题", "第一段 👍\n\n第二段 🚀\n\n你们怎么选？");
        assert!(rule(&balanced, "emoji_balance").ok);
        let over = check_xiaohongshu("标题", "一堆表情 😀😀😀😀😀\n\n你们怎么选？");
        assert!(!rule(&over, "emoji_balance").ok);
    }

    #[test]
    fn xhs_cta_question_detected() {
        let with_q = check_xiaohongshu("标题", "正文\n\n你们会怎么选？");
        assert!(rule(&with_q, "cta_question").ok);
        let without = check_xiaohongshu("标题", "正文\n\n以上就是全部内容");
        assert!(!rule(&without, "cta_question").ok);
    }

    #[test]
    fn traceable_source_detection() {
        let with_url = check_wechat("标题", "数据来自 https://example.com\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(rule(&with_url, "traceable").ok);
        let marked = check_wechat("标题", "该数字未证实\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(rule(&marked, "traceable").ok);
        let none = check_wechat("标题", "纯断言无来源\n\n正文\n\n# 小标题\n\n结尾有总结");
        assert!(!rule(&none, "traceable").ok);
    }

    #[test]
    fn zhihu_conclusion_first() {
        let verbose = check_zhihu("标题", &format!("铺垫铺垫{}{}", "字".repeat(100), "很长"));
        assert!(!rule(&verbose, "conclusion_first").ok);
        let punchy = check_zhihu("标题", "结论：直接给答案\n\n论证\n\n引用 https://example.com");
        assert!(rule(&punchy, "conclusion_first").ok);
    }

    #[test]
    fn slugify_sanitizes_and_falls_back() {
        assert_eq!(slugify("调研/报告?"), "调研_报告");
        assert_eq!(slugify("Q3 增长 15%"), "Q3_增长_15%");
        assert_eq!(slugify("   "), "untitled");
        assert_eq!(slugify("<<>>"), "untitled");
    }

    #[test]
    fn report_passed_flags_and_warnings() {
        let good = PlatformReport::new(
            PLATFORM_XIAOHONGSHU,
            "x.md".into(),
            check_xiaohongshu("标题", "第一段 👍\n\n第二段 🚀\n\n你们怎么选？"),
        );
        assert!(good.passed);
        assert_eq!(good.warnings, 0);
        let bad = PlatformReport::new(
            PLATFORM_WECHAT,
            "w.md".into(),
            check_wechat("标题", "在当今社会\n\n正文\n\n正文"),
        );
        assert!(!bad.passed);
        assert!(bad.warnings >= 1);
    }

    #[test]
    fn manifest_contains_status_per_platform() {
        let good = PlatformReport::new(
            PLATFORM_WECHAT,
            "w.md".into(),
            check_wechat(
                "公众号标题测试标题文",
                "一段\n\n二段\n\n# 小标题\n\n三段\n\n结尾有总结",
            ),
        );
        let m = build_manifest(&[good]);
        assert!(m.contains("\"wechat\""));
        assert!(m.contains("\"status\": \"pass\""));
        assert!(m.contains("\"file\": \"w.md\""));
    }

    #[test]
    fn normalize_content_uses_lf_and_trailing_newline() {
        assert_eq!(normalize_content("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(normalize_content("a\nb"), "a\nb\n");
    }

    #[test]
    fn effective_width_mixed() {
        assert_eq!(effective_width("中文"), 2.0);
        assert_eq!(effective_width("ab"), 1.0);
        assert_eq!(effective_width("中a"), 1.5);
    }
}
