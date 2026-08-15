//! File dependency graph — builds a file-level import dependency DAG.
//!
//! Uses regex-based import extraction to determine which files reference which
//! other files. Supports:
//! - **Rust**: `use`, `mod`, `extern crate`
//! - **TypeScript/JavaScript**: `import`, `require`
//! - **Python**: `import`, `from ... import`

use crate::codebase::symbols::Language;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Safe capture-group extraction — the regexes below always define their
/// groups when they match; a future edit that drops a group degrades to an
/// empty string instead of panicking on arbitrary user code.
fn cap(caps: &regex::Captures<'_>, idx: usize) -> String {
    caps.get(idx)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// A node in the dependency graph — represents a single source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    /// The file path (relative to the project root).
    pub path: PathBuf,
    /// The programming language.
    pub language: Language,
    /// Paths of files that this file depends on (imports).
    pub dependencies: Vec<PathBuf>,
    /// Paths of files that depend on this file (importers).
    pub dependents: Vec<PathBuf>,
}

/// An edge in the dependency graph — from source to target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// The dependency graph — a DAG of file-level dependencies.
#[derive(Clone)]
pub struct DependencyGraph {
    /// All file nodes, indexed by path.
    nodes: HashMap<PathBuf, FileNode>,
    /// All edges.
    edges: Vec<DependencyEdge>,
    /// The project root (for resolving relative imports).
    root: PathBuf,
    /// Staleness flag — set after any file write or external workspace
    /// change; the next lookup rebuilds instead of answering from
    /// pre-edit imports.
    stale: bool,
}

