//! Office COM host plumbing for `office_automate`.
//!
//! Owns the one-shot bridge runner and the persistent host process that
//! keeps ONE WPS/Office instance + visible window alive across calls (so
//! consecutive writes appear live in the user's open document window).
//! Script contents + format constants live in `office_scripts`.

use crate::toolkit::{ToolContext, ToolResult};
use crate::core::error::AppResult;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

pub use super::office_scripts::{
    app_for_extension, bgr_value, save_format_for, BRIDGE_SCRIPT, HOST_CALC_SCRIPT,
    HOST_IMPRESS_SCRIPT, HOST_SCRIPT,
};

/// Persistent host state shared with the stdout reader thread.
struct HostShared {
    responses: VecDeque<String>,
}

/// A long-lived PowerShell process holding the office COM instance.
struct OfficeHost {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    shared: Arc<(Mutex<HostShared>, Condvar)>,
}

static OFFICE_HOST: OnceLock<Mutex<Option<OfficeHost>>> = OnceLock::new();

fn spawn_host() -> AppResult<OfficeHost> {
    let dir = std::env::temp_dir();
    let scripts: [(&str, &str); 3] = [
        ("ddc_office_host.ps1", HOST_SCRIPT),
        ("ddc_office_host_calc.ps1", HOST_CALC_SCRIPT),
        ("ddc_office_host_impress.ps1", HOST_IMPRESS_SCRIPT),
    ];
    for (name, content) in scripts {
        let mut f = std::fs::File::create(dir.join(name))?;
        f.write_all(content.as_bytes())?;
    }
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(dir.join("ddc_office_host.ps1"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch PowerShell office host: {e}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "office host: no stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "office host: no stdout".to_string())?;
    let shared = Arc::new((
        Mutex::new(HostShared {
            responses: VecDeque::new(),
        }),
        Condvar::new(),
    ));
    let reader_shared = shared.clone();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let (m, c) = &*reader_shared;
            m.lock()
                .unwrap_or_else(|e| e.into_inner())
                .responses
                .push_back(line);
            c.notify_one();
        }
    });
    Ok(OfficeHost {
        child,
        stdin,
        shared,
    })
}

/// Wait for the next response line, up to `deadline` (type_text can take
/// minutes at a slow typing pace).
fn wait_for_host_response(
    shared: &Arc<(Mutex<HostShared>, Condvar)>,
    deadline: std::time::Instant,
) -> AppResult<String> {
    let (m, c) = &**shared;
    let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(line) = q.responses.pop_front() {
            return Ok(line);
        }
        if std::time::Instant::now() >= deadline {
            return Err("Office host timed out (no response)".into());
        }
        let (g, _) = c
            .wait_timeout(q, std::time::Duration::from_millis(200))
            .unwrap_or_else(|e| e.into_inner());
        q = g;
    }
}

/// Send a write action to the persistent office host and return its JSON
/// response. The host is (re)started when missing or crashed.
pub fn host_call(config: &Value) -> AppResult<Value> {
    let cell = OFFICE_HOST.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());

    for _attempt in 0..2 {
        let dead = match guard.as_mut() {
            None => true,
            Some(h) => h.child.try_wait().map(|s| s.is_some()).unwrap_or(true),
        };
        if dead {
            *guard = Some(spawn_host()?);
        }
        let host = guard
            .as_mut()
            .ok_or_else(|| "Office host not spawned after restart".to_string())?;

        let payload = format!("{}\n", config);
        host.stdin
            .write_all(payload.as_bytes())
            .map_err(|e| format!("Office host stdin: {e}"))?;
        host.stdin
            .flush()
            .map_err(|e| format!("Office host flush: {e}"))?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
        let line = wait_for_host_response(&host.shared, deadline)?;
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            return Ok(v);
        }
        // Unparseable response → the host is wedged; restart once. Kill the
        // old process explicitly — dropping a Child on Windows does NOT
        // terminate the powershell, so a wedged host would otherwise leak a
        // process that keeps locking the document.
        if let Some(mut old) = guard.take() {
            let _ = old.child.kill();
            let _ = old.child.wait();
        }
    }
    Err("Office host returned an unparseable response twice".into())
}

