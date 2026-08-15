//! doc_consistency — 长文档跨章一致性校验（只读）。
//!
//! 读 docx / md / txt，按章节切分，检测跨章重复段落、结构完整性
//! （required_sections）、章节编号连续性。断点即断——交付长文档前先跑它。

use crate::core::error::{AppError, AppResult};
use crate::toolkit::{Tool, ToolContext, ToolResult, ToolScope};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// 一个文档块：标题（level 1-6）或正文（level 0）。
struct Block {
    is_heading: bool,
    level: usize,
    text: String,
}

/// 跨章重复段落。
struct Dup {
    text: String,
    chapters: Vec<String>,
}

/// 识别段落 XML 里的 `w:pStyle` 值（小写）。
fn extract_pstyle(para_xml: &str) -> Option<String> {
    let lower = para_xml.to_ascii_lowercase();
    let ppr_start = lower.find("<w:ppr")?;
    let ppr_end = lower[ppr_start..].find("</w:ppr>")? + ppr_start;
    let ppr = &lower[ppr_start..ppr_end];
    let style_pos = ppr.find("w:pstyle")?;
    let after = &ppr[style_pos..];
    let val_pos = after.find("w:val=\"")? + 7;
    let val_end = after[val_pos..].find('"')? + val_pos;
    Some(after[val_pos..val_end].to_string())
}

fn chinese_numeral(c: char) -> Option<usize> {
    match c {
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        '十' => Some(10),
        _ => None,
    }
}

/// 判断一个段落是否为标题。优先 pStyle（HeadingN / Heading N / Title），
/// 回退文本特征（第N章 / N. / 一、）。
fn detect_heading(para_xml: &str, text: &str) -> Option<usize> {
    if let Some(style) = extract_pstyle(para_xml) {
        if style == "title" {
            return Some(1);
        }
        if let Some(rest) = style.strip_prefix("heading") {
            let rest = rest.trim_start();
            if let Ok(lvl) = rest.parse::<usize>() {
                if (1..=6).contains(&lvl) {
                    return Some(lvl);
                }
            }
        }
    }
    let chars: Vec<char> = text.trim_start().chars().collect();
    if chars.len() >= 3 && chars[0] == '第' {
        let tail = chars[1];
        let sep = chars[2];
        if (sep == '章' || sep == '篇')
            && (tail.is_ascii_digit() || chinese_numeral(tail).is_some())
        {
            return Some(1);
        }
    }
    if chars.first().is_some_and(|&c| c.is_ascii_digit())
        && chars.len() >= 2
        && matches!(chars[1], '.' | '、' | '．')
    {
        return Some(1);
    }
    if chars.first().is_some_and(|&c| chinese_numeral(c).is_some())
        && chars.len() >= 2
        && chars[1] == '、'
    {
        return Some(1);
    }
    None
}

/// 解析 docx 为块列表（空段落跳过）。
fn scan_docx_blocks(xml: &str) -> Vec<Block> {
    super::docx_edit::scan_paragraphs(xml)
        .into_iter()
        .filter(|p| !p.text.trim().is_empty())
        .map(|p| {
            let para_xml = &xml[p.start..p.end];
            let text = p.text.trim().to_string();
            match detect_heading(para_xml, &text) {
                Some(level) => Block {
                    is_heading: true,
                    level,
                    text,
                },
                None => Block {
                    is_heading: false,
                    level: 0,
                    text,
                },
            }
        })
        .collect()
}

/// 解析 markdown/纯文本为块列表（`#` 标题级数，照抄 docx_generate 模式）。
fn scan_text_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count().min(6);
            let heading = trimmed[level..].trim().to_string();
            if !heading.is_empty() {
                blocks.push(Block {
                    is_heading: true,
                    level,
                    text: heading,
                });
            }
        } else {
            blocks.push(Block {
                is_heading: false,
                level: 0,
                text: trimmed.to_string(),
            });
        }
    }
    blocks
}

