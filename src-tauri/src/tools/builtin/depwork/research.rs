//! Research tools — Depwork 调研域：学术文献检索、资料夹、引用导出。
//!
//! - `research_search` — Semantic Scholar + Crossref 文献检索（固定公开
//!   域名 JSON API，SSRF 天然免疫；无需密钥）
//! - `research_save` / `research_list` / `research_remove` — 资料夹
//! - `research_export` — 带引用的 Markdown 导出（标题/来源/URL/访问日期）

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use crate::toolkit::ToolScope;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Manager;

const MAX_RESULTS: usize = 10;
const SNIPPET_MAX: usize = 300;

/// A normalized literature hit shared by both sources.
#[derive(Debug, Clone, serde::Serialize)]
struct ResearchHit {
    title: String,
    authors: String,
    year: Option<i64>,
    venue: String,
    source: String,
    url: String,
    doi: Option<String>,
    snippet: String,
}

fn snippet(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= SNIPPET_MAX {
        clean
    } else {
        format!("{}…", clean.chars().take(SNIPPET_MAX).collect::<String>())
    }
}

fn authors_line(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} & {}", names[0], names[1]),
        _ => format!("{} et al.", names[0]),
    }
}

/// ── Semantic Scholar ────────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct ScholarResponse {
    data: Vec<ScholarPaper>,
}

#[derive(Debug, Deserialize)]
struct ScholarPaper {
    #[serde(default)]
    title: String,
    #[serde(default)]
    authors: Vec<ScholarAuthor>,
    #[serde(default)]
    year: Option<i64>,
    #[serde(default)]
    venue: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "abstract")]
    abstract_text: Option<String>,
    #[serde(default)]
    external_ids: Option<ScholarIds>,
}

