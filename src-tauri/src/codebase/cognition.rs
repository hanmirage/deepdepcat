//! Project cognition snapshot — module-level aggregation of the codebase
//! index, injected into the agent's context so it starts with a "project
//! map" instead of discovering the layout by trial exploration. Long-task
//! planning (cross-module edits, architecture decisions) needs this map.
//!
//! Pure function over the EXISTING index (`DependencyGraph` + `SymbolIndex`)
//! — no new parsing. Modules are files grouped by their top-level directory;
//! core modules are the most-referenced; entries are detected by project type.

use crate::codebase::dependency::DependencyGraph;
use crate::codebase::symbols::{SymbolIndex, SymbolKind};
use crate::core::types::ProjectType;
use std::collections::BTreeMap;
use std::path::Path;

/// Cap on symbols rendered per module — keep the snapshot compact.
const MAX_SYMBOLS_PER_MODULE: usize = 8;
/// Cap on modules rendered — a huge monorepo must not flood the context.
const MAX_MODULES: usize = 40;
/// Cap on modules reported as core.
const MAX_CORE_MODULES: usize = 3;

/// One module in the snapshot — files grouped by their top-level directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSummary {
    pub name: String,
    pub file_count: usize,
    /// Other modules this module imports.
    pub depends_on: Vec<String>,
    /// Modules that import this module.
    pub depended_by: Vec<String>,
    /// Main symbol names defined in this module (capped).
    pub symbols: Vec<String>,
}

/// The deterministic project-cognition snapshot (module graph + entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCognition {
    pub modules: Vec<ModuleSummary>,
    /// Most-referenced modules (most files depend on them), descending.
    pub core_modules: Vec<String>,
    /// Detected entry points (main.rs / index.ts / app.py ...).
    pub entries: Vec<String>,
}

/// Group a relative source path into its module ("" = root file).
///
/// Granularity is the first TWO path components so a Rust `src/` layout
/// yields distinct modules for `src/core`, `src/ui`, etc. instead of
/// collapsing the whole `src/` tree into one blob:
/// - `src/main.rs` → `src`
/// - `src/core/mod.rs` → `src/core`
/// - `packages/web/index.ts` → `packages/web`
/// - `README.md` → `""`
pub fn module_of(path: &Path) -> String {
    let parts: Vec<_> = path.components().collect();
    match parts.len() {
        0 | 1 => String::new(),
        2 => parts[0].as_os_str().to_string_lossy().to_string(),
        _ => format!(
            "{}/{}",
            parts[0].as_os_str().to_string_lossy(),
            parts[1].as_os_str().to_string_lossy()
        ),
    }
}

/// Entry-file basenames per project type.
fn entry_candidates(project_type: &ProjectType) -> &'static [&'static str] {
    match project_type {
        ProjectType::Rust => &["main.rs", "lib.rs"],
        ProjectType::NodeNpm | ProjectType::NodePnpm | ProjectType::NodeBun => {
            &["index.ts", "main.ts", "index.js", "main.js", "app.tsx"]
        }
        ProjectType::PythonPoetry | ProjectType::PythonPip | ProjectType::PythonUv => {
            &["main.py", "app.py", "manage.py"]
        }
        ProjectType::Go => &["main.go"],
        ProjectType::JavaMaven | ProjectType::JavaGradle => &["Application.java", "Main.java"],
        ProjectType::Cmake => &["main.cpp", "main.c"],
        ProjectType::Dotnet => &["Program.cs"],
        ProjectType::Monorepo | ProjectType::Unknown => &[],
    }
}

/// Symbols worth surfacing in a module overview.
fn is_notable(kind: &SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Struct
            | SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Trait
            | SymbolKind::Enum
            | SymbolKind::Const
            | SymbolKind::Method
    )
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// Strip the workspace root off an absolute path, falling back to the path
/// as-is when it is already relative.
fn rel_to_root<'a>(root: &Path, p: &'a Path) -> &'a Path {
    p.strip_prefix(root).unwrap_or(p)
}

