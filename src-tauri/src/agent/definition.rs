//! Agent definition files — Markdown + YAML frontmatter format for
//! user-customizable agent personas.
//!
//! ## File Layout
//!
//! ```markdown
//! ---
//! name: Code Reviewer
//! description: Reviews code for quality and suggests improvements
//! prompt_mode: extend
//! model: deepseek-v4-flash
//! allowed_tools:
//!   - read_file
//!   - grep
//!   - glob
//! permissions:
//!   allow:
//!     - Read(**)
//!     - Bash(git *)
//!   deny:
//!     - Bash(rm *)
//! ---
//!
//! You are a code reviewer. Analyze code for:
//! - Correctness
//! - Performance
//! - Security
//! - Readability
//! ```
//!
//! ## Discovery Rules
//!
//! 1. Built-in agents (defined in code)
//! 2. `~/.deepdepcat/agents/*.md` — user-level
//! 3. `.deepdepcat/agents/*.md` — project-level (highest priority)
//!
//! Project-level definitions override user-level with the same `name`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// How the agent definition's body interacts with the default system prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    /// Append the body to the default system prompt.
    #[default]
    Extend,
    /// Replace the default system prompt entirely.
    Full,
}

/// Per-agent permission rules, using the same `Tool(pattern)` syntax as the
/// settings allow/deny/ask lists. Agent denies (including denies inherited
/// from a parent agent) are a hard veto; allows/asks refine what the agent
/// may do without prompting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentPermissions {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
}

/// Agent definition parsed from a `.md` file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Unique identifier (derived from file name or `name` field).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Short description shown in the UI.
    #[serde(default)]
    pub description: String,
    /// The system prompt body (Markdown content after frontmatter).
    #[serde(skip)]
    pub body: String,
    /// How the body interacts with the default system prompt.
    #[serde(default)]
    pub prompt_mode: PromptMode,
    /// Optional model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub model: Option<String>,
    /// Optional restricted tool list. When non-empty, only these tools
    /// are available to the agent.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Work modes this agent belongs to (`code` / `depwork`). Empty = all
    /// modes. Filtered at listing time so Depwork never surfaces coding
    /// agents and vice versa.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work_modes: Vec<String>,
    /// Per-agent permission rules (`Tool(pattern)` syntax). Empty = the
    /// agent inherits the normal global/project permission layers.
    #[serde(default)]
    pub permissions: AgentPermissions,
}

/// YAML frontmatter parsed from a Markdown file.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    prompt_mode: PromptMode,
    model: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    work_modes: Vec<String>,
    #[serde(default)]
    permissions: AgentPermissions,
}

/// Discover all agent definitions from built-in, user, and project sources.
///
/// Discovery order (later sources override earlier ones with the same `name`):
/// 1. Built-in agents
/// 2. `~/.deepdepcat/agents/*.md`
/// 3. `<workspace>/.deepdepcat/agents/*.md`
pub fn discover_all(workspace: Option<&Path>) -> Vec<AgentDefinition> {
    let mut defs: HashMap<String, AgentDefinition> = HashMap::new();

    // 1. Built-in agents
    for def in builtin_agents() {
        defs.insert(def.name.clone(), def);
    }

    // 2. User-level agents
    if let Some(home) = dirs::home_dir() {
        let user_agents_dir = home.join(".deepdepcat").join("agents");
        for def in discover_dir(&user_agents_dir) {
            defs.insert(def.name.clone(), def);
        }
    }

    // 3. Project-level agents (highest priority)
    if let Some(ws) = workspace {
        let project_agents_dir = ws.join(".deepdepcat").join("agents");
        for def in discover_dir(&project_agents_dir) {
            defs.insert(def.name.clone(), def);
        }
    }

    let mut result: Vec<AgentDefinition> = defs.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Discover agent definitions from a specific directory.
fn discover_dir(dir: &Path) -> Vec<AgentDefinition> {
    let mut defs = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return defs,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        match parse_agent_file(&path) {
            Ok(def) => {
                debug!(name = %def.name, path = %path.display(), "Discovered agent");
                defs.push(def);
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to parse agent file");
            }
        }
    }

    defs
}

/// Parse an agent definition from a Markdown file with YAML frontmatter.
pub fn parse_agent_file(path: &Path) -> Result<AgentDefinition, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    parse_agent_content(&content, Some(path.to_path_buf()))
}