#[derive(Debug, Deserialize)]
struct ScholarAuthor {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct ScholarIds {
    #[serde(default, rename = "DOI")]
    doi: Option<String>,
}

async fn fetch_scholar(query: &str, limit: usize) -> AppResult<Vec<ResearchHit>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("DeepDepCat/1.0 (research tool)")
        .build()?;
    let resp: ScholarResponse = client
        .get("https://api.semanticscholar.org/graph/v1/paper/search")
        .query(&[
            ("query", query),
            ("limit", &limit.to_string()),
            (
                "fields",
                "title,authors,year,venue,url,abstract,externalIds",
            ),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Semantic Scholar request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Internal(format!("Semantic Scholar HTTP error: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("Semantic Scholar parse error: {e}")))?;

    Ok(resp
        .data
        .into_iter()
        .map(|p| ResearchHit {
            title: p.title,
            authors: authors_line(&p.authors.into_iter().map(|a| a.name).collect::<Vec<_>>()),
            year: p.year,
            venue: p.venue,
            source: "semantic_scholar".to_string(),
            url: p.url,
            doi: p.external_ids.and_then(|ids| ids.doi),
            snippet: snippet(p.abstract_text.as_deref().unwrap_or("")),
        })
        .collect())
}

/// ── Crossref ────────────────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct CrossrefResponse {
    message: CrossrefMessage,
}

#[derive(Debug, Deserialize)]
struct CrossrefMessage {
    #[serde(default)]
    items: Vec<CrossrefWork>,
}

#[derive(Debug, Deserialize)]
struct CrossrefWork {
    #[serde(default)]
    title: Vec<String>,
    #[serde(default)]
    author: Vec<CrossrefAuthor>,
    #[serde(default)]
    issued: Option<CrossrefDate>,
    #[serde(default, rename = "URL")]
    url: String,
    #[serde(default, rename = "DOI")]
    doi: String,
    #[serde(default, rename = "container-title")]
    container_title: Vec<String>,
    #[serde(default, rename = "abstract")]
    abstract_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CrossrefAuthor {
    #[serde(default)]
    family: String,
    #[serde(default)]
    given: String,
}

#[derive(Debug, Deserialize)]
struct CrossrefDate {
    #[serde(default, rename = "date-parts")]
    date_parts: Vec<Vec<i64>>,
}

async fn fetch_crossref(query: &str, limit: usize) -> AppResult<Vec<ResearchHit>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("DeepDepCat/1.0 (research tool; mailto:dev@deepdepcat.local)")
        .build()?;
    let resp: CrossrefResponse = client
        .get("https://api.crossref.org/works")
        .query(&[
            ("query", query),
            ("rows", &limit.to_string()),
            (
                "select",
                "title,author,issued,URL,DOI,container-title,abstract",
            ),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Crossref request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Internal(format!("Crossref HTTP error: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("Crossref parse error: {e}")))?;

    Ok(resp
        .message
        .items
        .into_iter()
        .map(|w| {
            let names = w
                .author
                .into_iter()
                .map(|a| {
                    if a.given.is_empty() {
                        a.family
                    } else {
                        format!("{} {}", a.given, a.family)
                    }
                })
                .collect::<Vec<_>>();
            let year = w
                .issued
                .and_then(|d| d.date_parts.first().cloned())
                .and_then(|parts| parts.first().copied());
            ResearchHit {
                title: w.title.first().cloned().unwrap_or_default(),
                authors: authors_line(&names),
                year,
                venue: w.container_title.first().cloned().unwrap_or_default(),
                source: "crossref".to_string(),
                url: if w.url.is_empty() {
                    format!("https://doi.org/{}", w.doi)
                } else {
                    w.url
                },
                doi: (!w.doi.is_empty()).then_some(w.doi),
                snippet: snippet(w.abstract_text.as_deref().unwrap_or("")),
            }
        })
        .collect())
}

/// ── General web search (Bing RSS — free, no API key) ────────────
async fn fetch_web(query: &str, limit: usize) -> AppResult<Vec<ResearchHit>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
        )
        .build()
        .map_err(|e| AppError::Internal(format!("Web search client error: {e}")))?;
    let url = format!(
        "https://cn.bing.com/search?q={}&format=rss",
        urlencoding::encode(query)
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Web search request failed: {e}")))?;
    let text = resp
        .error_for_status()
        .map_err(|e| AppError::Internal(format!("Web search HTTP error: {e}")))?
        .text()
        .await
        .map_err(|e| AppError::Internal(format!("Web search read failed: {e}")))?;
    let items = crate::tools::builtin::web_search::parse_bing_rss(&text, limit);
    Ok(items
        .into_iter()
        .map(|(title, url, desc)| ResearchHit {
            title,
            authors: String::new(),
            year: None,
            venue: "web".to_string(),
            source: "web".to_string(),
            url,
            doi: None,
            snippet: snippet(&crate::tools::builtin::web_search::strip_html(&desc)),
        })
        .collect())
}

/// ── Tools ───────────────────────────────────────────────────────
pub struct ResearchSearchTool;

impl ResearchSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchSearchTool {
    fn name(&self) -> &str {
        "research_search"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Search research sources and the web. Academic sources (Semantic \
         Scholar + Crossref) by default; use source=web for general web \
         search (industry trends, news, ordinary pages). Returns structured \
         hits with title, source, URL and a snippet — academic hits add \
         authors/year/venue/DOI. Then save what you keep with research_save."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query (topic, paper title, author)."},
                "max_results": {"type": "integer", "description": "Max hits per source (default 5, cap 10)."},
                "source": {"type": "string", "enum": ["all", "scholar", "crossref", "web"], "description": "Which API to query (default all = academic; web for general web search)."}
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| AppError::Parse("Missing non-empty 'query'".into()))?;
        let limit = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .map(|v| (v as usize).clamp(1, MAX_RESULTS))
            .unwrap_or(5);
        let source = args.get("source").and_then(|s| s.as_str()).unwrap_or("all");

        let mut hits = Vec::new();
        if matches!(source, "all" | "scholar") {
            hits.extend(fetch_scholar(query, limit).await?);
        }
        if matches!(source, "all" | "crossref") {
            hits.extend(fetch_crossref(query, limit).await?);
        }
        if source == "web" {
            hits.extend(fetch_web(query, limit).await?);
        }
        if hits.is_empty() {
            return Ok(ToolResult::success(
                "No literature results found — try broader terms or a different source."
                    .to_string(),
            ));
        }
        Ok(ToolResult::success(serde_json::to_string_pretty(&hits)?))
    }
}

pub struct ResearchSaveTool;

impl ResearchSaveTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchSaveTool {
    fn name(&self) -> &str {
        "research_save"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Save a research source into the session's 资料夹 (title, URL, \
         source, optional snippet/snapshot/tags). The saved item can be \
         listed (research_list), removed (research_remove) and exported \
         with citations (research_export). URLs must be public http(s); \
         new URLs are verified reachable before saving (clearly dead links \
         are rejected, anti-bot blocks are saved with a warning)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Human-readable title of the source."},
                "url": {"type": "string", "description": "Public http(s) URL of the source."},
                "source": {"type": "string", "description": "Origin label, e.g. semantic_scholar / crossref / web."},
                "snippet": {"type": "string", "description": "Short summary or key quote (what the source supports)."},
                "snapshot": {"type": "string", "description": "Optional content snapshot for offline reference."},
                "tags": {"type": "string", "description": "Comma-separated tags for filtering."}
            },
            "required": ["title", "url"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| AppError::Parse("Missing non-empty 'title'".into()))?;
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| AppError::Parse("Missing non-empty 'url'".into()))?;
        if let Err(reason) = crate::hooks::ssrf::validate_fetch_url(url) {
            return Ok(ToolResult::error(format!(
                "SSRF guard rejected URL: {reason}"
            )));
        }
        let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("web");
        let snippet = args.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
        let snapshot = args.get("snapshot").and_then(|v| v.as_str()).unwrap_or("");
        let tags = args.get("tags").and_then(|v| v.as_str()).unwrap_or("");

        let state = context.app.state::<AppState>();
        // Idempotence: saving the same url+source twice must not duplicate —
        // reuse the existing row instead of inserting a near-duplicate.
        let existing = crate::storage::database::list_research_items(
            &state.db,
            &context.session_id,
            None,
            200,
        )
        .ok()
        .into_iter()
        .flatten()
        .find(|item| item.url == url && item.source == source)
        .map(|item| item.id);
        if let Some(id) = existing {
            return Ok(ToolResult::success(format!(
                "Research item #{id} 已存在（同一 URL+来源，跳过重复）：{title}\nURL: {url}"
            )));
        }

        // New item — verify the URL is actually reachable before it enters
        // the 资料夹 (dead links poison every later citation). Only clearly
        // dead URLs are rejected; anti-bot blocks (403/429/5xx/timeout) are
        // allowed through with a warning so real sources are not lost.
        let warning = match super::web_fetch::verify_url_reachable(url).await? {
            super::web_fetch::UrlVerdict::Reachable => None,
            super::web_fetch::UrlVerdict::Blocked | super::web_fetch::UrlVerdict::TimedOut => {
                Some("URL 可能被反爬拦截或暂时不可达，已保存但请复核其可访问性。".to_string())
            }
            super::web_fetch::UrlVerdict::NotFound => {
                return Ok(ToolResult::error(format!(
                    "拒绝保存：URL 不存在（HTTP 404/410）：{url} —— 请核对链接或换一个来源。"
                )));
            }
            super::web_fetch::UrlVerdict::Unreachable => {
                return Ok(ToolResult::error(format!(
                    "拒绝保存：URL 无法连接（DNS/网络失败）：{url} —— 请核对链接或换一个来源。"
                )));
            }
        };
        let id = crate::storage::database::insert_research_item(
            &state.db,
            &context.session_id,
            title,
            url,
            source,
            snippet,
            snapshot,
            tags,
        )?;
        let mut msg =
            format!("Saved research item #{id}: {title}\nURL: {url}\nSource: {source}");
        if let Some(w) = warning {
            msg.push_str(&format!("\n⚠️ {w}"));
        }
        Ok(ToolResult::success(msg))
    }
}

pub struct ResearchListTool;

impl ResearchListTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchListTool {
    fn name(&self) -> &str {
        "research_list"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "List the session's saved research sources (newest first). Optional \
         tag filter. Returns id, title, url, source, snippet and tags — the \
         agent's working 资料夹."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tag": {"type": "string", "description": "Optional tag to filter by."},
                "limit": {"type": "integer", "description": "Max items (default 50)."}
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let tag = args.get("tag").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| (v as usize).clamp(1, 200))
            .unwrap_or(50);
        let state = context.app.state::<AppState>();
        let items = crate::storage::database::list_research_items(
            &state.db,
            &context.session_id,
            tag,
            limit,
        )?;
        if items.is_empty() {
            return Ok(ToolResult::success(
                "资料夹为空 — use research_save to add sources.".to_string(),
            ));
        }
        let view: Vec<Value> = items
            .into_iter()
            .map(|i| {
                json!({
                    "id": i.id,
                    "title": i.title,
                    "url": i.url,
                    "source": i.source,
                    "snippet": snippet(&i.snippet),
                    "tags": i.tags,
                })
            })
            .collect();
        Ok(ToolResult::success(serde_json::to_string_pretty(&view)?))
    }
}

pub struct ResearchRemoveTool;

impl ResearchRemoveTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchRemoveTool {
    fn name(&self) -> &str {
        "research_remove"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Remove a research item from the session's 资料夹 by id."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer", "description": "Item id from research_list."}
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AppError::Parse("Missing 'id'".into()))?;
        let state = context.app.state::<AppState>();
        let removed =
            crate::storage::database::remove_research_item(&state.db, &context.session_id, id)?;
        Ok(ToolResult::success(if removed {
            format!("Removed research item #{id}")
        } else {
            format!("Research item #{id} not found in this session")
        }))
    }
}

