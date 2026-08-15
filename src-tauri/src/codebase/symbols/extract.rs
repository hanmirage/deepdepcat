use regex::Regex;
use std::path::Path;

use super::types::{Language, Symbol, SymbolKind};

/// Safe capture-group extraction — the regexes below always define their
/// groups when they match, so this never actually misses; a future edit
/// that drops or renumbers a group degrades to an empty string instead of
/// panicking on arbitrary user code.
fn cap(caps: &regex::Captures<'_>, idx: usize) -> String {
    caps.get(idx)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// Extract symbols from source code.
pub fn extract_symbols(path: &Path, content: &str, language: Language) -> Vec<Symbol> {
    match language {
        Language::Rust => extract_rust_symbols(path, content),
        Language::TypeScript | Language::JavaScript => extract_ts_symbols(path, content),
        Language::Python => extract_python_symbols(path, content),
        Language::Unknown => vec![],
    }
}

/// Extract symbols from Rust source code.
pub fn extract_rust_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    // Patterns for Rust definitions
    let fn_re = Regex::new(
        r"^\s*(pub(?:\([^)]+\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+[^}]+)?fn\s+(\w+)",
    )
    .unwrap();
    let struct_re = Regex::new(r"^\s*(pub(?:\([^)]+\))?\s+)?struct\s+(\w+)").unwrap();
    let enum_re = Regex::new(r"^\s*(pub(?:\([^)]+\))?\s+)?enum\s+(\w+)").unwrap();
    let trait_re = Regex::new(r"^\s*(pub(?:\([^)]+\))?\s+)?trait\s+(\w+)").unwrap();
    let mod_re = Regex::new(r"^\s*(pub(?:\([^)]+\))?\s+)?mod\s+(\w+)").unwrap();
    let impl_re = Regex::new(r"^\s*impl(?:<[^>]+>)?\s+([\w<>]+)").unwrap();
    let const_re = Regex::new(r"^\s*(pub(?:\([^)]+\))?\s+)?(?:const|static)\s+(\w+)").unwrap();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1; // 1-indexed

        if let Some(caps) = fn_re.captures(line) {
            let is_public = caps.get(1).is_some();
            let name = cap(&caps, 2);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = struct_re.captures(line) {
            let is_public = caps.get(1).is_some();
            let name = cap(&caps, 2);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Struct,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = enum_re.captures(line) {
            let is_public = caps.get(1).is_some();
            let name = cap(&caps, 2);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Enum,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = trait_re.captures(line) {
            let is_public = caps.get(1).is_some();
            let name = cap(&caps, 2);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Trait,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = mod_re.captures(line) {
            let is_public = caps.get(1).is_some();
            let name = cap(&caps, 2);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Module,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = impl_re.captures(line) {
            let name = cap(&caps, 1);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Unknown,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public: true,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = const_re.captures(line) {
            let is_public = caps.get(1).is_some();
            let name = cap(&caps, 2);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Const,
                language: Language::Rust,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        }
    }

    symbols
}

/// Extract symbols from TypeScript/JavaScript source code.
pub fn extract_ts_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let language = Language::from_path(path);

    let fn_re = Regex::new(r"^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)").unwrap();
    let class_re = Regex::new(r"^\s*(?:export\s+)?(?:abstract\s+)?class\s+(\w+)").unwrap();
    let interface_re = Regex::new(r"^\s*(?:export\s+)?interface\s+(\w+)").unwrap();
    let const_re = Regex::new(r"^\s*(?:export\s+)?const\s+(\w+)\s*=").unwrap();
    let method_re = Regex::new(
        r"^\s+(?:public|private|protected|static|async|readonly|\s)*(\w+)\s*\([^)]*\)\s*[{:]",
    )
    .unwrap();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;

        if let Some(caps) = fn_re.captures(line) {
            let is_public = line.contains("export");
            let name = cap(&caps, 1);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                language,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = class_re.captures(line) {
            let is_public = line.contains("export");
            let name = cap(&caps, 1);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                language,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = interface_re.captures(line) {
            let is_public = line.contains("export");
            let name = cap(&caps, 1);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Interface,
                language,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = const_re.captures(line) {
            let is_public = line.contains("export");
            let name = cap(&caps, 1);
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Const,
                language,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = method_re.captures(line) {
            let name = cap(&caps, 1);
            // Skip common keywords that aren't method names
            if !matches!(
                name.as_str(),
                "if" | "for" | "while" | "switch" | "return" | "constructor"
            ) {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Method,
                    language,
                    file_path: path.to_path_buf(),
                    line: line_num,
                    end_line: 0,
                    is_public: !line.contains("private"),
                    signature: line.trim().to_string(),
                });
            }
        }
    }

    symbols
}

/// Extract symbols from Python source code.
pub fn extract_python_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    let fn_re = Regex::new(r"^\s*(?:async\s+)?def\s+(\w+)").unwrap();
    let class_re = Regex::new(r"^\s*class\s+(\w+)").unwrap();

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num + 1;

        if let Some(caps) = fn_re.captures(line) {
            let name = cap(&caps, 1);
            let is_public = !name.starts_with('_');
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                language: Language::Python,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        } else if let Some(caps) = class_re.captures(line) {
            let name = cap(&caps, 1);
            let is_public = !name.starts_with('_');
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                language: Language::Python,
                file_path: path.to_path_buf(),
                line: line_num,
                end_line: 0,
                is_public,
                signature: line.trim().to_string(),
            });
        }
    }

    symbols
}
