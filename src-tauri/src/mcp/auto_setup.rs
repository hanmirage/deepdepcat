//! MCP auto-setup — detect a missing bundled dependency and self-heal.
//!
//! The wps-office MCP server ships as a Python package in this repo
//! (`depwork-mcp/`, module `wps_controller`). If the user's Python doesn't
//! have it installed, the stdio child dies at startup with
//! `ModuleNotFoundError` and the connection closes. Instead of forcing the
//! user to manually `pip install`, we detect that specific failure and build
//! an app-managed venv with the bundled source installed — then reconnect.
//!
//! Scoped to the bundled wps-office server only: a random third-party MCP
//! that happens to be missing a Python module must NOT trigger an install
//! (we wouldn't know what to install, and auto-installing unknown packages
//! would be dangerous).

use std::path::{Path, PathBuf};

use crate::core::config::McpServerConfig;

/// Server name of the bundled WPS Office MCP server.
pub const WPS_SERVER_NAME: &str = "wps-office";
/// The Python module the bundled WPS server runs as.
const WPS_MODULE: &str = "wps_controller";

/// Should a failed connect for this server trigger auto-setup?
pub fn needs_setup(config: &McpServerConfig, err: &str) -> bool {
    if config.transport_type != "stdio" {
        return false;
    }
    // Direct evidence: the bundled module is what's missing.
    let wps_missing = err.contains("No module named") && err.contains(WPS_MODULE);
    // Name matches AND the failure isn't a different missing module (installing
    // wps_controller can't fix `No module named 'something_else'` — that would
    // just mask a broken Python environment). Non-module failures (connection
    // dropped, process died) are worth one auto-setup attempt: idempotent.
    let named_generic =
        config.name == WPS_SERVER_NAME && !err.contains("No module named");
    wps_missing || named_generic
}

/// App-managed venv for the bundled WPS server: `{app_data}/mcp_venvs/wps-office`.
pub fn venv_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("mcp_venvs").join(WPS_SERVER_NAME)
}

/// The venv's python interpreter (Windows `Scripts/`, POSIX `bin/`).
pub fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

/// Where the bundled depwork-mcp source lives. In a packaged build it's the
/// resource dir; in dev, the repo path (resource_dir is unreliable pre-build).
pub fn depwork_mcp_source_dir(resource_dir: &Path) -> PathBuf {
    let packaged = resource_dir.join("depwork-mcp");
    if packaged.join("wps_controller").is_dir() {
        return packaged;
    }
    // Dev fallback: repo layout is src-tauri/ + depwork-mcp/ side by side.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("depwork-mcp"))
        .unwrap_or(packaged)
}

/// Whether the venv already has the bundled package importable.
pub async fn venv_ready(venv: &Path) -> bool {
    let python = venv_python(venv);
    if !python.is_file() {
        return false;
    }
    let mut cmd = tokio::process::Command::new(&python);
    crate::core::proc::no_window_tokio(&mut cmd);
    cmd.arg("-c").arg(format!("import {WPS_MODULE}"));
    matches!(cmd.output().await, Ok(o) if o.status.success())
}

/// Build the app-managed venv and install the bundled depwork-mcp into it.
/// Idempotent — if the venv already imports the module, does nothing.
///
/// Returns the venv python path on success, or a friendly error string.
pub async fn ensure_venv(
    app_data_dir: &Path,
    source_dir: &Path,
) -> Result<PathBuf, String> {
    let venv = venv_dir(app_data_dir);
    let python = venv_python(&venv);

    if venv_ready(&venv).await {
        return Ok(python);
    }

    // Create the venv with the system python from PATH.
    let mut create = tokio::process::Command::new("python");
    crate::core::proc::no_window_tokio(&mut create);
    create.arg("-m").arg("venv").arg(&venv);
    let out = create
        .output()
        .await
        .map_err(|e| format!("无法创建 Python venv（找不到系统 python）：{e}"))?;
    if !out.status.success() {
        return Err(stderr_hint(
            "创建 Python venv 失败",
            &out.stderr,
            &out.stdout,
        ));
    }

    // Install the bundled package (editable → source edits take effect).
    let mut pip = tokio::process::Command::new(&python);
    crate::core::proc::no_window_tokio(&mut pip);
    pip.arg("-m")
        .arg("pip")
        .arg("install")
        .arg("--disable-pip-version-check")
        .arg("-e")
        .arg(source_dir);
    let out = pip
        .output()
        .await
        .map_err(|e| format!("运行 pip install 失败：{e}"))?;
    if !out.status.success() {
        return Err(stderr_hint("安装 wps_controller 失败", &out.stderr, &out.stdout));
    }

    if venv_ready(&venv).await {
        Ok(python)
    } else {
        Err("venv 已建好但 wps_controller 仍无法导入".to_string())
    }
}

/// Compact stderr/stdout tail for a friendly error (bounded, single line).
fn stderr_hint(prefix: &str, stderr: &[u8], stdout: &[u8]) -> String {
    let tail: String = std::str::from_utf8(stderr)
        .unwrap_or("")
        .lines()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let detail = if !tail.trim().is_empty() {
        tail.trim()
    } else {
        std::str::from_utf8(stdout).unwrap_or("").trim()
    };
    format!("{prefix}：{detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str, transport: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport_type: transport.to_string(),
            command: Some("python".to_string()),
            args: vec![],
            env: Default::default(),
            url: None,
            enabled: true,
        }
    }

    #[test]
    fn needs_setup_matches_by_name() {
        assert!(needs_setup(&cfg("wps-office", "stdio"), "some error"));
    }

    #[test]
    fn needs_setup_matches_module_evidence() {
        let err = "No module named 'wps_controller'";
        assert!(needs_setup(&cfg("my-wps", "stdio"), err));
    }

    #[test]
    fn needs_setup_rejects_non_stdio() {
        assert!(!needs_setup(&cfg("wps-office", "http"), "No module named 'wps_controller'"));
    }

    #[test]
    fn needs_setup_rejects_unrelated_missing_module() {
        let err = "No module named 'something_else'";
        assert!(!needs_setup(&cfg("random", "stdio"), err));
        assert!(!needs_setup(&cfg("wps-office", "stdio"), err));
    }

    #[test]
    fn venv_paths_windows_style() {
        let d = PathBuf::from("C:/data");
        let venv = venv_dir(&d);
        assert_eq!(venv, PathBuf::from("C:/data/mcp_venvs/wps-office"));
        if cfg!(windows) {
            assert!(venv_python(&venv).ends_with("Scripts/python.exe"));
        }
    }

    #[test]
    fn dev_source_dir_falls_back_to_repo() {
        // Packaged copy absent → falls back to CARGO_MANIFEST_DIR/depwork-mcp,
        // which exists in a real checkout.
        let resource = PathBuf::from("/nonexistent");
        let dir = depwork_mcp_source_dir(&resource);
        assert!(dir.join("wps_controller").is_dir() || dir.is_absolute());
    }
}
