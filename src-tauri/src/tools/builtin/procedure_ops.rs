//! Procedure tools — save and search learned workflows (procedural memory).

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::memory::procedure;
use crate::toolkit::ToolScope;
use async_trait::async_trait;
use serde_json::{json, Value};

fn collect_strings(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn render_procedures(procedures: &[procedure::Procedure], limit: usize) -> String {
    let mut out = String::new();
    for (i, p) in procedures.iter().take(limit).enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} — 触发：{}\n   步骤：{}\n",
            i + 1,
            p.mode,
            p.name,
            p.trigger,
            p.steps.join(" → ")
        ));
    }
    out
}

/// Save a verified workflow into procedural memory (`procedures.md`).
pub struct ProcedureSaveTool;

impl ProcedureSaveTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProcedureSaveTool {
    fn scope(&self) -> ToolScope {
        ToolScope::All
    }

    fn name(&self) -> &str {
        "procedure_save"
    }

    fn description(&self) -> &str {
        "Save a verified workflow into procedural memory — the reusable \
         step-by-step process distilled from a COMPLETED, VERIFIED task. \
         Procedures are injected into the system prompt on later similar \
         tasks (mode-filtered), so the agent reuses proven workflows \
         instead of re-discovering them. Use AFTER the task finished and \
         passed verification. `mode` defaults to the current work mode; \
         `all` makes it available in both. `scope` defaults to the current \
         project (project procedures), or 'user' for cross-project \
         workflows. Include concrete steps, what counts as verified, and \
         any non-obvious lessons. One procedure per workflow class."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short stable workflow name, e.g. 'wechat-article' or 'bug-fix'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["code", "depwork", "all"],
                    "description": "Which mode may reuse this workflow (default: current mode)."
                },
                "trigger": {
                    "type": "string",
                    "description": "When to reuse it — comma-separated task keywords, e.g. '公众号文章, 排版'."
                },
                "steps": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Ordered concrete steps of the workflow (3-10)."
                },
                "verify": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional checks that prove the workflow worked."
                },
                "lessons": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional non-obvious pitfalls or workarounds."
                },
                "scope": {
                    "type": "string",
                    "enum": ["project", "user"],
                    "description": "'project' (default) writes .deepdepcat/procedures.md; 'user' writes ~/.deepdepcat/procedures.md."
                }
            },
            "required": ["name", "trigger", "steps"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// File write — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let name = args
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| AppError::Parse("Missing required parameter: name".into()))?;
        let trigger = args
            .get("trigger")
            .and_then(|t| t.as_str())
            .ok_or_else(|| AppError::Parse("Missing required parameter: trigger".into()))?;
        let steps = collect_strings(&args, "steps");
        let verify = collect_strings(&args, "verify");
        let lessons = collect_strings(&args, "lessons");
        let mode = args
            .get("mode")
            .and_then(|m| m.as_str())
            .unwrap_or_else(|| context.work_mode.as_str())
            .to_string();
        let procedure = procedure::Procedure {
            name: name.to_string(),
            mode,
            trigger: trigger.to_string(),
            steps,
            verify,
            lessons,
        }
        .normalized();
        if procedure.steps.is_empty() {
            return Err(AppError::Parse(
                "steps must contain at least one step".into(),
            ));
        }
        let scope = args
            .get("scope")
            .and_then(|s| s.as_str())
            .unwrap_or("project");
        let path = match scope {
            "project" => {
                let ws = context.workspace.as_ref().ok_or_else(|| {
                    AppError::Other(
                        "procedure_save scope='project' needs a workspace — use scope='user' \
                         for workspace-independent procedures"
                            .to_string(),
                    )
                })?;
                procedure::project_procedures_path(ws)
            }
            "user" => procedure::user_procedures_path(),
            other => {
                return Err(AppError::Parse(format!(
                    "Invalid scope '{other}' — use 'project' or 'user'"
                )));
            }
        };
        procedure::save_procedure(&path, &procedure)?;
        Ok(ToolResult::success(format!(
            "Procedure '{}' saved ({}): {}\n触发：{}\n步骤：{}",
            procedure.name,
            scope,
            path.display(),
            procedure.trigger,
            procedure.steps.join(" → ")
        )))
    }
}

/// Search procedural memory for reusable workflows.
pub struct ProcedureSearchTool;

impl ProcedureSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ProcedureSearchTool {
    fn scope(&self) -> ToolScope {
        ToolScope::All
    }

    fn name(&self) -> &str {
        "procedure_search"
    }

    fn description(&self) -> &str {
        "Search procedural memory (procedures.md, user + project layers) \
         for learned workflows matching a task. Use at the START of a task \
         that resembles previously completed work to reuse the proven \
         workflow instead of inventing a new one. Matches name, trigger, \
         steps, verification and lessons."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Task keywords, e.g. '公众号' or 'xlsx 表格'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["code", "depwork", "all"],
                    "description": "Optional mode filter (default: all modes)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results. Defaults to 5."
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| AppError::Parse("Missing required parameter: query".into()))?;
        let mode = args.get("mode").and_then(|m| m.as_str());
        let limit = args
            .get("limit")
            .and_then(|l| l.as_u64())
            .unwrap_or(5) as usize;

        let mut all = procedure::read_procedures(&procedure::user_procedures_path());
        if let Some(ws) = context.workspace.as_deref() {
            all.extend(procedure::read_procedures(&procedure::project_procedures_path(ws)));
        }
        let hits: Vec<procedure::Procedure> = all
            .into_iter()
            .filter(|p| {
                p.matches_query(query)
                    && mode
                        .map(|m| p.applies_to(m))
                        .unwrap_or(true)
            })
            .collect();
        if hits.is_empty() {
            return Ok(ToolResult::success(format!(
                "No learned procedures match '{}'. This task may be new — \
                 save one with procedure_save after it passes verification.",
                query
            )));
        }
        Ok(ToolResult::success(format!(
            "Matched procedures for '{}':\n{}",
            query,
            render_procedures(&hits, limit)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_tool_shape_is_stable() {
        let tool = ProcedureSaveTool::new();
        assert_eq!(tool.name(), "procedure_save");
        assert!(!tool.is_read_only());
        assert!(!tool.is_concurrency_safe());
        assert_eq!(tool.scope(), ToolScope::All);
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("trigger")));
        assert!(required.contains(&json!("steps")));
    }

    #[test]
    fn search_tool_shape_is_stable() {
        let tool = ProcedureSearchTool::new();
        assert_eq!(tool.name(), "procedure_search");
        assert!(tool.is_read_only());
        let params = tool.parameters();
        assert!(params["required"][0] == "query");
    }

    #[test]
    fn save_rejects_missing_name() {
        let tool = ProcedureSaveTool::new();
        let err = tool
            .validate_args(&json!({ "trigger": "x", "steps": ["a"] }))
            .unwrap_err();
        assert!(err.contains("name"));
    }
}