pub struct ResearchExportTool;

impl ResearchExportTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchExportTool {
    fn name(&self) -> &str {
        "research_export"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Export the session's research sources as a citation-ready Markdown \
         file (title, source, URL, access date, snippet), or as BibTeX / \
         GB-T 7714 / APA citations (format param). Writes to the given path \
         (relative to the workspace; defaults research_export.md / .bib) — \
         the file can then be opened/edited like any document."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Output path (default research_export.md; research_export.bib for BibTeX)."},
                "format": {"type": "string", "enum": ["markdown", "bibtex", "gb7714", "apa"], "description": "Citation format (default markdown)."}
            }
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let format = args
            .get("format")
            .and_then(|f| f.as_str())
            .map(str::to_lowercase)
            .unwrap_or_else(|| "markdown".to_string());
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|p| !p.trim().is_empty())
            .unwrap_or(if format == "bibtex" {
                "research_export.bib"
            } else {
                "research_export.md"
            });
        let state = context.app.state::<AppState>();
        let items = crate::storage::database::list_research_items(
            &state.db,
            &context.session_id,
            None,
            200,
        )?;
        if items.is_empty() {
            return Ok(ToolResult::error(
                "资料夹为空 — nothing to export. Use research_save first.".to_string(),
            ));
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let content = match format.as_str() {
            "bibtex" => export_bibtex(&items),
            "gb7714" => export_gb7714(&items, &today),
            "apa" => export_apa(&items, &today),
            _ => {
                let mut md = String::from("# 调研资料与引用\n\n");
                for item in &items {
                    md.push_str(&format!(
                        "- **{}** — {} ({}) · 访问日期 {}",
                        item.title, item.source, item.url, today
                    ));
                    if let Some(doi) = item.url.strip_prefix("https://doi.org/") {
                        md.push_str(&format!(" · DOI: {doi}"));
                    }
                    md.push('\n');
                    if !item.snippet.is_empty() {
                        md.push_str(&format!("  - {}\n", snippet(&item.snippet)));
                    }
                    if !item.tags.is_empty() {
                        md.push_str(&format!("  - 标签: {}\n", item.tags));
                    }
                }
                md
            }
        };

        let mut resolved = crate::tools::builtin::resolve_path(context.workspace.as_deref(), path);
        if resolved.extension().is_none() {
            resolved.set_extension(if format == "bibtex" { "bib" } else { "md" });
        }
        tokio::fs::write(&resolved, content.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("Failed to write export: {e}")))?;
        // Own-output: the agent must be able to edit its own export without
        // re-prompting (depwork's write gate treats it as session output).
        super::permissions::record_output(context, &resolved);
        Ok(ToolResult::success(format!(
            "Exported {} research sources to {}\n\n{}",
            items.len(),
            resolved.display(),
            content
        )))
    }
}

