//! Project cognition — LLM-generated architecture understanding of a
//! workspace, persisted to `.deepdepcat/project-cognition.md` and injected
//! into the agent's context so long-task planning starts with a project map.
//!
//! The deterministic module snapshot (`codebase::cognition`) gives structure;
//! this LLM pass turns it + the key source files into what each module does,
//! the architecture pattern, key paths and conventions. Generated once per
//! project, reused across sessions.

use crate::codebase::cognition::{build_cognition, ProjectCognition};
use crate::codebase::dependency::DependencyGraph;
use crate::codebase::symbols::SymbolIndex;
use crate::core::types::{ConversationItem, ProjectType};
use crate::llm::client::LlmClient;
use crate::llm::provider::{LlmProvider, LlmRequest};
use std::path::{Path, PathBuf};

/// Max characters of a persisted cognition note.
pub const MAX_COGNITION_CHARS: usize = 3000;

/// Path of the workspace cognition file (`.deepdepcat/project-cognition.md`).
pub fn cognition_path(workspace: Option<&Path>) -> Option<PathBuf> {
    workspace.map(|w| w.join(".deepdepcat").join("project-cognition.md"))
}

/// Read the persisted cognition note (if any) for injection.
pub fn read_cognition(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Persist the cognition note (atomic write, full replace).
pub fn persist_cognition_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Cap on characters read per key file for the extraction prompt.
const KEY_FILE_CHARS: usize = 800;
/// Cap on key files fed to the extraction prompt.
const MAX_KEY_FILES: usize = 6;

fn push_file(out: &mut String, path: &Path, cap: usize) {
    if out.len() > 4000 {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let body: String = content.chars().take(cap).collect();
    out.push_str(&format!(
        "### {}\n{}\n\n",
        path.display(),
        body
    ));
}

/// Assemble the key source files for the extraction prompt — entry files
/// first, then one representative file per core module.
fn key_files_content(
    workspace: &Path,
    graph: &DependencyGraph,
    cog: &ProjectCognition,
) -> String {
    let all: Vec<_> = graph.files().collect();
    let mut out = String::new();
    let mut count = 0;
    let mut take = |node: &crate::codebase::dependency::FileNode, out: &mut String| -> bool {
        if count >= MAX_KEY_FILES {
            return true;
        }
        push_file(out, &node.path, KEY_FILE_CHARS);
        count += 1;
        false
    };
    // Entry files first.
    for entry in &cog.entries {
        let found = all.iter().find(|n| {
            n.path
                .file_name()
                .map(|f| f.to_string_lossy() == *entry)
                .unwrap_or(false)
        });
        if let Some(node) = found {
            if take(node, &mut out) {
                return out;
            }
        }
    }
    // One representative file per core module.
    for m in &cog.core_modules {
        let found = all.iter().find(|n| {
            crate::codebase::cognition::module_of(n.path.strip_prefix(workspace).unwrap_or(&n.path))
                == *m
        });
        if let Some(node) = found {
            if take(node, &mut out) {
                return out;
            }
        }
    }
    out
}

/// Full generation pipeline — deterministic snapshot + key files → LLM
/// extraction → persist to `.deepdepcat/project-cognition.md`. Returns the
/// note, or `None` when generation failed (skipped, never blocks the turn).
pub async fn generate_project_cognition(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    workspace: &Path,
    graph: &DependencyGraph,
    symbols: &SymbolIndex,
    project_type: &ProjectType,
) -> Option<String> {
    let cog = build_cognition(graph, symbols, project_type);
    if cog.modules.is_empty() {
        return None;
    }
    let key_files = key_files_content(workspace, graph, &cog);
    let note = extract_cognition(llm, model, provider, &cog.render(), &key_files).await?;
    let path = cognition_path(Some(workspace))?;
    persist_cognition_file(&path, &note).ok()?;
    Some(note)
}

/// The extraction prompt — turns the deterministic snapshot + key files into
/// a concise architecture understanding.
const EXTRACT_SYSTEM: &str = r#"You are a software architect analyzing a codebase.
Given the module snapshot and key source files, write a concise PROJECT
COGNITION note that helps another agent plan long tasks. Include:
- What each module is responsible for (one line each)
- The overall architecture pattern (layered / modular / event-driven / ...)
- Key execution paths or data flows across modules
- Project conventions and constraints (build/test commands, style, gotchas)

Keep it under 600 Chinese characters (or 900 words). Plain markdown, no
headers deeper than ##, no bullet soup. Facts only — do not invent."#;

/// Extract a project-cognition note from the deterministic snapshot + key
/// file contents. Returns `None` on failure or an empty result.
pub async fn extract_cognition(
    llm: &LlmClient,
    model: &str,
    provider: Option<&str>,
    snapshot: &str,
    key_files: &str,
) -> Option<String> {
    let input = format!("## Module snapshot\n{snapshot}\n\n## Key files\n{key_files}");
    let request = LlmRequest {
        model: model.to_string(),
        provider: provider.map(str::to_string),
        messages: vec![ConversationItem::user(input)],
        tools: vec![],
        system_prompt: EXTRACT_SYSTEM.to_string(),
        temperature: Some(0.0),
        top_p: None,
        max_tokens: Some(900),
        stream: false,
        reasoning_effort: None,
        response_format: None,
        cache_control: None,
        user_id: None,
    };
    let response = llm.complete(&request).await.ok()?;
    let text = response.content.trim();
    if text.is_empty() {
        return None;
    }
    let text: String = text.chars().take(MAX_COGNITION_CHARS).collect();
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_missing_file_is_none() {
        assert!(read_cognition(Path::new("/nonexistent/xx/project-cognition.md")).is_none());
    }

    #[test]
    fn persist_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ddc-cog-test-{}", std::process::id()));
        let path = dir.join("project-cognition.md");
        persist_cognition_file(&path, "# Cognition\ncore module does X").unwrap();
        let got = read_cognition(&path).expect("read back");
        assert!(got.contains("core module does X"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
