//! Tool registry — manages available tools and provides lookup.
//!
//! Uses interior mutability (`RwLock`) so tools can be registered at runtime
//! (e.g., MCP tools added after startup) without needing `&mut` access.
//!
//! `Clone` shares the underlying map (Arc), so every clone observes tools
//! registered after the clone was taken. Filtered clones
//! (`filtered_clone`/`read_only_clone`/`allowlist_clone`) build independent
//! maps on purpose — they are per-agent snapshots.

use crate::toolkit::{tool_to_definition, Tool};
use crate::core::types::ToolDefinition;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// The tool registry — holds all registered tools.
pub struct ToolRegistry {
    /// name → tool implementation.
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a tool at runtime. Re-registering a name replaces the
    /// previous implementation (backward-compatible overwrite semantics).
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        tracing::debug!("Registering tool: {}", name);
        let mut tools = self.tools.write().unwrap_or_else(|e| e.into_inner());
        tools.insert(name, tool);
    }

    /// Unregister every tool whose name starts with `prefix` (MCP server
    /// teardown: its tools are namespaced `server__tool`, so a prefix
    /// removal is exact). Returns how many tool ids were removed.
    pub fn unregister_prefix(&self, prefix: &str) -> usize {
        let mut tools = self.tools.write().unwrap_or_else(|e| e.into_inner());
        let before = tools.len();
        tools.retain(|name, _| !name.starts_with(prefix));
        before - tools.len()
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// Get all tool definitions (for sending to the model API).
    ///
    /// One definition per tool id. Sorted by name — the
    /// definitions are part of the prompt, and a stable order keeps the
    /// serialized prompt identical across turns, which is required for
    /// prompt caching to hit.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|t| t.is_enabled())
            .map(|t| tool_to_definition(t.as_ref()))
            .collect();
        defs.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        defs
    }

    /// Get the number of registered tool ids.
    pub fn len(&self) -> usize {
        self.tools.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Create a filtered clone of the registry containing only tools
    /// whose names pass the predicate.
    ///
    /// This is an independent snapshot — later registrations on the
    /// original registry are not visible through it.
    pub fn filtered_clone(&self, filter: impl Fn(&str) -> bool) -> ToolRegistry {
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner());
        let mut new_tools = HashMap::new();
        for (name, variants) in tools.iter() {
            if filter(name) {
                new_tools.insert(name.clone(), variants.clone());
            }
        }
        ToolRegistry {
            tools: Arc::new(RwLock::new(new_tools)),
        }
    }

    /// Create a clone containing only read-only tools.
    pub fn read_only_clone(&self) -> ToolRegistry {
        self.filtered_clone_by(|t| t.is_read_only())
    }

    /// Create a clone for an EVALUATOR subagent: read-only inspection tools
    /// PLUS bash (run tests/builds to verify claims) and the LSP tool (already
    /// read-only, kept for diagnostics). Deliberately EXCLUDES every
    /// mutating tool (write/edit/apply_patch/search_replace/file_operation)
    /// — an evaluator reviews and reports, it never changes the code. The
    /// agent tool (multi-agent spawner) is also excluded so an evaluator
    /// cannot delegate.
    pub fn evaluator_clone(&self) -> ToolRegistry {
        self.filtered_clone_by(|t| t.is_read_only() || t.name() == "bash" || t.name() == "lsp")
    }

    /// Create a clone containing only tools whose names are in the allowlist.
    pub fn allowlist_clone(&self, allowed: &[&str]) -> ToolRegistry {
        self.filtered_clone(|name| allowed.contains(&name))
    }

    /// Create a clone containing only tools available in the given work
    /// mode (per each tool's declared `scope`).
    ///
    /// Independent snapshot — later registrations on the original registry
    /// are not visible through it.
    pub fn for_mode(&self, mode: crate::toolkit::WorkMode) -> ToolRegistry {
        self.filtered_clone_by(|t| mode.allows(t.scope()))
    }

    /// Filtered clone by a predicate over the registered tool.
    fn filtered_clone_by(&self, pred: impl Fn(&Arc<dyn Tool>) -> bool) -> ToolRegistry {
        let tools = self.tools.read().unwrap_or_else(|e| e.into_inner());
        let mut new_tools = HashMap::new();
        for (name, tool) in tools.iter() {
            if pred(tool) {
                new_tools.insert(name.clone(), tool.clone());
            }
        }
        ToolRegistry {
            tools: Arc::new(RwLock::new(new_tools)),
        }
    }
}