/// Run the one-shot COM bridge and return the parsed JSON result.
pub fn run_bridge(config: &Value) -> AppResult<Value> {
    let script_path = std::env::temp_dir().join(format!(
        "office_com_{}.ps1",
        crate::core::ids::generate_id()
    ));
    {
        let mut f = std::fs::File::create(&script_path)?;
        f.write_all(BRIDGE_SCRIPT.as_bytes())?;
    }

    let args_json = config.to_string();

    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .arg("-ArgsJson")
        .arg(&args_json)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to launch PowerShell: {e}"))?;

    // Timeout: `.output()` would wait forever on a hung bridge. Kill and
    // report once the deadline passes instead of locking the tool.
    const BRIDGE_TIMEOUT_SECS: u64 = 120;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(BRIDGE_TIMEOUT_SECS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&script_path);
                    return Err(format!(
                        "PowerShell COM bridge timed out after {BRIDGE_TIMEOUT_SECS}s"
                    )
                    .into());
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&script_path);
                return Err(format!("Failed to wait for PowerShell: {e}").into());
            }
        }
    };

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        std::io::Read::read_to_end(&mut s, &mut stdout_buf).ok();
    }
    if let Some(mut s) = child.stderr.take() {
        std::io::Read::read_to_end(&mut s, &mut stderr_buf).ok();
    }
    let _ = std::fs::remove_file(&script_path);

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_buf);
        let stdout = String::from_utf8_lossy(&stdout_buf);
        if let Some(parsed) = parse_json_line(&stdout) {
            return Ok(parsed);
        }
        return Err(format!(
            "PowerShell COM bridge failed (exit {}): {}",
            status.code().unwrap_or(-1),
            if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            }
        )
        .into());
    }

    let stdout = String::from_utf8_lossy(&stdout_buf);
    parse_json_line(&stdout).ok_or_else(|| {
        format!(
            "COM bridge returned unparseable output: {}",
            truncate(&stdout, 300)
        )
        .into()
    })
}

/// Find the first JSON object line in the script output.
pub fn parse_json_line(stdout: &str) -> Option<Value> {
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                return Some(v);
            }
        }
    }
    None
}

pub fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().take(max).collect();
    let mut out: String = chars.into_iter().collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// Bridge result error helper.
pub fn bridge_error(v: &Value) -> Option<String> {
    v.get("error").and_then(|e| e.as_str()).map(str::to_string)
}

/// Bridge failure with the NO_OFFICE sentinel expanded into an actionable
/// hint (install WPS/Office + the file-level fallback tool for this action).
pub fn bridge_failure(result: &Value, action: &str, path: &Path) -> Option<String> {
    bridge_error(result).map(|err| {
        if err == "NO_OFFICE" {
            fallback_hint(action, path)
        } else {
            err
        }
    })
}

/// A human-readable fallback hint for an action when no office app is
/// installed — names the file-level tool that covers the same operation.
pub fn fallback_hint(action: &str, path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let file_tool = match ext.as_str() {
        "xls" | "xlsx" | "et" | "csv" => "table_process",
        _ => match action {
            "read" => "doc_read",
            "replace" | "insert" | "delete" | "set_style" | "set_font" => "docx_edit",
            "replace_all" => "docx_edit",
            _ => "doc_read / docx_edit / table_process",
        },
    };
    format!(
        "No office application detected on this machine (probed WPS: KWPS/KET/KWPP, \
         MS Office: Word/Excel/PowerPoint). Install WPS Office (free, https://www.wps.cn/) \
         or Microsoft Office to edit documents through the app itself — including .wps/.et/.dps \
         files and documents already open in the office app.\n\n\
         Until then, use the file-level tool \"{file_tool}\" for this operation — it works \
         without any office install."
    )
}

/// Resolve a path against the workspace, creating the parent dir when
/// needed (used for save_as / export_pdf outputs).
pub fn resolve_output(context: &ToolContext, raw: &str) -> AppResult<std::path::PathBuf> {
    let out = crate::tools::builtin::resolve_path(context.workspace.as_deref(), raw);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    Ok(out)
}

