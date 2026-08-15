use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;

/// The programming language of a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Unknown,
}

impl Language {
    /// Detect the language from a file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "ts" | "tsx" => Self::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" => Self::Python,
            _ => Self::Unknown,
        }
    }

    /// Detect the language from a file path.
    pub fn from_path(path: &Path) -> Self {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(Self::from_extension)
            .unwrap_or(Self::Unknown)
    }
}

/// The kind of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Interface,
    Class,
    Trait,
    Module,
    Const,
    Method,
    Unknown,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Class => "class",
            Self::Trait => "trait",
            Self::Module => "module",
            Self::Const => "const",
            Self::Method => "method",
            Self::Unknown => "unknown",
        }
    }
}

/// A symbol extracted from a source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// The symbol name (e.g., "MyFunction", "MyStruct").
    pub name: String,
    /// The kind of symbol.
    pub kind: SymbolKind,
    /// The language of the source file.
    pub language: Language,
    /// The file path where the symbol was found.
    pub file_path: PathBuf,
    /// The line number where the symbol definition starts (1-indexed).
    pub line: usize,
    /// The end line number (for multi-line definitions, 0 if unknown).
    pub end_line: usize,
    /// Whether the symbol is public (visible outside the module).
    pub is_public: bool,
    /// The raw signature line (first line of the definition).
    pub signature: String,
}