impl Clone for ToolRegistry {
    /// Shares the underlying map — every clone observes tools registered
    /// after the clone was taken (e.g. MCP tools registered at runtime).
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolkit::{Tool, ToolContext, ToolResult};
    use crate::core::error::AppResult;
    use crate::bootstrap::AppState;
    use crate::toolkit::WorkMode;
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct DummyTool {
        name: &'static str,
        read_only: bool,
    }

    /// Serialized size of one mode's full tool schema set — guards against
    /// silent tool-bloat regressions. Every tool schema is part of the
    /// stable request prefix, so it competes with the conversation for the
    /// context window and the first-request cache build.
    #[tokio::test]
    async fn tool_schema_budget_per_mode() {
        // `DEEPDEPCAT_DATA_DIR` is process-global — serialize with the
        // permissions integration tests to avoid two concurrent
        // `AppState::initialize` calls on the same DB.
        let _guard = crate::permissions::DATA_DIR_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ddc-tool-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DEEPDEPCAT_DATA_DIR", &dir);
        let state = AppState::initialize(None).await.expect("AppState init");
        for mode in [WorkMode::Code, WorkMode::Depwork] {
            let registry = state.tools.for_mode(mode);
            let defs = registry.definitions();
            let chars: usize = defs
                .iter()
                .map(|d| serde_json::to_string(d).map(|s| s.len()).unwrap_or(0))
                .sum();
            // Budgets measured 2026-08-10 (code 33.9k / depwork 67k) with
            // ~2x headroom: crossing them means real schema bloat, not noise.
            let budget = match mode {
                WorkMode::Code => 60_000,
                WorkMode::Depwork => 100_000,
            };
            eprintln!(
                "[tool-schema] {}: {} tools, {chars} chars (~{} tokens)",
                mode.as_str(),
                defs.len(),
                chars / 4
            );
            assert!(
                chars < budget,
                "{} tool schema grew past {budget} chars ({chars}); \
                 consider ToolSearch-style deferred loading or trimming descriptions"
                ,
                mode.as_str()
            );
        }
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "dummy"
        }
        fn parameters(&self) -> Value {
            json!({"type": "object", "properties": {}})
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
        async fn execute(&self, _: Value, _: &ToolContext) -> AppResult<ToolResult> {
            Ok(ToolResult::success("ok"))
        }
    }

    fn dummy(name: &'static str) -> Arc<DummyTool> {
        Arc::new(DummyTool {
            name,
            read_only: false,
        })
    }

    fn dummy_ro(name: &'static str) -> Arc<DummyTool> {
        Arc::new(DummyTool {
            name,
            read_only: true,
        })
    }

    #[test]
    fn clone_shares_registrations() {
        let registry = ToolRegistry::new();
        let shared = registry.clone();
        registry.register(Arc::new(DummyTool {
            name: "late",
            read_only: false,
        }));
        assert!(
            shared.get("late").is_some(),
            "clones must observe tools registered after cloning"
        );
    }

