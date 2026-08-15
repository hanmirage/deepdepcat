//! citation_link — 引用与文档内引注联动。
//!
//! 解析文档里的 `[#id]` 引用标记 → 查资料夹解析条目 → 渲染编号参考列表
//! （markdown / gb7714 / apa / bibtex）→ 返回「编号映射 + 断链报告 +
//! 引用替换后的正文」。断链即断——agent 必须修好断链才能交付。

use crate::bootstrap::AppState;
use crate::core::error::{AppError, AppResult};
use crate::storage::database::ResearchItem;
use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use tauri::Manager;

/// 一个 `[#id]` 引用组的字节范围与其内 id 列表（保持原文顺序）。
struct CitationGroup {
    start: usize,
    end: usize,
    ids: Vec<i64>,
}

fn citation_regex() -> Regex {
    Regex::new(r"\[#([0-9，,\s]+)\]").expect("valid citation marker regex")
}

/// 提取正文里的 `[#id]`（支持 `[#12]` / `[#12, 15]` / `[#12][#15]`），
/// 返回首次出现顺序的去重 id 列表 + 每个引用组的字节范围。
fn parse_citations(content: &str) -> (Vec<i64>, Vec<CitationGroup>) {
    let re = citation_regex();
    let mut ids: Vec<i64> = Vec::new();
    let mut groups: Vec<CitationGroup> = Vec::new();
    for m in re.find_iter(content) {
        let inner = &content[m.start() + 2..m.end() - 1];
        let group_ids: Vec<i64> = inner
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<i64>().ok())
            .collect();
        if group_ids.is_empty() {
            continue;
        }
        for id in &group_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        groups.push(CitationGroup {
            start: m.start(),
            end: m.end(),
            ids: group_ids,
        });
    }
    (ids, groups)
}

/// 把每个引用组替换为编号形式：`[#12]` → `[1]`，`[#3, 5]` → `[2, 3]`；
/// 断链 id → `[?]`（报告里另有明细，正文里标出待修位置）。
fn rewrite_citations(
    content: &str,
    groups: &[CitationGroup],
    id_to_index: &HashMap<i64, usize>,
) -> String {
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0;
    for g in groups {
        out.push_str(&content[cursor..g.start]);
        let inner: Vec<String> = g
            .ids
            .iter()
            .map(|id| match id_to_index.get(id) {
                Some(i) => (i + 1).to_string(),
                None => "?".to_string(),
            })
            .collect();
        out.push_str(&format!("[{}]", inner.join(", ")));
        cursor = g.end;
    }
    out.push_str(&content[cursor..]);
    out
}

/// 按引用顺序渲染编号参考列表。gb7714/apa/bibtex 复用 research.rs 的
/// 纯函数；markdown 自写。
fn render_references(items: &[ResearchItem], format: &str, today: &str) -> String {
    match format {
        "gb7714" => super::research::export_gb7714(items, today),
        "apa" => super::research::export_apa(items, today),
        "bibtex" => super::research::export_bibtex(items),
        _ => {
            let mut out = String::from("# 参考文献\n\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&format!(
                    "[{}] {} — {}，{}（访问日期 {today}）\n",
                    i + 1,
                    item.title,
                    item.source,
                    item.url
                ));
            }
            out
        }
    }
}

fn build_report(
    ids: &[i64],
    id_to_index: &HashMap<i64, usize>,
    broken: &[i64],
    uncited: usize,
    rewritten: &str,
    path: &Path,
) -> String {
    let mut lines = vec![format!(
        "引用联动完成：解析 {} 处引用标记（{} 条断链）",
        ids.len(),
        broken.len()
    )];
    let mapping: Vec<String> = ids
        .iter()
        .filter_map(|id| id_to_index.get(id).map(|i| format!("#{id} → [{}]", i + 1)))
        .collect();
    if !mapping.is_empty() {
        lines.push(format!("编号映射：{}", mapping.join("，")));
    }
    if !broken.is_empty() {
        let b: Vec<String> = broken.iter().map(|id| format!("#{id}")).collect();
        lines.push(format!(
            "断链：{}（资料夹中不存在，请先 research_save 或修正标记）",
            b.join("，")
        ));
    }
    if uncited > 0 {
        lines.push(format!("资料夹还有 {uncited} 条来源未被引用（如需请补引）。"));
    }
    lines.push(format!("参考列表已写入 {}", path.display()));
    lines.push("\n--- 引用替换后的正文 ---".to_string());
    lines.push(rewritten.to_string());
    lines.join("\n")
}

pub struct CitationLinkTool;