/// Shared save_as / export_pdf arm for all three app families: validates
/// the output extension against the family's format map, points the host
/// at the source document + output path and returns the tool result.
pub fn save_or_export(
    action: &str,
    args: &Value,
    config: &mut Value,
    path: Option<&std::path::PathBuf>,
    family: &str,
    context: &ToolContext,
    display_target: &str,
) -> AppResult<ToolResult> {
    let out_str = args
        .get("output_path")
        .and_then(|p| p.as_str())
        .ok_or_else(|| "Missing required parameter: output_path".to_string())?;
    let out = resolve_output(context, out_str)?;
    if action == "save_as" {
        let ext = out
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let format = save_format_for(family, &ext).ok_or_else(|| {
            let ok = match family {
                "calc" => "xlsx/xls/csv",
                "impress" => "pptx/ppt",
                _ => "docx/doc/pdf/rtf/txt/html/xps/odt",
            };
            format!("Unsupported save format: .{ext} ({family}: {ok})")
        })?;
        if let Some(src) = path {
            config["source_path"] = json!(src.to_string_lossy());
        }
        config["path"] = json!(out.to_string_lossy());
        config["format"] = json!(format);
    } else {
        if let Some(src) = path {
            config["source_path"] = json!(src.to_string_lossy());
        }
        config["path"] = json!(out.to_string_lossy());
    }
    let result = host_call(config)?;
    if let Some(err) = bridge_failure(&result, action, path.unwrap_or(&std::path::PathBuf::new())) {
        return Ok(ToolResult::error(err));
    }
    Ok(ToolResult::success(format!(
        "{}: {} → {}",
        action,
        display_target,
        out.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_json_line_from_script_output() {
        let out = "some noise\n{\"action\":\"read\",\"paragraphs\":3}\nmore noise";
        let v = parse_json_line(out).expect("parsed");
        assert_eq!(v["action"], "read");
        assert_eq!(v["paragraphs"], 3);
    }

    #[test]
    fn bridge_error_extraction() {
        let v = json!({"error": "No WPS or Microsoft Word COM available"});
        assert!(bridge_error(&v).is_some());
        let ok = json!({"action": "detect", "ok": true});
        assert!(bridge_error(&ok).is_none());
    }

    #[test]
    fn truncate_limits_output() {
        let long = "x".repeat(1000);
        let out = truncate(&long, 100);
        assert!(out.chars().count() <= 101);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn no_office_hint_names_the_fallback_tool() {
        let hint = fallback_hint("read", Path::new("r.docx"));
        assert!(hint.contains("doc_read"));
        assert!(hint.contains("wps.cn"));
        let edit_hint = fallback_hint("replace", Path::new("r.docx"));
        assert!(edit_hint.contains("docx_edit"));
        let calc_hint = fallback_hint("read_cells", Path::new("s.xlsx"));
        assert!(calc_hint.contains("table_process"));
    }

    #[test]
    fn bridge_failure_expands_no_office_sentinel() {
        let v = json!({ "error": "NO_OFFICE" });
        let hint = bridge_failure(&v, "read", Path::new("r.docx")).expect("hint");
        assert!(hint.contains("wps.cn"));
        let v2 = json!({ "error": "file locked by another process" });
        let err = bridge_failure(&v2, "read", Path::new("r.docx")).expect("err");
        assert_eq!(err, "file locked by another process");
    }

    #[test]
    #[ignore = "requires a real WPS/Word install; opens a visible window"]
    fn host_live_write_window_sync() {
        let dir = std::env::temp_dir().join("ddc_host_live");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("live.docx");
        let _ = std::fs::remove_file(&p);
        let p = p.to_string_lossy().to_string();

        let r1 = host_call(&json!({
            "action": "type_text", "app": "wps", "path": p,
            "text": "第一部分：宿主进程第一次写入的内容。",
            "pace": 150
        }))
        .expect("call 1");
        assert!(r1.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
        // Hold the host alive so the user can watch the live typing.
        std::thread::sleep(std::time::Duration::from_secs(10));

        let r2 = host_call(&json!({
            "action": "type_text", "app": "wps", "path": p,
            "text": "第二部分：第二次调用在同一窗口继续追加。",
            "pace": 150
        }))
        .expect("call 2");
        assert!(r2.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
        // Keep the host (and window content) alive for observation.
        std::thread::sleep(std::time::Duration::from_secs(10));

        let r3 =
            run_bridge(&json!({ "action": "read", "app": "wps", "path": p })).expect("read back");
        let text = r3.get("text").and_then(|v| v.as_str()).unwrap_or("");
        assert!(text.contains("第一部分"), "part 1 missing: {text}");
        assert!(text.contains("第二部分"), "part 2 missing: {text}");
    }
}