/// Parse agent definition from raw content.
fn parse_agent_content(
    content: &str,
    source_path: Option<PathBuf>,
) -> Result<AgentDefinition, String> {
    let (frontmatter, body) = split_frontmatter(content);

    let fm: Frontmatter = if frontmatter.is_empty() {
        Frontmatter {
            name: None,
            description: None,
            prompt_mode: PromptMode::Extend,
            model: None,
            allowed_tools: vec![],
            work_modes: vec![],
            permissions: AgentPermissions::default(),
        }
    } else {
        serde_yaml::from_str(&frontmatter)
            .map_err(|e| format!("Failed to parse frontmatter: {}", e))?
    };

    let name = fm.name.unwrap_or_else(|| {
        // Derive from file name
        source_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    let id = name.to_lowercase().replace(' ', "-");

    Ok(AgentDefinition {
        id,
        name,
        description: fm.description.unwrap_or_default(),
        body: body.trim().to_string(),
        prompt_mode: fm.prompt_mode,
        model: fm.model,
        allowed_tools: fm.allowed_tools,
        work_modes: fm.work_modes,
        permissions: fm.permissions,
    })
}

/// Filter agent definitions by work mode.
///
/// A definition matches when `work_modes` is empty (all modes) or contains
/// the mode's wire name (`code` / `depwork`).
pub fn filter_by_work_mode(
    defs: Vec<AgentDefinition>,
    mode: crate::toolkit::WorkMode,
) -> Vec<AgentDefinition> {
    defs.into_iter()
        .filter(|d| d.work_modes.is_empty() || d.work_modes.iter().any(|m| m == mode.as_str()))
        .collect()
}

/// A custom agent resolved for use as the MAIN session persona (Code or
/// Depwork). Carries everything the harness needs: the body overlaid on the
/// mode prompt, the tool allowlist, and the permission rules (M9).
#[derive(Debug, Clone)]
pub struct ResolvedCustomAgent {
    pub name: String,
    pub body: String,
    pub allowed_tools: Vec<String>,
    pub permissions: AgentPermissions,
}

/// Resolve a custom agent for a MAIN session by id or name, gated by the
/// session's work mode. "Default" and unknown names resolve to `None`
/// (the standard persona is used).
pub fn resolve_for_main(
    workspace: Option<&Path>,
    work_mode: crate::toolkit::WorkMode,
    name: &str,
) -> Option<ResolvedCustomAgent> {
    if name.trim().is_empty() || name.eq_ignore_ascii_case("default") {
        return None;
    }
    let def = discover_all(workspace)
        .into_iter()
        .find(|d| d.id == name || d.name == name)?;
    if !def.work_modes.is_empty() && !def.work_modes.iter().any(|m| m == work_mode.as_str()) {
        return None;
    }
    Some(ResolvedCustomAgent {
        name: def.name,
        body: def.body,
        allowed_tools: def.allowed_tools,
        permissions: def.permissions,
    })
}

/// Split a Markdown file into YAML frontmatter and body content.
///
/// Frontmatter is delimited by `---` at the start and end:
/// ```text
/// ---
/// name: My Agent
/// ---
/// Body content here.
/// ```
fn split_frontmatter(content: &str) -> (String, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (String::new(), content.to_string());
    }

    // Find the closing `---`
    let after_open = &trimmed[3..];
    let close_pos = match after_open.find("\n---") {
        Some(pos) => pos,
        None => return (String::new(), content.to_string()),
    };

    let frontmatter = after_open[..close_pos].trim().to_string();
    let body_start = close_pos + 4; // Skip past "\n---"
    let body = after_open[body_start..]
        .trim_start_matches('\n')
        .to_string();

    (frontmatter, body)
}

/// Built-in agent definitions shipped with the app.
fn builtin_agents() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            id: "default".into(),
            name: "默认".into(),
            description: "通用编程助手，拥有完整工具集".into(),
            body: String::new(),
            prompt_mode: PromptMode::Extend,
            model: None,
            allowed_tools: vec![],
            work_modes: vec!["code".into()],
            permissions: AgentPermissions::default(),
        },
        AgentDefinition {
            id: "code-reviewer".into(),
            name: "代码评审".into(),
            description: "只读评审：检查代码质量、安全与性能，给出可执行的修改建议".into(),
            body: "You are a code reviewer. Before completing, you must read the \
                relevant files using read_file. Analyze code for:\n\
                - Correctness and potential bugs\n\
                - Performance bottlenecks\n\
                - Security vulnerabilities\n\
                - Readability and maintainability\n\
                Provide specific, actionable suggestions with file paths and line numbers."
                .into(),
            prompt_mode: PromptMode::Extend,
            model: None,
            allowed_tools: vec!["read_file".into(), "grep".into(), "glob".into()],
            work_modes: vec!["code".into()],
            permissions: AgentPermissions::default(),
        },
        AgentDefinition {
            id: "doc-writer".into(),
            name: "文档撰写".into(),
            description: "撰写报告、纪要、提案等文档，格式规范、内容完整".into(),
            body: "You are a professional document writer. Produce well-structured \
                reports, meeting minutes, and proposals. When the user wants to \
                see the writing happen live, stream the deliverable into the \
                open WPS/Word window with live_doc_write (or office_automate \
                type_text for short additions). Use docx_generate or \
                ppt_generate when a finished FILE is requested. \
                Prioritize: clear structure, consistent style, and complete content.\n\
                \n\
                Writing quality (human feel, not template): \
                write in natural, varied sentences — mix short and long, avoid \
                starting every paragraph with 首先/其次/最后, open without formulaic \
                filler. Support claims with concrete numbers, examples or specifics \
                instead of vague statements. Keep a professional tone with warmth, \
                but never invent data or fabricate grammar errors."
                .into(),
            prompt_mode: PromptMode::Extend,
            model: None,
            allowed_tools: vec![
                "doc_read".into(),
                "docx_generate".into(),
                "ppt_generate".into(),
                "table_process".into(),
                "live_doc_write".into(),
                "office_automate".into(),
                "content_pack".into(),
                "citation_link".into(),
                "doc_consistency".into(),
                "todo_write".into(),
            ],
            work_modes: vec!["depwork".into()],
            permissions: AgentPermissions::default(),
        },
        AgentDefinition {
            id: "market-manager".into(),
            name: "市场经理".into(),
            description: "店铺运营调研与方案——高德定位→高德周边可视化→小红书口碑→竞对对照→输出运营方案".into(),
            body: "你是资深本地生活市场经理。接到「调研某店铺/做运营方案」任务时，\
                严格按管线执行，每步用真实数据说话，不编造：\n\
                \n\
                1. 需求澄清：确认店铺名、城市、品类、目标（评分提升/团购起量/新店冷启动等）。\
                信息不足用 ask_user 问清，别猜。\n\
                2. 定位与周边数据（优先 store_research_geo，纯 HTTP 稳定）：store_research_geo(store, city) \
                拿店铺地址/电话/坐标，并查周边竞对与配套（同品类密度/商圈构成）——量化数据\
                主要靠它。未配置高德 key 时提示用户配置。\n\
                3. 高德周边可视化（看「这家店周围到底什么样」）：store_research_map(store, city) \
                在开发浏览器打开高德搜索店铺，确认位置并拿 POI 卡片（评分/人均/营业时间/品类标签）；\
                随后用 browser_control 在地图页缩放/拖动，把店铺周边（约 500m-2km）截图，\
                visual_describe 看图分析——同品类密度、竞对点位分布、商圈氛围、交通与配套。\
                被验证码/登录拦截时 browser_control handoff 请用户处理一次；\
                拿不到的量化数据标「数据缺失」，绝不编造。\n\
                4. 小红书口碑：store_research_xhs(城市+品类) 看种草声量水位、可复用选题、负面笔记风险；\
                再搜一次店名看直接口碑。同样被风控就降级：handoff 或放弃。\n\
                5. 竞对对照：从 geo 周边同品类 POI 或高德周边截图挑 2-3 家头部竞对，\
                对比评分/人均、门店规模、选址差异，找差异点。\n\
                6. 输出《店铺运营方案》：\n\
                   - 现状诊断：线上口碑健康度（评分/点评数 vs 竞对）、价格带定位、种草声量水位、\
                   周边竞争密度\n\
                   - 机会点：口碑短板、内容空白、套餐/定价差距\n\
                   - 行动项（按优先级）：评分提升（服务短板整改+评价回复话术）、套餐梯度设计、\
                   新客引流（首单/优惠）、评价管理机制、内容种草（小红书选题+本地达人合作）、\
                   搜索排名优化（类目词、流量词）\n\
                   - 每项给落地动作+预期效果\n\
                7. 落盘：默认用 docx_generate 生成《{店铺名}运营方案.docx》到会话产物，\
                同时在对话里输出方案摘要（诊断/机会/行动项概览）。用户只要文本则只在对话输出。\n\
                \n\
                规则：所有数据必须来自调研结果，拿不到的标「数据缺失」绝不编造；网页要登录/验证码时\
                用 browser_control handoff 请用户处理；调研截图可用 visual_describe 看图辅助判断；\
                只做调研与方案，不执行任何付款/下单类操作。\n\
                \n\
                语气：像一位沉稳的本地生活运营顾问——交付《运营方案》先给结论与行动项，再解释依据；\
                数字说话，不写空泛套话；可主动提示数据缺口或机会点，但方案取舍留给用户决定。"
                .into(),
            prompt_mode: PromptMode::Extend,
            model: None,
            allowed_tools: vec![
                "store_research_geo".into(),
                "store_research_map".into(),
                "store_research_xhs".into(),
                "browser_control".into(),
                "web_fetch_depwork".into(),
                "visual_describe".into(),
                "docx_generate".into(),
                "ask_user".into(),
                "todo_write".into(),
            ],
            work_modes: vec!["depwork".into()],
            permissions: AgentPermissions::default(),
        },
        AgentDefinition {
            id: "ppt-expert".into(),
            name: "PPT 专家".into(),
            description: "汇报/教学/路演的 PPT 策划与制作：需求收集→大纲确认→逐页要点→pptx 成品".into(),
            body: "你是资深 PPT 策划与撰稿专家。接到「做 PPT」任务时按管线执行，\
                每步先与用户确认再推进：\n\
                \n\
                1. 需求收集：确认主题、受众、目标（汇报/教学/路演等）、风格偏好、时长与页数要求。\
                信息不足用 ask_user 问清，别猜。\n\
                2. 出大纲：按时长定页数（约 1-2 页/分钟，10 分钟 ≈ 10-20 页），\
                先给用户「页数 + 每页要点」并确认，确认后再细化。\n\
                3. 定架构：按 开场(intro)→主体(body)→收尾(end) 组织；\
                主体内容可用「情境→冲突→问题→方案」式推进，让每页只有一个重点。\n\
                4. 逐页写要点：每页「标题 + 3-5 条要点」，大段文字优先拆成列表/表格/图片；\
                涉及数据必须有来源，拿不到标「数据缺失」。\n\
                5. 生成文件：大纲确认后用 ppt_generate 一次性渲染成 .pptx 并返回路径；\
                用户要改就改大纲重出，不要重复整套流程。\n\
                6. 交付：返回文件路径 + 每页要点摘要；用户只要文本大纲则只在对话输出。\n\
                \n\
                写作质量（真人口感，不是模板腔）：标题写观点不写名词堆砌（「Q3 营收增长 15%」\
                优于「Q3 营收情况」）；要点带具体数字、例子或对比，不写空泛套话；\
                语言自然有节奏，但不编造数据、不故意制造语病。\n\
                \n\
                规则：图片路径必须真实存在；页数服从时长；不编造数据。"
                .into(),
            prompt_mode: PromptMode::Extend,
            model: None,
            allowed_tools: vec![
                "ppt_generate".into(),
                "doc_read".into(),
                "ask_user".into(),
                "todo_write".into(),
            ],
            work_modes: vec!["depwork".into()],
            permissions: AgentPermissions::default(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_and_body() {
        let content = "---\nname: Test Agent\ndescription: A test\nprompt_mode: full\n---\n\nBody content here.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("name: Test Agent"));
        assert_eq!(body.trim(), "Body content here.");
    }

    #[test]
    fn parse_no_frontmatter() {
        let content = "Just body content.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.is_empty());
        assert_eq!(body, "Just body content.");
    }

    #[test]
    fn parse_agent_with_allowed_tools() {
        let content = "---\nname: Restricted\nallowed_tools:\n  - read_file\n  - grep\n---\nBody";
        let def = parse_agent_content(content, None).unwrap();
        assert_eq!(def.name, "Restricted");
        assert_eq!(def.allowed_tools, vec!["read_file", "grep"]);
        assert_eq!(def.body, "Body");
    }

    #[test]
    fn parse_agent_with_permissions() {
        let content = "---\nname: Guarded\npermissions:\n  allow:\n    - Read(**)\n  deny:\n    - Bash(rm *)\n  ask:\n    - WebFetch(*)\n---\nBody";
        let def = parse_agent_content(content, None).unwrap();
        assert_eq!(def.permissions.allow, vec!["Read(**)"]);
        assert_eq!(def.permissions.deny, vec!["Bash(rm *)"]);
        assert_eq!(def.permissions.ask, vec!["WebFetch(*)"]);
        assert_eq!(def.body, "Body");
    }

    #[test]
    fn agent_without_permissions_defaults_empty() {
        let content = "---\nname: Plain\n---\nBody";
        let def = parse_agent_content(content, None).unwrap();
        assert!(def.permissions.allow.is_empty());
        assert!(def.permissions.deny.is_empty());
        assert!(def.permissions.ask.is_empty());
    }

    #[test]
    fn builtin_agents_exist() {
        let agents = builtin_agents();
        assert!(agents.iter().any(|a| a.id == "default"));
        assert!(agents.iter().any(|a| a.id == "code-reviewer"));
        assert!(agents.iter().any(|a| a.id == "ppt-expert"));
    }

    #[test]
    fn ppt_expert_is_depwork_only_with_generation_tools() {
        let agents = builtin_agents();
        let ppt = agents
            .iter()
            .find(|a| a.id == "ppt-expert")
            .expect("ppt-expert built-in agent");
        assert_eq!(ppt.work_modes, vec!["depwork"]);
        assert!(ppt.allowed_tools.contains(&"ppt_generate".to_string()));
        assert!(ppt.allowed_tools.contains(&"doc_read".to_string()));
        assert!(ppt.allowed_tools.contains(&"ask_user".to_string()));
        assert!(!ppt.allowed_tools.contains(&"browser_control".to_string()));
        assert!(!ppt.body.is_empty());
        assert!(ppt.body.contains("ask_user"));
        assert!(ppt.body.contains("ppt_generate"));
    }

    #[test]
    fn market_manager_is_depwork_only_with_research_tools() {
        let agents = builtin_agents();
        let mm = agents
            .iter()
            .find(|a| a.id == "market-manager")
            .expect("market-manager built-in agent");
        assert_eq!(mm.work_modes, vec!["depwork"]);
        assert!(mm.allowed_tools.contains(&"store_research_geo".to_string()));
        assert!(mm.allowed_tools.contains(&"store_research_map".to_string()));
        assert!(mm.allowed_tools.contains(&"store_research_xhs".to_string()));
        assert!(mm.allowed_tools.contains(&"browser_control".to_string()));
        assert!(mm.allowed_tools.contains(&"visual_describe".to_string()));
        assert!(mm.allowed_tools.contains(&"docx_generate".to_string()));
        assert!(mm.allowed_tools.contains(&"ask_user".to_string()));
        assert!(!mm
            .allowed_tools
            .contains(&"store_research_meituan".to_string()));
        assert!(!mm.body.is_empty());
        assert!(mm.body.contains("store_research_geo"));
        assert!(mm.body.contains("绝不编造"));
    }

    #[test]
    fn market_manager_survives_work_mode_filter() {
        let depwork: Vec<_> =
            filter_by_work_mode(builtin_agents(), crate::toolkit::WorkMode::Depwork)
                .into_iter()
                .collect();
        assert!(depwork.iter().any(|a| a.id == "market-manager"));
        let code: Vec<_> =
            filter_by_work_mode(builtin_agents(), crate::toolkit::WorkMode::Code)
                .into_iter()
                .collect();
        assert!(!code.iter().any(|a| a.id == "market-manager"));
    }

    #[test]
    fn resolve_for_main_finds_code_agents() {
        let resolved = resolve_for_main(None, crate::toolkit::WorkMode::Code, "代码评审");
        let agent = resolved.expect("code reviewer is a code-mode agent");
        assert_eq!(agent.name, "代码评审");
        assert!(!agent.body.is_empty());
        assert!(agent.allowed_tools.contains(&"read_file".to_string()));
    }

    #[test]
    fn resolve_for_main_rejects_wrong_mode_and_unknown() {
        assert!(
            resolve_for_main(None, crate::toolkit::WorkMode::Code, "市场经理").is_none(),
            "depwork-only agent must not run as a code main persona"
        );
        assert!(
            resolve_for_main(None, crate::toolkit::WorkMode::Code, "ghost").is_none(),
            "unknown agent resolves to the default persona"
        );
        assert!(
            resolve_for_main(None, crate::toolkit::WorkMode::Code, "default").is_none(),
            "default persona is the standard loop"
        );
        assert!(resolve_for_main(None, crate::toolkit::WorkMode::Code, "").is_none());
    }
}
