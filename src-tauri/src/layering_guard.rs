//! Layering guard — enforces one-directional module dependencies.
//!
//! Kept as a unit test (not an integration test) so it runs fast with
//! `cargo test --lib` — an integration test would relink the whole tauri
//! dependency tree on every run (10+ minutes on Windows).
//!
//! Target stack (see `src-tauri/ARCHITECTURE.md`):
//!   entry → harness(agent) → capability(tools/permissions/hooks/memory/skills/toolkit) → model(llm) → infra
//!
//! Rules enforced:
//! - A module must not `use crate::<higher-layer>` (bootstrap exempt — it is
//!   the composition root + global-state hub every layer legitimately reads).
//! - Known exceptions are listed below and must shrink as meta-tools are
//!   re-homed or shared utilities move down (see ARCHITECTURE.md gaps).
#![cfg(test)]

use std::path::Path;

/// Recursively collect .rs files under a directory.
fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

const LAYERS: &[(&str, u8)] = &[
    // infra (0)
    ("core", 0),
    ("storage", 0),
    ("observability", 0),
    ("workspace", 0),
    ("browser", 0),
    ("codebase", 0),
    ("task", 0),
    ("sandbox", 0),
    // model (1)
    ("llm", 1),
    // capability (2)
    ("toolkit", 2),
    ("tools", 2),
    ("permissions", 2),
    ("hooks", 2),
    ("memory", 2),
    ("skills", 2),
    // harness (3)
    ("agent", 3),
    // entry (4)
    ("commands", 4),
    ("acp", 4),
    ("a2a", 4),
    ("automation", 4),
    // composition root (5) — depends on everything by design; nothing above.
    ("bootstrap", 5),
];

/// Documented exceptions — files that may reach up to the harness (agent).
/// The shared-utility imports (sanitize / stream_chunk / image_transcribe)
/// should move down to shrink this list; the real meta-tools (agent_tool,
/// workflow_tool) are the harness boundary exposed as tools.
const UPWARD_EXCEPTIONS: &[&str] = &[
    "src/tools/builtin/agent_tool.rs",
    "src/tools/builtin/workflow_tool.rs",
    "src/tools/builtin/bash.rs",
    "src/tools/builtin/plan_mode.rs",
    "src/tools/builtin/ask_user.rs",
    "src/tools/builtin/read_file.rs",
    "src/tools/builtin/visual_describe.rs",
    "src/memory/procedure.rs",
];

fn layer_of(module: &str) -> Option<u8> {
    LAYERS
        .iter()
        .find(|(name, _)| *name == module)
        .map(|&(_, l)| l)
}

/// Owning module of a source file = its first path component under `src/`.
fn owner_module(rel: &str) -> &str {
    rel.strip_prefix("src/")
        .unwrap_or(rel)
        .split('/')
        .next()
        .unwrap_or("")
}

#[test]
fn layering_is_one_directional() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations: Vec<String> = Vec::new();

    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    for path in &files {
        let rel = path
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel == "src/lib.rs" || rel == "src/main.rs" {
            continue;
        }
        let owner = owner_module(&rel);
        let owner_layer = match layer_of(owner) {
            Some(l) => l,
            None => continue, // unknown module — ignore
        };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (line_no, line) in content.lines().enumerate() {
            // Only scan use statements and crate:: paths; skip comments/doc.
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut idx = 0;
            while let Some(pos) = line[idx..].find("crate::") {
                let start = idx + pos + "crate::".len();
                let rest = &line[start..];
                let seg_end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                let target = &rest[..seg_end];
                // bootstrap = AppState global-state hub + composition root.
                // Every layer legitimately reads it; slimming it is the S4
                // follow-up (see ARCHITECTURE.md), not a layering violation.
                let skip_bootstrap = target == "bootstrap";
                if let Some(target_layer) = layer_of(target) {
                    if !skip_bootstrap && target_layer > owner_layer {
                        let rel_line = format!("{rel}:{}", line_no + 1);
                        violations.push(format!(
                            "{rel_line}  ({owner} L{owner_layer} → {target} L{target_layer})"
                        ));
                    }
                }
                idx = start + seg_end;
            }
        }
    }

    let real: Vec<String> = violations
        .iter()
        .filter(|v| {
            let file = v.split(':').next().unwrap_or("");
            !UPWARD_EXCEPTIONS.contains(&file)
        })
        .cloned()
        .collect();

    assert!(
        real.is_empty(),
        "Layering violations ({} total, {} after exceptions):\n{}",
        violations.len(),
        real.len(),
        real.join("\n")
    );
}