/// BibTeX export (`@misc` entries; DOI surfaced via the doi.org URL).
pub(crate) fn export_bibtex(items: &[crate::storage::database::ResearchItem]) -> String {
    let mut out = String::from("% 调研资料引用（BibTeX）\n");
    for item in items {
        let doi = item.url.strip_prefix("https://doi.org/");
        out.push_str(&format!(
            "@misc{{ddc{},\n  title = {{{}}},\n  howpublished = {{\\url{{{}}}}},\n",
            item.id,
            item.title.replace('&', "\\&"),
            item.url
        ));
        if let Some(doi) = doi {
            out.push_str(&format!("  doi = {{{doi}}},\n"));
        }
        out.push_str("}\n\n");
    }
    out
}

/// GB-T 7714 numeric citation list.
pub(crate) fn export_gb7714(items: &[crate::storage::database::ResearchItem], today: &str) -> String {
    let mut out = String::from("# 参考文献（GB/T 7714）\n\n");
    for (i, item) in items.iter().enumerate() {
        out.push_str(&format!(
            "[{}] {}[EB/OL]. {}, 访问日期 {}（{}）。\n",
            i + 1,
            item.title,
            item.source,
            today,
            item.url
        ));
    }
    out
}

/// APA-style reference list (retrieval-date form).
pub(crate) fn export_apa(items: &[crate::storage::database::ResearchItem], today: &str) -> String {
    let mut out = String::from("# References (APA)\n\n");
    for item in items {
        out.push_str(&format!(
            "{}. {}, Retrieved {today}, from {}\n",
            item.title, item.source, item.url
        ));
    }
    out
}