    #[test]
    fn evaluator_clone_keeps_read_only_and_bash_only() {
        let registry = ToolRegistry::new();
        registry.register(dummy_ro("read_file"));
        registry.register(dummy_ro("grep"));
        registry.register(dummy_ro("lsp"));
        registry.register(dummy("bash"));
        registry.register(dummy("write_file"));
        registry.register(dummy("edit_file"));
        registry.register(dummy("agent"));
        let evaluator = registry.evaluator_clone();
        let names: Vec<String> = evaluator
            .definitions()
            .iter()
            .map(|d| d.function.name.clone())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"grep".to_string()));
        assert!(names.contains(&"lsp".to_string()));
        assert!(
            names.contains(&"bash".to_string()),
            "evaluator may run tests via bash"
        );
        assert!(
            !names.contains(&"write_file".to_string()),
            "evaluator must never mutate files"
        );
        assert!(
            !names.contains(&"edit_file".to_string()),
            "evaluator must never edit files"
        );
        assert!(
            !names.contains(&"agent".to_string()),
            "evaluator must not spawn subagents"
        );
    }

    #[test]
    fn for_mode_depwork_excludes_code_only_tools() {
        // The meta/shell/debug tools must not leak into Depwork: use_tool can
        // wrap ANY tool (bypassing the mode boundary), scheduler executes
        // shell commands, monitor is a coding debug aid. Depwork's office
        // toolset keeps read/write/search/web research (its designed
        // surface per DEPWORK_MODE_PROMPT).
        struct ScopedDummy {
            name: &'static str,
            scope: crate::toolkit::ToolScope,
        }
        #[async_trait]
        impl Tool for ScopedDummy {
            fn name(&self) -> &str {
                self.name
            }
            fn description(&self) -> &str {
                "scoped dummy"
            }
            fn parameters(&self) -> Value {
                json!({"type": "object", "properties": {}})
            }
            fn scope(&self) -> crate::toolkit::ToolScope {
                self.scope
            }
            async fn execute(&self, _: Value, _: &ToolContext) -> AppResult<ToolResult> {
                Ok(ToolResult::success("ok"))
            }
        }

        let registry = ToolRegistry::new();
        let all = crate::toolkit::ToolScope::All;
        let code = crate::toolkit::ToolScope::Code;
        for (name, scope) in [
            ("read_file", all),
            ("write_file", all),
            ("grep", all),
            ("web_fetch", all),
            ("todo_write", all),
            ("visual_describe", all),
            ("docx_generate", all),
            ("use_tool", code),
            ("scheduler_create", code),
            ("scheduler_list", code),
            ("scheduler_delete", code),
            ("monitor", code),
            ("bash", code),
        ] {
            registry.register(Arc::new(ScopedDummy { name, scope }));
        }

        let depwork_names: Vec<String> = registry
            .for_mode(crate::toolkit::WorkMode::Depwork)
            .definitions()
            .iter()
            .map(|d| d.function.name.clone())
            .collect();
        for allowed in [
            "read_file",
            "write_file",
            "grep",
            "web_fetch",
            "todo_write",
            "visual_describe",
            "docx_generate",
        ] {
            assert!(
                depwork_names.contains(&allowed.to_string()),
                "Depwork must keep {allowed}"
            );
        }
        for excluded in [
            "use_tool",
            "scheduler_create",
            "scheduler_list",
            "scheduler_delete",
            "monitor",
            "bash",
        ] {
            assert!(
                !depwork_names.contains(&excluded.to_string()),
                "Depwork must NOT see {excluded}"
            );
        }

        // Code mode keeps everything.
        let code_names: Vec<String> = registry
            .for_mode(crate::toolkit::WorkMode::Code)
            .definitions()
            .iter()
            .map(|d| d.function.name.clone())
            .collect();
        assert!(code_names.contains(&"monitor".to_string()));
        assert!(code_names.contains(&"use_tool".to_string()));
        assert!(code_names.contains(&"scheduler_create".to_string()));
    }

    #[test]
    fn definitions_sorted_by_name() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool {
            name: "zeta",
            read_only: false,
        }));
        registry.register(Arc::new(DummyTool {
            name: "alpha",
            read_only: false,
        }));
        registry.register(Arc::new(DummyTool {
            name: "mike",
            read_only: false,
        }));
        let names: Vec<String> = registry
            .definitions()
            .iter()
            .map(|d| d.function.name.clone())
            .collect();
        assert_eq!(names, vec!["alpha", "mike", "zeta"]);
    }

    #[test]
    fn tool_definitions_are_byte_stable_for_prefix_cache() {
        // The provider prompt-cache prefix includes the tool schema: two
        // consecutive definition snapshots must serialize byte-identically
        // (sorted order, same fields) or every request silently misses.
        let registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool {
            name: "zeta",
            read_only: false,
        }));
        registry.register(Arc::new(DummyTool {
            name: "alpha",
            read_only: false,
        }));
        let first = format!("{:?}", registry.definitions());
        let second = format!("{:?}", registry.definitions());
        assert_eq!(
            first, second,
            "tool definitions must be deterministic for the cache prefix"
        );
    }

    #[test]
    fn unregister_prefix_removes_only_matching_tools() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool {
            name: "srv__list",
            read_only: false,
        }));
        registry.register(Arc::new(DummyTool {
            name: "srv__call",
            read_only: false,
        }));
        registry.register(Arc::new(DummyTool {
            name: "other__list",
            read_only: false,
        }));
        registry.register(Arc::new(DummyTool {
            name: "builtin",
            read_only: false,
        }));

        assert_eq!(registry.unregister_prefix("srv__"), 2);
        assert!(registry.get("srv__list").is_none());
        assert!(registry.get("srv__call").is_none());
        assert!(
            registry.get("other__list").is_some(),
            "other server untouched"
        );
        assert!(registry.get("builtin").is_some(), "builtin untouched");
    }
}