/// 按扩展名分发文档解析（docx / md / txt；其余报错）。
fn parse_document(path: &Path) -> AppResult<Vec<Block>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if ext == "docx" {
        let xml = super::docx_edit::read_document_xml(path).map_err(AppError::Internal)?;
        return Ok(scan_docx_blocks(&xml));
    }
    if matches!(ext.as_str(), "md" | "markdown" | "txt") {
        let bytes = std::fs::read(path)
            .map_err(|e| AppError::Internal(format!("Cannot read {}: {e}", path.display())))?;
        return Ok(scan_text_blocks(&crate::core::encoding::decode_native_output(&bytes)));
    }
    Err(format!(
        "Unsupported document format: .{ext}（支持 docx / md / txt）"
    )
    .into())
}

/// 每个标题开新「章」（标题文本作章名）；首个标题前的正文归入「（前言）」。
fn split_chapters(blocks: &[Block]) -> Vec<(String, Vec<&Block>)> {
    let mut chapters: Vec<(String, Vec<&Block>)> = Vec::new();
    for block in blocks {
        if block.is_heading {
            chapters.push((block.text.clone(), Vec::new()));
        } else if let Some((_, body)) = chapters.last_mut() {
            body.push(block);
        } else {
            chapters.push(("（前言）".to_string(), vec![block]));
        }
    }
    chapters
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '！' | '？' | '；' | '：' | '、' | '「' | '」' | '『' | '』'
            | '（' | '）' | '《' | '》' | '\u{201C}' | '\u{201D}' | '\'' | '.' | ','
            | '!' | '?' | ';' | ':' | '·' | '—' | '-' | '…' | '–'
    )
}

