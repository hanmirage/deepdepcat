//! Multi-language symbol extraction.
//!
//! Extracts function, struct, class, and interface definitions from source
//! files using regex-based parsing. Supports:
//! - **Rust**: `fn`, `struct`, `enum`, `impl`, `trait`, `mod`
//! - **TypeScript/JavaScript**: `function`, `class`, `interface`, `const`
//! - **Python**: `def`, `class`

mod extract;
mod types;
mod walk;

#[cfg(test)]
mod tests;

pub use types::*;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

/// The symbol index — a collection of symbols extracted from a codebase.
#[derive(Clone)]
pub struct SymbolIndex {
    /// All symbols, indexed by file path.
    symbols_by_file: HashMap<PathBuf, Vec<Symbol>>,
    /// All symbols, indexed by name (lowercase for case-insensitive lookup).
    symbols_by_name: HashMap<String, Vec<Symbol>>,
    /// All symbols, flat list.
    all_symbols: Vec<Symbol>,
    /// The workspace this index was built for. A lookup against a DIFFERENT
    /// workspace (or `None` before the first build) must rebuild — serving
    /// another project's symbols would be silently wrong.
    pub indexed_root: Option<PathBuf>,
    /// Set when files were written after indexing. The cached content is
    /// then stale and must be rebuilt before serving lookups — a long agent
    /// session must not answer "where is X defined?" from a pre-edit index.
    pub stale: bool,
}

impl Default for SymbolIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolIndex {
    /// Create an empty symbol index.
    pub fn new() -> Self {
        Self {
            symbols_by_file: HashMap::new(),
            symbols_by_name: HashMap::new(),
            all_symbols: Vec::new(),
            indexed_root: None,
            stale: false,
        }
    }

    /// Mark the cached content stale after a file write — the next lookup
    /// rebuilds instead of serving pre-edit symbols.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Index a single file, extracting all symbols.
    pub fn index_file(&mut self, path: &Path, content: &str) {
        let language = Language::from_path(path);
        if language == Language::Unknown {
            return;
        }

        let symbols = extract::extract_symbols(path, content, language);
        debug!(
            file = %path.display(),
            language = ?language,
            count = symbols.len(),
            "Indexed file symbols"
        );

        for sym in &symbols {
            let name_lower = sym.name.to_lowercase();
            self.symbols_by_name
                .entry(name_lower)
                .or_default()
                .push(sym.clone());
        }

        self.symbols_by_file
            .entry(path.to_path_buf())
            .or_default()
            .extend(symbols.clone());
        self.all_symbols.extend(symbols);
    }

    /// Index a directory recursively, extracting symbols from all supported files.
    pub fn index_directory(&mut self, root: &Path) {
        let walker = walk::walkdir(root);
        for file_path in walker {
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                self.index_file(&file_path, &content);
            }
        }
        self.indexed_root = Some(root.to_path_buf());
        self.stale = false;
        info!(
            files = self.symbols_by_file.len(),
            symbols = self.all_symbols.len(),
            "Directory indexing complete"
        );
    }

    /// Find symbols by name (case-insensitive).
    pub fn find_by_name(&self, name: &str) -> Vec<&Symbol> {
        self.symbols_by_name
            .get(&name.to_lowercase())
            .map(|syms| syms.iter().collect())
            .unwrap_or_default()
    }

    /// Find symbols by prefix (case-insensitive).
    pub fn find_by_prefix(&self, prefix: &str) -> Vec<&Symbol> {
        let prefix_lower = prefix.to_lowercase();
        self.all_symbols
            .iter()
            .filter(|s| s.name.to_lowercase().starts_with(&prefix_lower))
            .collect()
    }

    /// Get all indexed symbols.
    pub fn all(&self) -> &[Symbol] {
        &self.all_symbols
    }
}