/// Search the session's 资料夹 by keyword — the missing "I saved it
/// somewhere, where was it?" tool once the folder grows past a handful of
/// sources.
pub struct ResearchFolderSearchTool;

impl ResearchFolderSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchFolderSearchTool {
    fn name(&self) -> &str {
        "research_folder_search"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Search the session's saved 资料夹 by keyword across title, URL, \
         source, snippet, snapshot and tags. Returns matching research \
         sources (id/title/url/source/snippet/tags). Use when research_list \
         grows large and you need to find a previously saved source."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": {"type": "string", "description": "Keyword to search for in the saved sources."},
                "limit": {"type": "integer", "description": "Max results (default 20)."}
            },
            "required": ["keyword"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if keyword.is_empty() {
            return Ok(ToolResult::error(
                "Missing 'keyword' — search the 资料夹 with a keyword.".to_string(),
            ));
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(100) as usize;
        let state = context.app.state::<AppState>();
        let items = crate::storage::database::search_research_items(
            &state.db,
            &context.session_id,
            keyword,
            limit,
        )?;
        if items.is_empty() {
            return Ok(ToolResult::success(format!(
                "资料夹中未找到包含「{keyword}」的来源。"
            )));
        }

        let mut out = format!(
            "在资料夹中找到 {} 条包含「{keyword}」的来源：\n",
            items.len()
        );
        for item in &items {
            out.push_str(&format!(
                "\n[#{}] {}\n来源: {} · {}\n",
                item.id, item.title, item.source, item.url
            ));
            if !item.snippet.is_empty() {
                out.push_str(&format!("摘要: {}\n", snippet(&item.snippet)));
            }
            if !item.tags.is_empty() {
                out.push_str(&format!("标签: {}\n", item.tags));
            }
        }
        Ok(ToolResult::success(out))
    }
}

/// Normalize a comma/space separated tag string into a clean comma list
/// (`"ml,  agents, web"` → `"ml,agents,web"`). Pure + unit-testable.
fn normalize_clip_tags(tags: &str) -> String {
    tags.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Clip a web page into the session's 资料夹 in one step: fetch (SSRF-safe,
/// redirect-revalidated), extract the title + readable body, and save as a
/// research item with a snippet + full snapshot. Closes the loop between
/// web research and the 资料夹 (M17 search can then find it).
pub struct ResearchClipTool;

impl ResearchClipTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchClipTool {
    fn name(&self) -> &str {
        "research_clip"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Clip a web page into the session's 资料夹: fetch the URL (SSRF-safe, \
         redirect-revalidated), extract title + readable body, and save it as \
         a research source with snippet and snapshot. Later find it with \
         research_folder_search and export with research_export. Parameters: \
         url (required), title (optional override), tags (optional comma \
         separated), max_chars (optional snapshot cap, default 20000)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Page URL to clip (https:// prefix optional)."},
                "title": {"type": "string", "description": "Optional title override; defaults to the page title."},
                "tags": {"type": "string", "description": "Optional comma separated tags, e.g. \"ml,agents\"."},
                "max_chars": {"type": "number", "description": "Snapshot size cap (default 20000)."}
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let url = args
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| "Missing required parameter: url".to_string())?;
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(20_000)
            .clamp(500, 100_000) as usize;
        let tags = args
            .get("tags")
            .and_then(|v| v.as_str())
            .map(normalize_clip_tags)
            .unwrap_or_default();

        let (target, page_title, body) =
            crate::tools::builtin::depwork::web_fetch::fetch_web_page(url, max_chars).await?;
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|t| !t.trim().is_empty())
            .map(|t| t.trim().to_string())
            .unwrap_or_else(|| {
                if page_title.trim().is_empty() {
                    target.clone()
                } else {
                    page_title.trim().to_string()
                }
            });
        let snippet: String = body.chars().take(400).collect();

        let state = context.app.state::<AppState>();
        // Idempotence: clipping the same page twice pollutes the folder —
        // match on session + url (clip source is fixed "web-clip"), re-using
        // the existing row instead of inserting a near-duplicate.
        let existing = crate::storage::database::list_research_items(
            &state.db,
            &context.session_id,
            None,
            200,
        )
        .ok()
        .into_iter()
        .flatten()
        .find(|item| item.url == target && item.source == "web-clip")
        .map(|item| item.id);
        let id = match existing {
            Some(id) => {
                return Ok(ToolResult::success(format!(
                    "已在资料夹 [#{id}]（同一 URL 已剪藏过，跳过重复）：{title}\nURL: {target}"
                )));
            }
            None => crate::storage::database::insert_research_item(
                &state.db,
                &context.session_id,
                &title,
                &target,
                "web-clip",
                &snippet,
                &body,
                &tags,
            )?,
        };

        Ok(ToolResult::success(format!(
            "已剪藏到资料夹 [#{id}]：{title}\nURL: {target}\n正文 {count} 字符{tags_line}\n\
             后续可用 research_folder_search 检索、research_export 导出。",
            id = id,
            title = title,
            target = target,
            count = body.chars().count(),
            tags_line = if tags.is_empty() {
                String::new()
            } else {
                format!(" · 标签: {tags}")
            },
        )))
    }
}