/// Build the deterministic project-cognition snapshot from the codebase
/// index. Pure function over the existing graph + symbol index.
pub fn build_cognition(
    graph: &DependencyGraph,
    symbols: &SymbolIndex,
    project_type: &ProjectType,
) -> ProjectCognition {
    let all_nodes: Vec<_> = graph.files().collect();
    // Graph node paths are absolute — module grouping needs them relative
    // to the workspace root (`src/ui/mod.rs`, not `C:/.../src/ui/mod.rs`).
    let root = graph.root();

    // Group files by module.
    let mut files_by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in &all_nodes {
        let m = module_of(rel_to_root(root, &node.path));
        files_by_module
            .entry(m)
            .or_default()
            .push(node.path.to_string_lossy().to_string());
    }

    // Cross-module dependency edges + per-module dependents count (core).
    let mut depends_by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut depended_by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut dependents_count: BTreeMap<String, usize> = BTreeMap::new();
    for (module, files) in &files_by_module {
        for file in files {
            let Some(node) = all_nodes.iter().find(|n| n.path.to_string_lossy() == *file) else {
                continue;
            };
            for dep in &node.dependencies {
                let dep_module = module_of(rel_to_root(root, dep));
                if !dep_module.is_empty() && dep_module != *module {
                    push_unique(
                        depends_by_module.entry(module.clone()).or_default(),
                        dep_module.clone(),
                    );
                    push_unique(
                        depended_by_module.entry(dep_module.clone()).or_default(),
                        module.clone(),
                    );
                    *dependents_count.entry(dep_module.clone()).or_insert(0) += 1;
                }
            }
        }
    }

    // Symbols per module (notable kinds, capped).
    let mut symbols_by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for sym in symbols.all() {
        if !is_notable(&sym.kind) {
            continue;
        }
        let list = symbols_by_module
            .entry(module_of(rel_to_root(root, &sym.file_path)))
            .or_default();
        if list.len() < MAX_SYMBOLS_PER_MODULE && !list.contains(&sym.name) {
            list.push(sym.name.clone());
        }
    }

    // Core modules = most-referenced (dependents count, descending).
    let mut ranked: Vec<(String, usize)> = dependents_count.into_iter().collect();
    ranked.sort_by_key(|x| std::cmp::Reverse(x.1));
    let core_modules: Vec<String> = ranked
        .into_iter()
        .take(MAX_CORE_MODULES)
        .map(|(m, _)| m)
        .collect();

    // Entry files by project type.
    let candidates = entry_candidates(project_type);
    let mut entries: Vec<String> = Vec::new();
    for node in &all_nodes {
        let name = node
            .path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.is_empty() && candidates.contains(&name.as_str()) && !entries.contains(&name) {
            entries.push(name);
        }
    }

    // Deterministic module list (sorted by name).
    let modules: Vec<ModuleSummary> = files_by_module
        .into_iter()
        .map(|(name, files)| {
            let mut depends_on = depends_by_module.remove(&name).unwrap_or_default();
            depends_on.sort();
            let mut depended_by = depended_by_module.remove(&name).unwrap_or_default();
            depended_by.sort();
            let symbols = symbols_by_module.remove(&name).unwrap_or_default();
            ModuleSummary {
                name,
                file_count: files.len(),
                depends_on,
                depended_by,
                symbols,
            }
        })
        .collect();

    ProjectCognition {
        modules,
        core_modules,
        entries,
    }
}

impl ProjectCognition {
    /// Render the snapshot as a compact markdown block for context injection.
    pub fn render(&self) -> String {
        let mut out = String::from("## Project Modules\n");
        for m in self.modules.iter().take(MAX_MODULES) {
            let star = if self.core_modules.contains(&m.name) { " ⭐" } else { "" };
            let deps = if m.depends_on.is_empty() {
                "-".to_string()
            } else {
                m.depends_on.join(", ")
            };
            let syms = if m.symbols.is_empty() {
                String::new()
            } else {
                format!(" · symbols: {}", m.symbols.join(", "))
            };
            out.push_str(&format!(
                "- {}{}: {} files · deps: {deps}{syms}\n",
                m.name, star, m.file_count
            ));
        }
        if self.modules.len() > MAX_MODULES {
            out.push_str(&format!(
                "…{} modules omitted\n",
                self.modules.len() - MAX_MODULES
            ));
        }
        if !self.entries.is_empty() {
            out.push_str(&format!("Entries: {}\n", self.entries.join(", ")));
        }
        out
    }