impl CitationLinkTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CitationLinkTool {
    fn name(&self) -> &str {
        "citation_link"
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "引用联动：把正文里的 [#id] 引用标记解析成资料夹条目，渲染编号参考列表 \
         （markdown/gb7714/apa/bibtex），返回编号映射、断链报告与引用替换后的正文。\
         断链即断——agent 必须修好断链（research_save 补源或改标记）才能交付。\
         Parameters: content (required, 含 [#id] 标记的正文), format (可选), \
         path (可选，参考列表输出路径)。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "含 [#id] 引用标记的文档正文（如 [#12]、[#12, 15]）。"},
                "format": {"type": "string", "enum": ["markdown", "gb7714", "apa", "bibtex"], "description": "参考列表格式（默认 markdown）。"},
                "path": {"type": "string", "description": "参考列表输出路径（默认 references.md；bibtex 默认 references.bib）。"}
            },
            "required": ["content"]
        })
    }

    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let raw = args
            .get("path")
            .and_then(|p| p.as_str())
            .filter(|p| !p.trim().is_empty())
            .unwrap_or("references.md");
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, None);
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;
        let (ids, groups) = parse_citations(content);
        if ids.is_empty() {
            return Ok(ToolResult::error(
                "未发现 [#id] 引用标记。请在正文里用 [#id] 标记资料夹条目（如 [#12]）。"
                    .to_string(),
            ));
        }
        let format = args
            .get("format")
            .and_then(|f| f.as_str())
            .map(str::to_lowercase)
            .unwrap_or_else(|| "markdown".to_string());
        let path = args
            .get("path")
            .and_then(|p| p.as_str())
            .filter(|p| !p.trim().is_empty())
            .unwrap_or(if format == "bibtex" {
                "references.bib"
            } else {
                "references.md"
            });

        let state = context.app.state::<AppState>();
        let mut id_to_index: HashMap<i64, usize> = HashMap::new();
        let mut resolved_items: Vec<ResearchItem> = Vec::new();
        let mut broken: Vec<i64> = Vec::new();
        for id in &ids {
            match crate::storage::database::get_research_item(
                &state.db,
                &context.session_id,
                *id,
            )? {
                Some(item) => {
                    id_to_index.insert(*id, resolved_items.len());
                    resolved_items.push(item);
                }
                None => broken.push(*id),
            }
        }

        let all = crate::storage::database::list_research_items(
            &state.db,
            &context.session_id,
            None,
            200,
        )?;
        let uncited = all
            .iter()
            .filter(|it| !id_to_index.contains_key(&it.id))
            .count();

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let refs_text = render_references(&resolved_items, &format, &today);

        let mut out_path =
            super::permissions::resolve_target(context.workspace.as_deref(), path, None);
        if out_path.extension().is_none() {
            out_path.set_extension(if format == "bibtex" { "bib" } else { "md" });
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Internal(format!("Failed to create output dir: {e}"))
            })?;
        }
        tokio::fs::write(&out_path, refs_text.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("Failed to write {}: {e}", out_path.display())))?;
        super::permissions::record_output(context, &out_path);

        let rewritten = rewrite_citations(content, &groups, &id_to_index);
        let report = build_report(&ids, &id_to_index, &broken, uncited, &rewritten, &out_path);
        Ok(ToolResult::success(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::database::ResearchItem;

    fn item(id: i64, title: &str) -> ResearchItem {
        ResearchItem {
            id,
            session_id: "s1".to_string(),
            title: title.to_string(),
            url: format!("https://example.com/{id}"),
            source: "web".to_string(),
            snippet: String::new(),
            snapshot: String::new(),
            tags: String::new(),
            created_at: "2026-08-08T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn parses_single_and_multi_citations() {
        let (ids, groups) = parse_citations("观点[#12]与[#3, 5]见[#12][#7]。");
        assert_eq!(ids, vec![12, 3, 5, 7]);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].ids, vec![12]);
        assert_eq!(groups[1].ids, vec![3, 5]);
        assert_eq!(groups[2].ids, vec![12]);
        assert_eq!(groups[3].ids, vec![7]);
    }

    #[test]
    fn parses_chinese_comma_and_space_separators() {
        let (ids, _) = parse_citations("[#12，15] [#18]");
        assert_eq!(ids, vec![12, 15, 18]);
    }

    #[test]
    fn no_markers_yields_empty() {
        let (ids, groups) = parse_citations("纯文本没有引用标记");
        assert!(ids.is_empty());
        assert!(groups.is_empty());
    }

    #[test]
    fn rewrite_maps_ids_to_numbers() {
        let mut map = HashMap::new();
        map.insert(12, 0usize);
        map.insert(3, 1usize);
        map.insert(5, 2usize);
        map.insert(7, 3usize);
        let (_, groups) = parse_citations("观点[#12]与[#3, 5]见[#12][#7]。");
        let out = rewrite_citations("观点[#12]与[#3, 5]见[#12][#7]。", &groups, &map);
        assert_eq!(out, "观点[1]与[2, 3]见[1][4]。");    }

    #[test]
    fn rewrite_marks_broken_ids_as_question() {
        let map = HashMap::new();
        let (_, groups) = parse_citations("见[#99]");
        let out = rewrite_citations("见[#99]", &groups, &map);
        assert_eq!(out, "见[?]");
    }

    #[test]
    fn render_markdown_references_numbers_in_order() {
        let refs = render_references(&[item(12, "A"), item(3, "B")], "markdown", "2026-08-15");
        assert!(refs.contains("# 参考文献"));
        assert!(refs.contains("[1] A"));
        assert!(refs.contains("[2] B"));
        assert!(refs.contains("访问日期 2026-08-15"));
    }

    #[test]
    fn render_gb7714_reuses_export() {
        let refs = render_references(&[item(12, "A")], "gb7714", "2026-08-15");
        assert!(refs.contains("参考文献（GB/T 7714）"));
        assert!(refs.contains("[1] A"));
    }

    #[test]
    fn report_includes_mapping_broken_and_uncited() {
        let mut map = HashMap::new();
        map.insert(12, 0usize);
        let report = build_report(
            &[12, 99],
            &map,
            &[99],
            2,
            "正文[1][?]",
            Path::new("references.md"),
        );
        assert!(report.contains("#12 → [1]"));
        assert!(report.contains("断链：#99"));
        assert!(report.contains("2 条来源未被引用"));
        assert!(report.contains("正文[1][?]"));
    }
}