/// Assemble the citation-ready Markdown body for a research report.
fn build_report_markdown(
    intro: Option<&str>,
    items: &[crate::storage::database::ResearchItem],
) -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut md = format!("## 调研来源（{} 条）\n", items.len());
    if let Some(intro) = intro.map(str::trim).filter(|i| !i.is_empty()) {
        md.push_str(&format!("\n{intro}\n"));
    }
    for (i, item) in items.iter().enumerate() {
        md.push_str(&format!(
            "\n### {}. {}\n来源：{} · {}（访问日期 {}）\n",
            i + 1,
            item.title,
            item.source,
            item.url,
            today
        ));
        if !item.snippet.is_empty() {
            md.push_str(&format!("- 摘要：{}\n", snippet(&item.snippet)));
        }
        if !item.tags.is_empty() {
            md.push_str(&format!("- 标签：{}\n", item.tags));
        }
    }
    md
}

/// Generate a citation-ready Word report from the session's 资料夹 —
/// closes the调研 loop: clip/save → folder search → one-click docx.
pub struct ResearchReportTool;

impl ResearchReportTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchReportTool {
    fn name(&self) -> &str {
        "research_report"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Generate a citation-ready Word (.docx) report from the session's \
         资料夹: title, optional intro, and every saved source as a \
         numbered entry (source, URL, access date, snippet, tags). \
         Parameters: path (optional, default research_report.docx), title \
         (optional, default 调研报告), intro (optional), tag (optional \
         filter), max_items (default 100). Use after research_clip / \
         research_save."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Output path (default research_report.docx; .docx added if missing)."},
                "title": {"type": "string", "description": "Report title (default 调研报告)."},
                "intro": {"type": "string", "description": "Optional intro paragraph."},
                "tag": {"type": "string", "description": "Only include sources carrying this tag."},
                "max_items": {"type": "number", "description": "Max sources to include (default 100)."}
            }
        })
    }

    /// Self-approval mirrors the other docx writers: creating a NEW file
    /// (or overwriting this session's own draft) skips the prompt.
    fn self_approve(&self, args: &Value, context: &ToolContext) -> PermissionDecision {
        let Some(raw) = args.get("path").and_then(|p| p.as_str()) else {
            return PermissionDecision::Ask;
        };
        let target =
            super::permissions::resolve_target(context.workspace.as_deref(), raw, Some("docx"));
        super::permissions::write_target_decision(context, &target)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let path_raw = args
            .get("path")
            .and_then(|p| p.as_str())
            .filter(|p| !p.trim().is_empty())
            .unwrap_or("research_report.docx");
        let title = args
            .get("title")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("调研报告")
            .trim()
            .to_string();
        let intro = args.get("intro").and_then(|i| i.as_str());
        let tag = args
            .get("tag")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty());
        let max_items = args
            .get("max_items")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .min(500) as usize;

        let state = context.app.state::<AppState>();
        let items = crate::storage::database::list_research_items(
            &state.db,
            &context.session_id,
            tag,
            max_items,
        )?;
        if items.is_empty() {
            return Ok(ToolResult::error(
                "资料夹为空（或没有该标签的来源）— 先用 research_clip / research_save 添加资料。"
                    .to_string(),
            ));
        }

        let md = build_report_markdown(intro, &items);
        let path = super::permissions::resolve_target(
            context.workspace.as_deref(),
            path_raw,
            Some("docx"),
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("Failed to create output dir: {e}")))?;
        }
        crate::tools::builtin::depwork::docx_generate::write_docx(
            &path,
            &title,
            &md,
            context.workspace.as_deref(),
        )
        .map_err(|e| AppError::Internal(format!("Failed to write {}: {e}", path.display())))?;
        // Own-output: the report is the agent's own draft — later edits
        // (docx_edit) must not re-prompt as if it were a user file.
        super::permissions::record_output(context, &path);

        Ok(ToolResult::success(format!(
            "已生成调研报告：{}\n包含 {} 条来源。\n可用 doc_read 检查，或继续用 docx_edit 补充分析。",
            path.display(),
            items.len()
        )))
    }
}