    /// Compact render for context injection — core modules + entries + deps
    /// only, WITHOUT per-module symbols (the agent queries those on demand
    /// with `search_symbols`). Keeps the per-request injected token cost
    /// small on large projects.
    pub fn render_compact(&self) -> String {
        let mut out = String::from("## Project Modules\n");
        let core: Vec<&str> = self.core_modules.iter().map(|s| s.as_str()).collect();
        let mut shown = 0;
        for m in self.modules.iter().filter(|m| core.contains(&m.name.as_str())) {
            out.push_str(&format!("- {} ⭐: deps {}\n", m.name, compact_deps(&m.depends_on)));
            shown += 1;
        }
        for m in self
            .modules
            .iter()
            .filter(|m| !core.contains(&m.name.as_str()))
            .take(MAX_MODULES.saturating_sub(shown))
        {
            out.push_str(&format!("- {}: deps {}\n", m.name, compact_deps(&m.depends_on)));
        }
        if self.modules.len() > MAX_MODULES {
            out.push_str(&format!(
                "…{} modules omitted\n",
                self.modules.len() - MAX_MODULES
            ));
        }
        if !self.entries.is_empty() {
            out.push_str(&format!("Entries: {}\n", self.entries.join(", ")));
        }
        out
    }
}

fn compact_deps(deps: &[String]) -> String {
    if deps.is_empty() {
        "-".to_string()
    } else {
        deps.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codebase::dependency::DependencyGraph;
    use crate::codebase::symbols::SymbolIndex;

    /// A throwaway temp workspace for index construction in tests.
    struct TempWs(std::path::PathBuf);
    impl TempWs {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ddc-cognition-test-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempWs(dir)
        }
        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
    }
    impl Drop for TempWs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn build_ws() -> TempWs {
        let ws = TempWs::new();
        ws.write("Cargo.toml", "[package]\nname = \"demo\"\n");
        ws.write(
            "src/main.rs",
            "mod core;\nmod ui;\nfn main() {}\nstruct App;\n",
        );
        ws.write(
            "src/core/mod.rs",
            "pub fn engine() {}\npub struct Engine;\n",
        );
        ws.write(
            "src/ui/mod.rs",
            "pub fn render() {}\npub struct Panel;\n",
        );
        ws
    }

    #[test]
    fn module_of_groups_top_level_dirs() {
        assert_eq!(module_of(Path::new("src/main.rs")), "src");
        assert_eq!(module_of(Path::new("src/core/mod.rs")), "src/core");
        assert_eq!(module_of(Path::new("README.md")), "");
        assert_eq!(module_of(Path::new("packages/web/index.ts")), "packages/web");
    }

    #[test]
    fn cognition_detects_modules_core_and_entries() {
        let ws = build_ws();
        let mut graph = DependencyGraph::new(&ws.0);
        graph.build();
        let mut symbols = SymbolIndex::new();
        symbols.index_directory(&ws.0);
        let cog = build_cognition(&graph, &symbols, &ProjectType::Rust);

        // src/main.rs declares `mod core; mod ui;` → src depends on both.
        let src = cog
            .modules
            .iter()
            .find(|m| m.name == "src")
            .expect("src module");
        assert!(src.depends_on.contains(&"src/core".to_string()), "{src:?}");
        assert!(src.depends_on.contains(&"src/ui".to_string()), "{src:?}");
        // src/core and src/ui are referenced by src → core modules.
        assert!(cog.core_modules.contains(&"src/core".to_string()), "{cog:?}");
        // Entries detected for Rust.
        assert!(cog.entries.iter().any(|e| e == "main.rs"));
        // Symbols from the symbol index surface in the snapshot.
        assert!(src.symbols.iter().any(|s| s == "main"), "{src:?}");
    }

    #[test]
    fn render_is_compact_markdown() {
        let ws = build_ws();
        let mut graph = DependencyGraph::new(&ws.0);
        graph.build();
        let mut symbols = SymbolIndex::new();
        symbols.index_directory(&ws.0);
        let cog = build_cognition(&graph, &symbols, &ProjectType::Rust);
        let out = cog.render();
        assert!(out.starts_with("## Project Modules\n"));
        assert!(out.contains("deps:"));
        assert!(out.contains("Entries: main.rs"));
    }
}