/// 段落归一化指纹：小写 + 去空白 + 去常见标点（供重复检测）。
fn normalize_for_dup(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !is_punct(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 跨章重复段落：正文块指纹（≥20 字符）出现在 ≥2 个不同章 → 报。
fn find_cross_chapter_duplicates(chapters: &[(String, Vec<&Block>)]) -> Vec<Dup> {
    let mut seen: HashMap<String, (String, Vec<String>)> = HashMap::new();
    for (chapter, body) in chapters {
        for block in body {
            let fp = normalize_for_dup(&block.text);
            if fp.chars().count() < 20 {
                continue;
            }
            let entry = seen
                .entry(fp)
                .or_insert_with(|| (block.text.clone(), Vec::new()));
            if !entry.1.contains(chapter) {
                entry.1.push(chapter.clone());
            }
        }
    }
    seen.into_iter()
        .filter(|(_, (_, chapters))| chapters.len() >= 2)
        .map(|(_, (text, chapters))| Dup { text, chapters })
        .collect()
}

/// 结构完整性：每个 required_section 对任一标题子串匹配，缺失 → 报。
fn check_structure(headings: &[&Block], required_sections: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    for section in required_sections {
        let s = section.trim();
        if s.is_empty() {
            continue;
        }
        if !headings.iter().any(|h| h.text.contains(s)) {
            missing.push(format!("缺少章节：「{s}」（在标题中未找到）"));
        }
    }
    missing
}

/// 章节编号连续性：取一级标题的 `第N章`/`N.`/`一、` 编号，按序检查 1,2,3…
/// 无数值标题则跳过。
fn check_numbering(level1: &[&Block]) -> Vec<String> {
    let nums: Vec<(usize, &str)> = level1
        .iter()
        .filter_map(|h| heading_number(&h.text).map(|n| (n, h.text.as_str())))
        .collect();
    if nums.is_empty() {
        return Vec::new();
    }
    let mut issues = Vec::new();
    let mut expected = 1usize;
    for (num, text) in &nums {
        if *num != expected {
            issues.push(format!("章节编号跳号：「{text}」（应为 {expected}）"));
        }
        expected = num + 1;
    }
    issues
}

fn heading_number(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.trim_start().chars().collect();
    if chars.len() >= 3 && chars[0] == '第' {
        let sep = chars[2];
        if sep == '章' || sep == '篇' {
            if let Some(n) = chars[1].to_digit(10) {
                return Some(n as usize);
            }
            if let Some(n) = chinese_numeral(chars[1]) {
                return Some(n);
            }
        }
    }
    if let Some(n) = chars.first().and_then(|&c| c.to_digit(10)) {
        if chars.len() >= 2 && matches!(chars[1], '.' | '、' | '．') {
            return Some(n as usize);
        }
    }
    if let Some(n) = chars.first().and_then(|&c| chinese_numeral(c)) {
        if chars.len() >= 2 && chars[1] == '、' {
            return Some(n);
        }
    }
    None
}

fn build_report(
    chapters: &[(String, Vec<&Block>)],
    dups: &[Dup],
    structure: &[String],
    numbering: &[String],
    path: &Path,
) -> String {
    let mut lines = vec![format!(
        "长文档一致性校验：{}\n章节数：{}",
        path.display(),
        chapters.len()
    )];
    let mut clean = true;
    if !dups.is_empty() {
        lines.push(format!("\n--- 跨章重复段落（{} 处）---", dups.len()));
        for d in dups {
            lines.push(format!(
                "- 「{}」\n  出现在：{}",
                super::office_host::truncate(&d.text, 60),
                d.chapters.join("、")
            ));
        }
        clean = false;
    }
    if !structure.is_empty() {
        lines.push("\n--- 结构完整性 ---".to_string());
        for s in structure {
            lines.push(format!("- {s}"));
        }
        clean = false;
    }
    if !numbering.is_empty() {
        lines.push("\n--- 章节编号连续性 ---".to_string());
        for n in numbering {
            lines.push(format!("- {n}"));
        }
        clean = false;
    }
    if clean {
        lines.push("\n✓ 通过：无跨章重复、结构完整、编号连续。".to_string());
    } else {
        lines.push("\n✗ 待修复：见上方发现。".to_string());
    }
    lines.join("\n")
}

pub struct DocConsistencyTool;

impl DocConsistencyTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DocConsistencyTool {
    fn name(&self) -> &str {
        "doc_consistency"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        "长文档跨章一致性校验（只读）：读 docx / md / txt，检测跨章重复段落、\
         结构完整性（required_sections）、章节编号连续性，返回一致性报告。\
         交付长文档前先跑它，修完问题再交付。\
         Parameters: path (required), required_sections (可选，必须出现的章节标题数组)。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "docx / md / txt 文档路径。"},
                "required_sections": {"type": "array", "items": {"type": "string"}, "description": "必须出现的章节标题（对标题子串匹配）。"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_raw = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let required_sections: Vec<String> = args
            .get("required_sections")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let path = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path_raw);
        let blocks = parse_document(&path)?;
        if blocks.is_empty() {
            return Ok(ToolResult::success(format!(
                "一致性校验（{}）：文档为空或无法解析。",
                path.display()
            )));
        }

        let chapters = split_chapters(&blocks);
        let dups = find_cross_chapter_duplicates(&chapters);
        let headings: Vec<&Block> = blocks.iter().filter(|b| b.is_heading).collect();
        let level1: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.is_heading && b.level == 1)
            .collect();
        let structure = check_structure(&headings, &required_sections);
        let numbering = check_numbering(&level1);
        let report = build_report(&chapters, &dups, &structure, &numbering, &path);
        Ok(ToolResult::success(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(level: usize, text: &str) -> Block {
        Block {
            is_heading: true,
            level,
            text: text.to_string(),
        }
    }

    #[test]
    fn normalize_for_dup_ignores_punctuation_case_whitespace() {
        assert_eq!(
            normalize_for_dup("结论：这是重点！"),
            normalize_for_dup("结论这是重点")
        );
        assert_eq!(normalize_for_dup("API 接口"), normalize_for_dup("api接口"));
    }

    #[test]
    fn scan_text_blocks_parses_markdown_headings() {
        let blocks = scan_text_blocks("# 第一章\n正文内容\n\n## 小节\n更多内容\n");
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].is_heading, true);
        assert_eq!(blocks[0].level, 1);
        assert_eq!(blocks[0].text, "第一章");
        assert_eq!(blocks[1].is_heading, false);
        assert_eq!(blocks[2].level, 2);
        assert_eq!(blocks[2].text, "小节");
    }

    #[test]
    fn detect_heading_matches_pstyle_and_text_features() {
        assert_eq!(
            detect_heading(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>X</w:t></w:r></w:p>"#,
                "X"
            ),
            Some(1)
        );
        assert_eq!(
            detect_heading(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading 2"/></w:pPr></w:p>"#,
                ""
            ),
            Some(2)
        );
        assert_eq!(
            detect_heading(r#"<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr></w:p>"#, ""),
            Some(1)
        );
        assert_eq!(
            detect_heading(r#"<w:p><w:r><w:t>第1章 简介</w:t></w:r></w:p>"#, "第1章 简介"),
            Some(1)
        );
        assert_eq!(
            detect_heading(r#"<w:p><w:r><w:t>1. 概述</w:t></w:r></w:p>"#, "1. 概述"),
            Some(1)
        );
        assert_eq!(
            detect_heading(r#"<w:p><w:r><w:t>普通段落内容</w:t></w:r></w:p>"#, "普通段落内容"),
            None
        );
    }

    #[test]
    fn cross_chapter_duplicates_flagged_and_short_skipped() {
        let blocks = scan_text_blocks(
            "## 第一章\n这是第一章的段落内容，主要讲述整个项目的背景信息与目标。\n\n\
             ## 第二章\n这是第一章的段落内容，主要讲述整个项目的背景信息与目标。\n\n\
             ## 第三章\n完全不同的内容在这里。",
        );
        let chapters = split_chapters(&blocks);
        let dups = find_cross_chapter_duplicates(&chapters);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].chapters, vec!["第一章".to_string(), "第二章".to_string()]);

        let short = scan_text_blocks("## 一\n短句\n\n## 二\n短句");
        let chapters = split_chapters(&short);
        assert!(find_cross_chapter_duplicates(&chapters).is_empty());
    }

    #[test]
    fn structure_missing_required_section() {
        let h = heading(1, "引言");
        let missing = check_structure(&vec![&h], &["结论".to_string(), "引言".to_string()]);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("结论"));
    }

    #[test]
    fn numbering_flags_gap_and_accepts_sequence() {
        let b1 = heading(1, "第1章 背景");
        let b2 = heading(1, "第3章 结论");
        let issues = check_numbering(&vec![&b1, &b2]);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("应为 2"));

        let a = heading(1, "1. 背景");
        let b = heading(1, "2. 结论");
        assert!(check_numbering(&vec![&a, &b]).is_empty());

        let unnumbered = heading(1, "背景");
        assert!(check_numbering(&vec![&unnumbered]).is_empty());
    }

    #[test]
    fn report_lists_findings_and_marks_fixable() {
        let chapters = vec![
            ("第一章".to_string(), vec![]),
            ("第二章".to_string(), vec![]),
        ];
        let dup = Dup {
            text: "重复段落内容".to_string(),
            chapters: vec!["第一章".to_string(), "第二章".to_string()],
        };
        let report = build_report(
            &chapters,
            &[dup],
            &["缺少章节：「结论」".to_string()],
            &[],
            Path::new("x.md"),
        );
        assert!(report.contains("跨章重复段落"));
        assert!(report.contains("结构完整性"));
        assert!(report.contains("待修复"));
        assert!(report.contains("章节数：2"));
    }
}