impl DependencyGraph {
    /// Create a new empty dependency graph with the given root.
    pub fn new(root: &Path) -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            root: root.to_path_buf(),
            stale: false,
        }
    }

    /// The workspace root this graph was built for — a lookup against a
    /// different root must rebuild (cached edges of another project would
    /// be silently wrong).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Mark the graph stale — the next lookup triggers a rebuild. Called
    /// after every successful file write and on external file changes, so
    /// `file_dependencies` never answers from pre-edit imports (same
    /// contract as `SymbolIndex::mark_stale`).
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Whether the graph is stale (needs a rebuild before the next read).
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Build the dependency graph by scanning the project directory.
    pub fn build(&mut self) {
        let files = self.walk_source_files(&self.root.clone());
        info!(file_count = files.len(), "Building dependency graph");

        for file_path in &files {
            if let Ok(content) = std::fs::read_to_string(file_path) {
                self.add_file(file_path, &content);
            }
        }

        // Resolve dependencies to actual file paths
        self.resolve_dependencies();

        // The rebuild consumed the staleness — the graph is fresh again.
        self.stale = false;

        info!(
            nodes = self.nodes.len(),
            edges = self.edges.len(),
            "Dependency graph built"
        );
    }

    /// Add a file to the graph, extracting its imports.
    pub fn add_file(&mut self, path: &Path, content: &str) {
        let language = Language::from_path(path);
        let imports = extract_imports(content, language);

        let node = FileNode {
            path: path.to_path_buf(),
            language,
            dependencies: imports.iter().map(PathBuf::from).collect(),
            dependents: vec![],
        };

        debug!(
            file = %path.display(),
            language = ?language,
            import_count = node.dependencies.len(),
            "Added file to dependency graph"
        );

        self.nodes.insert(path.to_path_buf(), node);
    }

    /// Resolve string import paths to actual file paths.
    fn resolve_dependencies(&mut self) {
        let mut edges = Vec::new();
        let mut dependents_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

        let node_paths: Vec<PathBuf> = self.nodes.keys().cloned().collect();

        for node_path in &node_paths {
            let Some(node) = self.nodes.get(node_path) else {
                continue;
            };
            let resolved_deps = self.resolve_imports(&node.dependencies, node_path, node.language);

            for dep_path in &resolved_deps {
                edges.push(DependencyEdge {
                    from: node_path.clone(),
                    to: dep_path.clone(),
                });
                dependents_map
                    .entry(dep_path.clone())
                    .or_default()
                    .push(node_path.clone());
            }

            // Update the node's resolved dependencies
            if let Some(node) = self.nodes.get_mut(node_path) {
                node.dependencies = resolved_deps;
            }
        }

        // Update dependents
        for (file_path, dependents) in dependents_map {
            if let Some(node) = self.nodes.get_mut(&file_path) {
                node.dependents = dependents;
            }
        }

        self.edges = edges;
    }

    /// Resolve import strings to actual file paths.
    fn resolve_imports(
        &self,
        imports: &[PathBuf],
        source_file: &Path,
        language: Language,
    ) -> Vec<PathBuf> {
        let mut resolved = Vec::new();

        for import in imports {
            let import_str = import.to_string_lossy();
            let candidates = self.resolve_import(&import_str, source_file, language);

            for candidate in candidates {
                if self.nodes.contains_key(&candidate) {
                    resolved.push(candidate);
                }
            }
        }

        resolved
    }

    /// Resolve a single import string to candidate file paths.
    fn resolve_import(&self, import: &str, source_file: &Path, language: Language) -> Vec<PathBuf> {
        let mut candidates = Vec::new();

        match language {
            Language::Rust => {
                // Rust uses module paths like "crate::foo::bar" or "foo::bar"
                let module_path = import.replace("::", "/");
                let base = &self.root;

                candidates.push(base.join(format!("src/{}.rs", module_path)));
                candidates.push(base.join(format!("src/{}/mod.rs", module_path)));
                candidates.push(base.join(format!("{}.rs", module_path)));
                candidates.push(base.join(format!("{}/mod.rs", module_path)));
            }
            Language::TypeScript | Language::JavaScript => {
                // TS/JS uses relative or absolute paths
                let source_dir = source_file.parent().unwrap_or(&self.root);

                // Try various extensions
                for ext in &["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
                    candidates.push(source_dir.join(format!("{}.{}", import, ext)));
                }
                // Try as directory with index
                for ext in &["ts", "tsx", "js", "jsx"] {
                    candidates.push(source_dir.join(format!("{}/index.{}", import, ext)));
                }
            }
            Language::Python => {
                // Python uses dotted module paths
                let module_path = import.replace('.', "/");
                let source_dir = source_file.parent().unwrap_or(&self.root);

                candidates.push(source_dir.join(format!("{}.py", module_path)));
                candidates.push(source_dir.join(format!("{}/__init__.py", module_path)));
                candidates.push(self.root.join(format!("{}.py", module_path)));
                candidates.push(self.root.join(format!("{}/__init__.py", module_path)));
            }
            Language::Unknown => {}
        }

        // Filter to only existing candidates
        candidates.into_iter().filter(|p| p.exists()).collect()
    }

    /// Walk the project directory, yielding all source files.
    fn walk_source_files(&self, root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        self.walk_recursive(root, &mut files);
        files
    }

    fn walk_recursive(&self, dir: &Path, files: &mut Vec<PathBuf>) {
        if !dir.is_dir() {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    ".git" | "node_modules" | "target" | "dist" | "build" | "__pycache__" | ".next"
                ) {
                    continue;
                }
                self.walk_recursive(&path, files);
            } else if path.is_file() {
                let lang = Language::from_path(&path);
                if lang != Language::Unknown {
                    files.push(path);
                }
            }
        }
    }

    /// Iterate over all file nodes in the graph — used for module-level
    /// aggregation (the project-cognition snapshot groups files by top-level
    /// directory and sums cross-module dependency edges).
    pub fn files(&self) -> impl Iterator<Item = &FileNode> {
        self.nodes.values()
    }

    /// Get the dependencies of a file (files it imports).
    pub fn dependencies_of(&self, path: &Path) -> Vec<&FileNode> {
        self.nodes
            .get(path)
            .map(|n| {
                n.dependencies
                    .iter()
                    .filter_map(|dep| self.nodes.get(dep))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the dependents of a file (files that import it).
    pub fn dependents_of(&self, path: &Path) -> Vec<&FileNode> {
        self.nodes
            .get(path)
            .map(|n| {
                n.dependents
                    .iter()
                    .filter_map(|dep| self.nodes.get(dep))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Extract import statements from source code.
fn extract_imports(content: &str, language: Language) -> Vec<String> {
    match language {
        Language::Rust => extract_rust_imports(content),
        Language::TypeScript | Language::JavaScript => extract_ts_imports(content),
        Language::Python => extract_python_imports(content),
        Language::Unknown => vec![],
    }
}

/// Extract Rust imports (use statements, mod declarations).
fn extract_rust_imports(content: &str) -> Vec<String> {
    let mut imports = Vec::new();

    // use crate::foo::bar;
    // use foo::bar;
    // use foo::{bar, baz};
    let use_re = Regex::new(r"^\s*use\s+([\w:]+)").unwrap();
    // mod foo;
    let mod_re = Regex::new(r"^\s*mod\s+(\w+)").unwrap();

    for line in content.lines() {
        if let Some(caps) = use_re.captures(line) {
            let path = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            // Strip the last segment if it's a brace (e.g., "foo::{bar" → "foo")
            let clean = path.split('{').next().unwrap_or(path);
            // Remove leading "crate::" or "self::" or "super::"
            let clean = clean
                .strip_prefix("crate::")
                .or_else(|| clean.strip_prefix("self::"))
                .or_else(|| clean.strip_prefix("super::"))
                .unwrap_or(clean);
            imports.push(clean.to_string());
        }
        if let Some(caps) = mod_re.captures(line) {
            imports.push(cap(&caps, 1));
        }
    }

    imports
}

/// Extract TypeScript/JavaScript imports.
fn extract_ts_imports(content: &str) -> Vec<String> {
    let mut imports = Vec::new();

    // import { foo } from './bar';
    // import foo from './bar';
    // import * as foo from './bar';
    // const foo = require('./bar');
    let import_re =
        Regex::new(r#"(?:import\s+.*?\s+from\s+|require\s*\(\s*)['"]([^'"]+)['"]"#).unwrap();

    for line in content.lines() {
        for caps in import_re.captures_iter(line) {
            imports.push(cap(&caps, 1));
        }
    }

    imports
}

/// Extract Python imports.
fn extract_python_imports(content: &str) -> Vec<String> {
    let mut imports = Vec::new();

    // import foo.bar
    let import_re = Regex::new(r"^\s*import\s+([\w.]+)").unwrap();
    // from foo.bar import baz
    let from_re = Regex::new(r"^\s*from\s+([\w.]+)\s+import").unwrap();

    for line in content.lines() {
        if let Some(caps) = import_re.captures(line) {
            imports.push(cap(&caps, 1));
        }
        if let Some(caps) = from_re.captures(line) {
            imports.push(cap(&caps, 1));
        }
    }

    imports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rust_imports() {
        let code = r#"
use std::collections::HashMap;
use crate::foo::bar;
use foo::baz;
mod my_module;
"#;
        let imports = extract_rust_imports(code);
        assert!(imports.contains(&"std::collections::HashMap".to_string()));
        assert!(imports.contains(&"foo::bar".to_string()));
        assert!(imports.contains(&"foo::baz".to_string()));
        assert!(imports.contains(&"my_module".to_string()));
    }

    #[test]
    fn test_extract_ts_imports() {
        let code = r#"
import { foo } from './bar';
import baz from '../utils/baz';
const qux = require('./qux');
"#;
        let imports = extract_ts_imports(code);
        assert!(imports.contains(&"./bar".to_string()));
        assert!(imports.contains(&"../utils/baz".to_string()));
        assert!(imports.contains(&"./qux".to_string()));
    }

    #[test]
    fn test_extract_python_imports() {
        let code = r#"
import os
import sys.path
from foo.bar import baz
from .local import thing
"#;
        let imports = extract_python_imports(code);
        assert!(imports.contains(&"os".to_string()));
        assert!(imports.contains(&"sys.path".to_string()));
        assert!(imports.contains(&"foo.bar".to_string()));
        assert!(imports.contains(&".local".to_string()));
    }

    #[test]
    fn test_dependency_graph_exposes_root() {
        let graph = DependencyGraph::new(Path::new("/proj/ws"));
        assert_eq!(graph.root(), Path::new("/proj/ws"));
    }

    #[test]
    fn stale_flag_roundtrips_and_build_clears_it() {
        // Fresh graphs are not stale.
        let graph = DependencyGraph::new(Path::new("/proj/ws"));
        assert!(!graph.is_stale());

        // A write event marks the graph stale (C4: same contract as
        // SymbolIndex::mark_stale) — the next lookup must rebuild.
        let mut graph = graph;
        graph.mark_stale();
        assert!(graph.is_stale());

        // A rebuild consumes the staleness — the graph is fresh again.
        graph.build();
        assert!(!graph.is_stale());
    }
}
