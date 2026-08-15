use super::extract::{extract_python_symbols, extract_rust_symbols, extract_ts_symbols};
use super::*;

#[test]
fn test_language_from_extension() {
    assert_eq!(Language::from_extension("rs"), Language::Rust);
    assert_eq!(Language::from_extension("ts"), Language::TypeScript);
    assert_eq!(Language::from_extension("py"), Language::Python);
    assert_eq!(Language::from_extension("xyz"), Language::Unknown);
}

#[test]
fn test_extract_rust_symbols() {
    let code = r#"
pub struct MyStruct {
    field: i32,
}

pub fn my_function(x: i32) -> i32 {
    x + 1
}

enum MyEnum {
    A,
    B,
}

trait MyTrait {
    fn method(&self);
}

const MY_CONST: u32 = 42;
"#;
    let path = Path::new("test.rs");
    let symbols = extract_rust_symbols(path, code);

    assert_eq!(symbols.len(), 6);
    assert!(symbols
        .iter()
        .any(|s| s.name == "MyStruct" && s.kind == SymbolKind::Struct));
    assert!(symbols
        .iter()
        .any(|s| s.name == "my_function" && s.kind == SymbolKind::Function));
    assert!(symbols
        .iter()
        .any(|s| s.name == "MyEnum" && s.kind == SymbolKind::Enum));
    assert!(symbols
        .iter()
        .any(|s| s.name == "MyTrait" && s.kind == SymbolKind::Trait));
    assert!(symbols
        .iter()
        .any(|s| s.name == "MY_CONST" && s.kind == SymbolKind::Const));
}

#[test]
fn test_extract_rust_private_symbol() {
    let code = r#"
fn private_function() {}
pub fn public_function() {}
"#;
    let path = Path::new("test.rs");
    let symbols = extract_rust_symbols(path, code);

    assert_eq!(symbols.len(), 2);
    let private = symbols
        .iter()
        .find(|s| s.name == "private_function")
        .unwrap();
    assert!(!private.is_public);
    let public = symbols
        .iter()
        .find(|s| s.name == "public_function")
        .unwrap();
    assert!(public.is_public);
}

#[test]
fn test_extract_ts_symbols() {
    let code = r#"
export function myFunction(x: number): number {
    return x + 1;
}

export class MyClass {
    private method(): void {}
}

export interface MyInterface {
    prop: string;
}

const myConst = 42;
"#;
    let path = Path::new("test.ts");
    let symbols = extract_ts_symbols(path, code);

    assert!(symbols
        .iter()
        .any(|s| s.name == "myFunction" && s.kind == SymbolKind::Function));
    assert!(symbols
        .iter()
        .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
    assert!(symbols
        .iter()
        .any(|s| s.name == "MyInterface" && s.kind == SymbolKind::Interface));
    assert!(symbols
        .iter()
        .any(|s| s.name == "myConst" && s.kind == SymbolKind::Const));
}

#[test]
fn test_extract_python_symbols() {
    let code = r#"
class MyClass:
    def __init__(self):
        pass

    def public_method(self):
        pass

    def _private_method(self):
        pass

def standalone_function():
    pass
"#;
    let path = Path::new("test.py");
    let symbols = extract_python_symbols(path, code);

    assert!(symbols
        .iter()
        .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
    assert!(symbols
        .iter()
        .any(|s| s.name == "public_method" && s.is_public));
    assert!(symbols
        .iter()
        .any(|s| s.name == "_private_method" && !s.is_public));
    assert!(symbols
        .iter()
        .any(|s| s.name == "standalone_function" && s.kind == SymbolKind::Function));
}

#[test]
fn test_symbol_index_find_by_name() {
    let mut index = SymbolIndex::new();
    let code = "pub fn my_function() {}";
    index.index_file(Path::new("test.rs"), code);

    let results = index.find_by_name("my_function");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "my_function");

    // Case-insensitive
    let results = index.find_by_name("MY_FUNCTION");
    assert_eq!(results.len(), 1);
}

#[test]
fn test_symbol_index_find_by_prefix() {
    let mut index = SymbolIndex::new();
    index.index_file(
        Path::new("test.rs"),
        "pub fn my_function() {}\npub fn my_method() {}",
    );

    let results = index.find_by_prefix("my_");
    assert_eq!(results.len(), 2);
}

#[test]
fn test_symbol_index_staleness_lifecycle() {
    let mut index = SymbolIndex::new();
    // Fresh index: no root, not stale — a lookup must build.
    assert_eq!(index.indexed_root, None);
    assert!(!index.stale);
    // After directory indexing: root pinned, stale cleared.
    let dir = std::env::temp_dir().join(format!("ddc-symtest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(dir.join("a.rs"), "pub fn alpha() {}").ok();
    index.index_directory(&dir);
    assert_eq!(index.indexed_root.as_deref(), Some(dir.as_path()));
    assert!(!index.stale);
    // A write during the agent session marks the cache stale.
    index.mark_stale();
    assert!(index.stale);
    // Re-indexing clears it again.
    index.index_directory(&dir);
    assert!(!index.stale);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_symbol_index_tracks_rebuild_root() {
    let mut index = SymbolIndex::new();
    let dir_a = std::env::temp_dir().join(format!("ddc-symtest-a-{}", std::process::id()));
    let dir_b = std::env::temp_dir().join(format!("ddc-symtest-b-{}", std::process::id()));
    std::fs::create_dir_all(&dir_a).ok();
    std::fs::create_dir_all(&dir_b).ok();
    std::fs::write(dir_a.join("a.rs"), "pub fn alpha() {}").ok();
    std::fs::write(dir_b.join("b.rs"), "pub fn beta() {}").ok();
    index.index_directory(&dir_a);
    // Workspace switch must be detectable via the recorded root.
    assert_eq!(index.indexed_root.as_deref(), Some(dir_a.as_path()));
    index.index_directory(&dir_b);
    assert_eq!(index.indexed_root.as_deref(), Some(dir_b.as_path()));
    assert!(index.find_by_name("beta").len() == 1);
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