/// One arXiv result (Atom feed entry).
#[derive(Debug, Clone)]
struct ArxivEntry {
    title: String,
    id: String,
    summary: String,
}

/// Parse an arXiv Atom feed into entries (title / id / summary). The feed
/// format is stable; string scanning is sufficient and dependency-free.
fn parse_arxiv_entries(xml: &str) -> Vec<ArxivEntry> {
    let mut out = Vec::new();
    for block in xml.split("<entry>").skip(1) {
        let entry = block.split("</entry>").next().unwrap_or("");
        let title = tag_content(entry, "title").unwrap_or_default();
        let id = tag_content(entry, "id").unwrap_or_default();
        let summary = tag_content(entry, "summary").unwrap_or_default();
        if title.is_empty() || id.is_empty() {
            continue;
        }
        out.push(ArxivEntry { title, id, summary });
    }
    out
}

/// Extract the text of one `<tag>…</tag>` element (first occurrence),
/// XML-unescaped.
fn tag_content(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(
        crate::tools::builtin::depwork::docx_edit::xml_unescape(&xml[s..e])
            .trim()
            .to_string(),
    )
}

/// Search arXiv's open-access API and optionally download the top PDF into
/// the workspace + save the citation into the 资料夹 — the 科研域
/// "全文抓取" gap, SSRF-safe (fixed arxiv.org domains).
pub struct ResearchOpenAccessTool;

impl ResearchOpenAccessTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ResearchOpenAccessTool {
    fn name(&self) -> &str {
        "research_open_access"
    }

    fn scope(&self) -> ToolScope {
        ToolScope::Depwork
    }

