//! Repository type discovery — detects project language, build system, and
//! key configuration files from a workspace root.
//!
//! Scans for marker files (Cargo.toml, package.json, pyproject.toml, etc.)
//! and returns a structured `ProjectType` that the agent can use to tailor
//! its behavior (e.g., which linter to run, which file patterns to search).

use std::path::Path;
use tracing::debug;

use crate::core::types::ProjectType;

/// Discovered project information.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// The primary project type.
    pub project_type: ProjectType,
}

/// Discover the project type from a workspace root.
pub fn discover(root: &Path) -> ProjectInfo {
    let mut types = Vec::new();

    // ── Rust ──────────────────────────────────────────────────────
    if root.join("Cargo.toml").is_file() {
        types.push(ProjectType::Rust);
    }

    // ── Node.js ───────────────────────────────────────────────────
    if root.join("package.json").is_file() {
        if root.join("pnpm-lock.yaml").is_file() {
            types.push(ProjectType::NodePnpm);
        } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
            types.push(ProjectType::NodeBun);
        } else {
            types.push(ProjectType::NodeNpm);
        }
    }

    // ── Python ─────────────────────────────────────────────────────
    if root.join("pyproject.toml").is_file() {
        let content = std::fs::read_to_string(root.join("pyproject.toml")).unwrap_or_default();
        if content.contains("poetry") || root.join("poetry.lock").is_file() {
            types.push(ProjectType::PythonPoetry);
        } else if content.contains("uv") || root.join("uv.lock").is_file() {
            types.push(ProjectType::PythonUv);
        } else {
            types.push(ProjectType::PythonPip);
        }
    } else if root.join("requirements.txt").is_file() {
        types.push(ProjectType::PythonPip);
    }

    // ── Go ────────────────────────────────────────────────────────
    if root.join("go.mod").is_file() {
        types.push(ProjectType::Go);
    }

    // ── Java ──────────────────────────────────────────────────────
    if root.join("pom.xml").is_file() {
        types.push(ProjectType::JavaMaven);
    }
    if root.join("build.gradle").is_file() || root.join("build.gradle.kts").is_file() {
        types.push(ProjectType::JavaGradle);
    }

    // ── C/C++ ─────────────────────────────────────────────────────
    if root.join("CMakeLists.txt").is_file() {
        types.push(ProjectType::Cmake);
    }

    // ── .NET ──────────────────────────────────────────────────────
    if root.join("*.csproj").exists() || has_extension(root, "csproj") {
        types.push(ProjectType::Dotnet);
    }

    // Monorepo detection: multiple project types or workspace markers
    let is_monorepo = types.len() > 2
        || root.join("pnpm-workspace.yaml").is_file()
        || root.join("lerna.json").is_file()
        || root.join("nx.json").is_file()
        || root.join("Cargo.toml").is_file()
            && std::fs::read_to_string(root.join("Cargo.toml"))
                .unwrap_or_default()
                .contains("[workspace]");

    let project_type = if is_monorepo && types.len() > 1 {
        ProjectType::Monorepo
    } else {
        types.first().cloned().unwrap_or(ProjectType::Unknown)
    };

    debug!(
        root = ?root,
        project_type = project_type.as_str(),
        secondary_count = types.len().saturating_sub(1),
        "Project discovered"
    );

    ProjectInfo { project_type }
}

/// Check if the directory contains any file with the given extension.
fn has_extension(dir: &Path, ext: &str) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some(ext)),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detect_rust_project() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();

        let info = discover(tmp.path());
        assert_eq!(info.project_type, ProjectType::Rust);
    }

    #[test]
    fn detect_node_npm_project() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"test"}"#).unwrap();

        let info = discover(tmp.path());
        assert_eq!(info.project_type, ProjectType::NodeNpm);
    }

    #[test]
    fn detect_node_pnpm_project() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"test"}"#).unwrap();
        fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();

        let info = discover(tmp.path());
        assert_eq!(info.project_type, ProjectType::NodePnpm);
    }

    #[test]
    fn detect_python_poetry_project() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            "[tool.poetry]\nname = \"test\"",
        )
        .unwrap();

        let info = discover(tmp.path());
        assert_eq!(info.project_type, ProjectType::PythonPoetry);
    }

    #[test]
    fn detect_unknown_project() {
        let tmp = tempfile::tempdir().unwrap();
        let info = discover(tmp.path());
        assert_eq!(info.project_type, ProjectType::Unknown);
    }

    #[test]
    fn detect_monorepo() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\"]",
        )
        .unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"root"}"#).unwrap();

        let info = discover(tmp.path());
        assert_eq!(info.project_type, ProjectType::Monorepo);
    }
}