    fn description(&self) -> &str {
        "Search arXiv open-access papers by query. Returns the top results \
         (title, arXiv id, abstract). With download=true, fetches the top \
         paper's PDF into the workspace (path optional); with save=true \
         (default), saves the top citation into the 资料夹 (source=arxiv). \
         Parameters: query (required), download (bool), path (optional PDF \
         path), save (bool, default true), tags (optional)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query (title/keywords)."},
                "download": {"type": "boolean", "description": "Download the top PDF (default false)."},
                "path": {"type": "string", "description": "PDF output path (default arxiv_<id>.pdf)."},
                "save": {"type": "boolean", "description": "Save the top citation into the 资料夹 (default true)."},
                "tags": {"type": "string", "description": "Optional comma separated tags for the saved citation."}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| "Missing required parameter: query".to_string())?;
        let download = args
            .get("download")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let save = args.get("save").and_then(|v| v.as_bool()).unwrap_or(true);
        let tags = args
            .get("tags")
            .and_then(|v| v.as_str())
            .map(normalize_clip_tags)
            .unwrap_or_default();

        let feed_url =
            format!("http://export.arxiv.org/api/query?search_query=all:{query}&max_results=5");
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::Internal(format!("HTTP client error: {e}")))?;
        let feed = client
            .get(&feed_url)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("arXiv query failed: {e}")))?
            .text()
            .await
            .map_err(|e| AppError::Internal(format!("arXiv response read failed: {e}")))?;
        let entries = parse_arxiv_entries(&feed);
        if entries.is_empty() {
            return Ok(ToolResult::error(format!(
                "arXiv 未找到与「{query}」相关的结果。"
            )));
        }

        let mut out = format!(
            "arXiv 结果（{} 条，取前 {}）:\n",
            entries.len(),
            entries.len().min(5)
        );
        for (i, entry) in entries.iter().take(5).enumerate() {
            out.push_str(&format!(
                "\n[{i}] {}\n    {}\n    摘要: {}\n",
                entry.title,
                entry.id,
                snippet(&entry.summary)
            ));
        }

        let top = &entries[0];
        let pdf_url = top.id.replace("/abs/", "/pdf/");
        if download {
            let path_raw = args
                .get("path")
                .and_then(|p| p.as_str())
                .filter(|p| !p.trim().is_empty())
                .map(String::from)
                .unwrap_or_else(|| {
                    format!("arxiv_{}.pdf", top.id.rsplit('/').next().unwrap_or("paper"))
                });
            let mut path =
                crate::tools::builtin::resolve_path(context.workspace.as_deref(), &path_raw);
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AppError::Internal(format!("Failed to create output dir: {e}")))?;
            }
            if let Err(reason) = crate::hooks::ssrf::validate_fetch_url(&pdf_url) {
                return Err(AppError::Mcp(format!(
                    "SSRF guard rejected arXiv URL: {reason}"
                )));
            }
            let bytes = client
                .get(&pdf_url)
                .send()
                .await
                .map_err(|e| AppError::Internal(format!("PDF download failed: {e}")))?
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("PDF read failed: {e}")))?;
            tokio::fs::write(&path, bytes.as_ref())
                .await
                .map_err(|e| AppError::Internal(format!("Failed to write PDF: {e}")))?;
            super::permissions::record_output(context, &path);
            out.push_str(&format!("\n已下载 PDF：{}\n", path.display()));
        }

        if save {
            let state = context.app.state::<AppState>();
            let id = crate::storage::database::insert_research_item(
                &state.db,
                &context.session_id,
                &top.title,
                &pdf_url,
                "arxiv",
                &snippet(&top.summary),
                "",
                &tags,
            )?;
            out.push_str(&format!("\n已保存到资料夹 [#{id}]：{}\n", top.title));
        }

        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_tags_are_normalized() {
        assert_eq!(normalize_clip_tags("ml,  agents, web"), "ml,agents,web");
        assert_eq!(normalize_clip_tags("ml agents"), "ml,agents");
        assert_eq!(normalize_clip_tags("  ,  , "), "");
        assert_eq!(normalize_clip_tags("single"), "single");
    }

    #[test]
    fn report_markdown_is_assembled_from_items() {
        let item = crate::storage::database::ResearchItem {
            id: 7,
            session_id: "s1".into(),
            title: "Transformer survey".into(),
            url: "https://example.com/tr".into(),
            source: "scholar".into(),
            snippet: "attention is all you need".into(),
            snapshot: String::new(),
            tags: "ml,agents".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
        };
        let md = build_report_markdown(Some("本报告汇总如下。"), &[item]);
        assert!(md.contains("## 调研来源（1 条）"));
        assert!(md.contains("### 1. Transformer survey"));
        assert!(md.contains("https://example.com/tr"));
        assert!(md.contains("ml,agents"));
        assert!(md.contains("本报告汇总如下。"));
    }

    fn sample_item() -> crate::storage::database::ResearchItem {
        crate::storage::database::ResearchItem {
            id: 7,
            session_id: "s1".into(),
            title: "Transformer survey".into(),
            url: "https://doi.org/10.1000/xyz".into(),
            source: "scholar".into(),
            snippet: "attention is all you need".into(),
            snapshot: String::new(),
            tags: "ml,agents".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
        }
    }

    #[test]
    fn bibtex_export_includes_doi() {
        let out = export_bibtex(&[sample_item()]);
        assert!(out.contains("@misc{ddc7,"));
        assert!(out.contains("\\url{https://doi.org/10.1000/xyz}"));
        assert!(out.contains("doi = {10.1000/xyz}"));
    }

    #[test]
    fn gb7714_export_is_numbered() {
        let out = export_gb7714(&[sample_item()], "2026-08-08");
        assert!(out.contains("[1] Transformer survey[EB/OL]."));
        assert!(out.contains("访问日期 2026-08-08"));
        assert!(out.contains("https://doi.org/10.1000/xyz"));
    }

    #[test]
    fn apa_export_includes_retrieval_date() {
        let out = export_apa(&[sample_item()], "2026-08-08");
        assert!(out.contains(
            "Transformer survey. scholar, Retrieved 2026-08-08, from https://doi.org/10.1000/xyz"
        ));
    }

    #[test]
    fn arxiv_feed_parses_entries() {
        let xml = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
<entry><id>http://arxiv.org/abs/2301.00001v1</id><title>Attention Is All You Need</title>
<summary>We propose a new architecture &amp; more.</summary></entry>
<entry><id>http://arxiv.org/abs/2302.00002v2</id><title>Second Paper</title>
<summary>Another abstract.</summary></entry>
</feed>"#;
        let entries = parse_arxiv_entries(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Attention Is All You Need");
        assert!(entries[0].summary.contains("&"), "XML entities unescaped");
        assert_eq!(entries[1].id, "http://arxiv.org/abs/2302.00002v2");
    }
}
