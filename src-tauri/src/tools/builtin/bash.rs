//! Bash tool — executes shell commands with timeout and streaming output.
//!
//! Uses `spawn()` + async read to stream stdout/stderr to the frontend
//! in real-time via `StreamEvent::ToolCallProgress`. Each chunk is run
//! through `stream_chunk()` for UTF-8-safe slicing.
//!
//! Supports `background: true` — the command runs detached, its PID is
//! registered in the shared [`BackgroundTaskRegistry`], and `kill_task`
//! can terminate it later.

use crate::agent::streaming::stream_chunk;
use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::core::stream::emit_stream;
use crate::core::types::StreamEvent;
use crate::tools::background::BackgroundTaskRegistry;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::sandbox::executor::SandboxExecutor;

/// The shell kind chosen for this platform — drives both how commands are
/// executed and the tool description the model sees (Unix-trained models need
/// to know whether they are talking to bash, PowerShell, or cmd).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellKind {
    /// PowerShell 7 (pwsh) — best PowerShell compatibility.
    Pwsh,
    /// Git Bash — a real Unix bash; the model's native syntax runs unchanged.
    GitBash,
    /// Windows PowerShell 5.1 (always present) — `ls`/`cat` aliases + pipelines.
    Powershell51,
    /// cmd.exe — last resort on Windows.
    Cmd,
    /// Unix (Linux/macOS) — native bash.
    UnixBash,
}

static SHELL_KIND: std::sync::OnceLock<ShellKind> = std::sync::OnceLock::new();

/// The shell kind detected at startup (defaults to UnixBash before detection).
pub(crate) fn shell_kind() -> ShellKind {
    *SHELL_KIND.get().unwrap_or(&ShellKind::UnixBash)
}

/// Whether a shell executable exists and runs — used for the Windows
/// fallback tiers between pwsh and cmd (Git Bash, PowerShell 5.1).
fn shell_runs(name: &str, args: &[&str]) -> bool {
    let mut cmd = std::process::Command::new(name);
    crate::core::proc::no_window(&mut cmd);
    cmd.args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub struct BashTool {
    timeout_secs: u64,
    max_output_chars: usize,
    shell: String,
    shell_flags: &'static [&'static str],
    background_tasks: Arc<BackgroundTaskRegistry>,
    /// Optional sandbox executor — drives process-tree isolation (Windows
    /// Job Object) for foreground commands whose profile is active.
    sandbox: Option<Arc<std::sync::RwLock<SandboxExecutor>>>,
}

/// Detect the best available shell on this platform.
///
/// Returns the shell name and the flag arguments that must be passed as
/// SEPARATE argv elements (never a space-joined single string — `Command::arg`
/// does not split on spaces, so `"-NoProfile -Command"` would reach pwsh as
/// one bogus parameter).
///
/// Windows order: Git Bash → PowerShell 7 → PowerShell 5.1 → cmd. Git Bash
/// FIRST because the model is trained on Unix bash — a real bash runs its
/// native syntax (grep, find, sed, `&&`) unchanged, which pwsh only
/// partially aliases. Git for Windows installs add `Git\cmd` (git.exe + bash
/// wrappers) to PATH but OFTEN NOT `Git\bin` where bash.exe lives, so the
/// common install dirs are probed before giving up on bash.
pub(crate) fn detect_shell() -> (String, &'static [&'static str]) {
    #[cfg(windows)]
    {
        // Tier 1: Git Bash — a real Unix bash; the model's native syntax runs
        // unchanged. Probe PATH first, then the standard Git-for-Windows
        // install dirs (bash.exe is rarely on PATH there).
        for candidate in [
            "bash.exe",
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ] {
            if shell_runs(candidate, &["--version"]) {
                tracing::info!("Git Bash detected ({candidate}) — using bash as shell");
                let _ = SHELL_KIND.set(ShellKind::GitBash);
                return (candidate.to_string(), &["-c"]);
            }
        }
        // Tier 2: PowerShell 7 (pwsh).
        let mut probe = std::process::Command::new("pwsh.exe");
        crate::core::proc::no_window(&mut probe);
        let result = probe
            .arg("-NoProfile")
            .arg("-Command")
            .arg("$PSVersionTable.PSVersion.Major")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        match result {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                tracing::info!("PowerShell {} detected — using pwsh as shell", version);
                let _ = SHELL_KIND.set(ShellKind::Pwsh);
                ("pwsh.exe".to_string(), &["-NoProfile", "-Command"])
            }
            _ => {
                // Tier 3: Windows PowerShell 5.1 (always present) — `ls`/`cat`
                // aliases + pipelines, still far better than cmd.
                if shell_runs(
                    "powershell.exe",
                    &["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"],
                ) {
                    tracing::info!(
                        "Windows PowerShell 5.1 detected — using powershell.exe as shell"
                    );
                    let _ = SHELL_KIND.set(ShellKind::Powershell51);
                    ("powershell.exe".to_string(), &["-NoProfile", "-Command"])
                }
                // Tier 4: cmd — last resort.
                else {
                    tracing::warn!(
                        "no usable PowerShell or Git Bash — falling back to cmd. \
                         Install Git for Windows from https://git-scm.com for \
                         full agent command compatibility."
                    );
                    let _ = SHELL_KIND.set(ShellKind::Cmd);
                    ("cmd".to_string(), &["/C"])
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = SHELL_KIND.set(ShellKind::UnixBash);
        ("bash".to_string(), &["-c"])
    }
}

impl BashTool {
    /// Create a bash tool that registers background tasks in the shared
    /// registry (so `kill_task` can terminate them), optionally carrying a
    /// sandbox executor for process-tree isolation.
    pub fn with_background_registry(
        timeout_secs: u64,
        max_output_chars: usize,
        background_tasks: Arc<BackgroundTaskRegistry>,
        sandbox: Option<Arc<RwLock<SandboxExecutor>>>,
    ) -> Self {
        let (shell_name, shell_flags) = detect_shell();
        Self {
            timeout_secs,
            max_output_chars,
            shell: shell_name,
            shell_flags,
            background_tasks,
            sandbox,
        }
    }

    /// Spawn the configured shell with the given command as a script.
    fn spawn_shell(&self, command: &str) -> Command {
        let mut cmd = Command::new(&self.shell);
        crate::core::proc::no_window_tokio(&mut cmd);
        // Strip sensitive environment variables from the child's inherited
        // env — a command like `env` or `printenv` must not leak harness
        // credentials (*KEY*/*SECRET*/*TOKEN*/*PASSWORD*) into tool output.
        cmd.envs(Self::sanitized_shell_env());
        cmd.args(self.shell_flags).arg(command);
        cmd
    }

    /// The process environment minus sensitive variables (keys naming
    /// *KEY*/*SECRET*/*TOKEN*/*PASSWORD*). `cmd.envs(...)` REPLACES the
    /// inherited env, so this returns the full filtered set — the session id
    /// is re-set explicitly by callers afterward.
    fn sanitized_shell_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        std::env::vars_os()
            .filter(|(key, _)| {
                let name = key.to_string_lossy().to_uppercase();
                !(name.contains("KEY")
                    || name.contains("SECRET")
                    || name.contains("TOKEN")
                    || name.contains("PASSWORD"))
            })
            .collect()
    }

    /// Execute a command detached in the background.
    ///
    /// The child process is spawned detached, its PID is registered in the
    /// shared registry, and a task ID is returned immediately. stdout/stderr
    /// are appended to a per-task log file so `wait_tasks` can poll output
    /// incrementally. A watcher task marks the task completed/failed when
    /// the process exits and fires the TaskCompleted/TaskUpdated hooks.
    async fn execute_background(
        &self,
        command: &str,
        context: &ToolContext,
    ) -> AppResult<ToolResult> {
        let mut cmd = self.spawn_shell(command);
        if let Some(ref cwd) = context.workspace {
            cmd.current_dir(cwd);
        }
        cmd.kill_on_drop(false);
        #[cfg(windows)]
        {
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            // Must OR with CREATE_NO_WINDOW — assignment would drop the
            // no-window flag set by no_window_tokio and spawn a visible CMD.
            cmd.creation_flags(crate::core::proc::CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }

        // Route output to a per-task log file (append mode) so the task
        // can be polled by wait_tasks after this call returns.
        let log_path = std::env::temp_dir().join(format!(
            "ddc-task-{}.log",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| {
                AppError::Internal(format!("Failed to create background task log: {}", e))
            })?;
        let stderr_file = log_file.try_clone().map_err(|e| {
            AppError::Internal(format!("Failed to clone background task log: {}", e))
        })?;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(stderr_file));

        let mut child = cmd.spawn().map_err(|e| {
            AppError::Internal(format!("Failed to spawn background command: {}", e))
        })?;

        let pid = child.id().unwrap_or_default();
        let task_id = self.background_tasks.register_with_output(
            command,
            pid,
            &context.session_id,
            Some(log_path.to_string_lossy().into_owned()),
        );

        // Watch the child to completion: keep the Child handle alive (so the
        // process survives), then mark the task finished and fire hooks.
        let registry = self.background_tasks.clone();
        let app = context.app.clone();
        let session_id = context.session_id.clone();
        let command_owned = command.to_string();
        let task_id_owned = task_id.clone();
        tokio::spawn(async move {
            let exit_code = child.wait().await.ok().and_then(|s| s.code());
            registry.mark_exited(&task_id_owned, exit_code);

            // Auto-wake signal: emit a task-completed event so the frontend
            // can surface the completion (and offer to continue) even while
            // the agent is idle.
            let _ = app.emit(
                "task-completed",
                serde_json::json!({
                    "task_id": task_id_owned.clone(),
                    "session_id": session_id.clone(),
                    "command": command_owned.clone(),
                    "exit_code": exit_code,
                    "status": if exit_code == Some(0) { "completed" } else { "failed" },
                }),
            );

            // Fire the TaskCompleted (exit 0) / TaskUpdated (non-zero)
            // observe hooks so external tooling can react to task state.
            use crate::hooks::{HookContext, HookEvent};
            let event = if exit_code == Some(0) {
                HookEvent::TaskCompleted
            } else {
                HookEvent::TaskUpdated
            };
            let ctx = HookContext::new(event, session_id.clone())
                .with_data("task_id", serde_json::json!(task_id_owned))
                .with_data("command", serde_json::json!(command_owned))
                .with_data("exit_code", serde_json::json!(exit_code))
                .with_data(
                    "status",
                    serde_json::json!(registry.get(&task_id_owned).map(|t| t.status)),
                );
            // AppState is always managed when tools execute — safe to borrow.
            let executor = crate::hooks::HookExecutor::new(
                app.state::<crate::bootstrap::AppState>().hooks.clone(),
            );
            executor.execute_observe(&ctx).await;
            // Push a monitor event so the monitor tool can observe task
            // lifecycle end-to-end. Bucketed under the session that started
            // the task.
            let state = app.state::<crate::bootstrap::AppState>();
            state.monitor_events.push(
                &session_id,
                crate::tools::builtin::monitor::MonitoredEvent {
                    event_type: "task".to_string(),
                    payload: serde_json::json!({
                        "id": task_id_owned,
                        "command": command_owned,
                        "status": if exit_code == Some(0) { "completed" } else { "failed" },
                    }),
                    timestamp_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                },
            );
            tracing::info!(
                task_id = %task_id_owned,
                exit_code = ?exit_code,
                "Background task finished"
            );
        });

        Ok(ToolResult::success(format!(
            "Background task started.\n\
             Task ID: {}\n\
             PID: {}\n\
             Command: {}\n\
             Use kill_task(task_id=\"{}\") to stop it, or \
             wait_tasks(task_id=\"{}\") to poll its output.",
            task_id, pid, command, task_id, task_id
        )))
    }
}

#[async_trait]
impl Tool for BashTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        match shell_kind() {
            ShellKind::Pwsh => {
                "Execute a shell command in PowerShell 7. Use PowerShell syntax with `&&`, `||`, and `;` for pipeline chains. Commands run with -NoProfile for clean execution. NEVER download reference/temporary material into the user's workspace: write such files to the system temp directory ($env:TEMP) and clean them up. Use the grep/read_file/web_fetch tools instead of shell text extraction whenever possible. NOTE: Git Bash (Unix bash) is NOT installed here, so native Unix syntax (git, grep, find, sed, `&&`) is limited — if a command fails because a Unix tool is missing, tell the user plainly: 没有 git 有点难办，推荐安装 Git for Windows: https://git-scm.com — do not silently work around it or claim success."
            }
            ShellKind::GitBash => {
                "Execute a shell command in Git Bash (Unix bash on Windows). Use standard bash syntax for pipelines, redirections, and subcommands — `ls`, `cat`, `grep`, pipes, and `&&` all work. NEVER download reference/temporary material into the user's workspace: write such files to the system temp directory and clean them up."
            }
            ShellKind::Powershell51 => {
                "Execute a shell command in Windows PowerShell 5.1. `ls`/`cat`/`cp`/`rm` aliases work, as do pipelines with `|`. NOTE: the `&&`/`||` operators are NOT supported in PowerShell 5.1 (PowerShell 7+ only) — chain with `;` instead. NEVER download reference/temporary material into the user's workspace: write such files to the system temp directory ($env:TEMP) and clean them up. NOTE: Git Bash (Unix bash) is NOT installed and you are on PowerShell 5.1 — Unix syntax (git, grep, find, sed) is limited and `&&`/`||` are unsupported. If a command fails because the shell lacks a capability, tell the user plainly what to install: PowerShell 7 (https://aka.ms/powershell) for a stronger shell, or Git for Windows (https://git-scm.com) for full bash compatibility — do not silently work around it or claim success."
            }
            ShellKind::Cmd => {
                "Execute a shell command in cmd.exe. IMPORTANT: Use cmd syntax (e.g. `dir` instead of `ls`, `type` instead of `cat`). Prefer PowerShell-style `&&` and `||` for pipeline chains — these work in cmd too. NEVER download reference/temporary material into the user's workspace: write such files to the system temp directory and clean them up. NOTE: cmd is the most limited shell and Git Bash is NOT installed. If a command fails because the shell lacks a capability, tell the user plainly what to install: PowerShell 7 (https://aka.ms/powershell) or Git for Windows (https://git-scm.com) — do not silently work around it or claim success."
            }
            ShellKind::UnixBash => {
                "Execute a shell command in bash. Use bash syntax for pipelines, redirections, and subcommands. NEVER download reference/temporary material into the user's workspace: write such files to /tmp and clean them up."
            }
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "cwd": { "type": "string", "description": "Working directory. Defaults to the workspace root." },
                "timeout": { "type": "integer", "description": "Timeout in seconds. Defaults to 120. Long builds/tests/installs often need more — pass e.g. 600 explicitly for long-running work." },
                "background": { "type": "boolean", "description": "Run the command detached in the background and return immediately with a task ID (use kill_task to stop it). Defaults to false." }
            },
            "required": ["command"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let command = args
            .get("command")
            .and_then(|c| c.as_str())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'command'".into()))?;

        let background = args
            .get("background")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);

        if background {
            return self.execute_background(command, context).await;
        }

        let cwd = args
            .get("cwd")
            .and_then(|c| c.as_str())
            .map(std::path::PathBuf::from)
            .or_else(|| context.workspace.clone());

        let timeout = args
            .get("timeout")
            .and_then(|t| t.as_u64())
            .unwrap_or(self.timeout_secs);

        let mut cmd = self.spawn_shell(command);
        if let Some(ref cwd) = cwd {
            cmd.current_dir(cwd);
        }
        // deepseek-native: legacy behavior (0.1.0) did not expose the session
        // environment variable to child processes — honor the pinned version.
        if context.behavior_version == crate::toolkit::ToolBehaviorVersion::Current {
            cmd.env("DEEPDEPCAT_SESSION", &context.session_id);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        // When the sandbox profile is active, create a Windows Job Object so
        // the whole process tree (not just the shell) dies on timeout. For
        // the maximum-isolation profiles (Strict/ReadOnly) the job also
        // carries the restricted-token security filter — every assigned
        // process runs without admin SIDs and without SeDebugPrivilege.
        let mut job = None;
        if let Some(sandbox) = &self.sandbox {
            if let Ok(executor) = sandbox.read() {
                let profile = executor.profile();
                if profile.is_active() {
                    let restricted = matches!(
                        profile,
                        crate::sandbox::executor::SandboxProfile::Strict
                            | crate::sandbox::executor::SandboxProfile::ReadOnly
                    );
                    #[cfg(windows)]
                    {
                        let create = if restricted {
                            crate::core::proc::JobObject::create_restricted
                        } else {
                            crate::core::proc::JobObject::create
                        };
                        match create() {
                            Ok(j) => job = Some(j),
                            Err(e) => {
                                tracing::warn!(error = %e, "Job Object creation failed — running without tree isolation");
                            }
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = &executor; // no-op on non-Windows
                    }
                }
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| AppError::Internal(format!("Failed to spawn command: {}", e)))?;

        // Attach the spawned process to the job (best-effort — a failure just
        // means the direct-child kill is the only cleanup).
        #[cfg(windows)]
        if let Some(ref j) = job {
            crate::core::proc::attach_job(&child, j);
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Internal("stdout pipe not available".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::Internal("stderr pipe not available".into()))?;

        let turn_id = context.turn_id.clone();
        let app = context.app.clone();

        // Spawn reader tasks — each reads lines and emits progress. The
        // events carry the REAL tool call id so the frontend's
        // `event.call_id === tool.id` matching can route the deltas onto
        // the running tool card.
        let call_id = context.call_id.clone();
        // Readers accumulate into shared CAPPED sinks — the capture below is
        // bounded even if a grandchild keeps the pipe open (see the join
        // timeout after `child.wait`), so the buffer cannot grow forever
        // while a detached descendant holds the handles.
        let stdout_sink = Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_sink = Arc::new(std::sync::Mutex::new(String::new()));
        let sink_cap = self.max_output_chars.max(8192);
        let stdout_task = tokio::spawn(stream_reader(
            BufReader::new(stdout),
            app.clone(),
            turn_id.clone(),
            call_id.clone(),
            stdout_sink.clone(),
            sink_cap,
        ));
        let stderr_task = tokio::spawn(stream_reader(
            BufReader::new(stderr),
            app,
            turn_id.clone(),
            call_id,
            stderr_sink.clone(),
            sink_cap,
        ));

        // Wait for process with timeout
        let exit_status = tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await;

        // When the wall-clock timeout fired, kill the tree BEFORE draining
        // output — the readers only reach EOF once the process tree is
        // actually dead, so killing first makes the capture below return
        // promptly with the full partial output.
        if exit_status.is_err() {
            crate::core::proc::kill_tree(&mut child, job.as_ref());
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        }

        // Capture is BOUNDED even after the process exits: a grandchild that
        // inherited the stdout/stderr handles keeps the pipe open, and the
        // reader tasks would otherwise never EOF — hanging the tool call
        // until the 600s dispatcher timeout. The readers write into shared
        // capped sinks, so on timeout we return whatever streamed so far
        // (the frontend already saw the live progress events).
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            futures_util::future::join(stdout_task, stderr_task),
        )
        .await;
        let stdout_str = stdout_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let stderr_str = stderr_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let exit_code = match exit_status {
            Ok(Ok(status)) => {
                // Orthogonal reporting: a process killed by a signal has NO
                // exit code (`code()` is None). Folding that into -1 makes
                // the model read "exit code -1" and retry as if it were an
                // ordinary failure, when the real story is "killed by
                // signal". Report the signal explicitly instead.
                #[cfg(unix)]
                if let Some(sig) = status.signal() {
                    let mut msg = format!("Process terminated by signal {sig}.");
                    if !stdout_str.is_empty() {
                        msg.push_str(&format!("\n{stdout_str}"));
                    }
                    if !stderr_str.is_empty() {
                        msg.push_str(&format!("\nSTDERR:\n{stderr_str}"));
                    }
                    return Ok(ToolResult::error(msg));
                }
                status.code().unwrap_or(-1)
            }
            Ok(Err(e)) => {
                return Ok(ToolResult::error(format!(
                    "Failed to wait for process: {}",
                    e
                )));
            }
            Err(_) => {
                return Ok(ToolResult::error(render_timeout_feedback(
                    timeout,
                    &stdout_str,
                    &stderr_str,
                )));
            }
        };

        // Build final result
        let mut result = String::new();
        if !stdout_str.is_empty() {
            result.push_str(&stdout_str);
        }
        if !stderr_str.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&format!("STDERR:\n{}", stderr_str));
        }
        if result.is_empty() {
            result = format!("Command completed with exit code: {}", exit_code);
        } else {
            result = format!("{}\n\nExit code: {}", result.trim_end(), exit_code);
        }

        // Truncate if too long — at a char boundary so multi-byte UTF-8
        // (CJK output) never panics mid-character.
        if result.len() > self.max_output_chars {
            let shown =
                crate::core::str_util::truncate_at_char_boundary(&result, self.max_output_chars);
            result = format!(
                "{}\n\n...(output truncated, showing {} of {} chars)",
                shown,
                self.max_output_chars,
                result.len()
            );
        }

        if exit_code != 0 {
            Ok(ToolResult::error(result))
        } else {
            Ok(ToolResult::success(result))
        }
    }
}

/// Read lines from a process output stream, emit each as a
/// `StreamEvent::ToolCallProgress` event, and accumulate the text into a
/// shared CAPPED sink.
///
/// Uses `stream_chunk()` for UTF-8-safe slicing — incomplete multi-byte
/// sequences at the buffer boundary are held back and reassembled on the
/// next read.
async fn stream_reader<R: AsyncBufReadExt + Unpin>(
    mut reader: R,
    app: tauri::AppHandle,
    turn_id: String,
    call_id: String,
    sink: Arc<std::sync::Mutex<String>>,
    sink_cap: usize,
) {
    let mut buf = Vec::with_capacity(8192);
    let mut total_bytes: u64 = 0;
    let mut last_total: u64 = 0;
    let mut tail = Vec::new();

    loop {
        buf.clear();
        let n = match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        total_bytes += n as u64;

        // Decode this line with native encoding detection (strict UTF-8
        // first, GBK fallback for cmd/legacy console output), then feed
        // clean UTF-8 bytes to the chunker so every streamed delta is
        // valid UTF-8 (GBK bytes would otherwise be replaced with U+FFFD).
        let decoded = crate::core::encoding::decode_native_output(&buf[..n]);
        tail.extend_from_slice(decoded.as_bytes());

        // UTF-8-safe chunking
        while let Some(progress) = stream_chunk(None, &tail, total_bytes, &mut last_total, false) {
            // Extract delta from the progress payload
            let delta = match &progress {
                crate::toolkit::ToolProgress::Custom { payload, .. } => payload
                    .get("delta")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                crate::toolkit::ToolProgress::PartialResult { delta, .. } => {
                    delta.clone()
                }
            };

            // Emit to frontend — real call_id so the running tool card
            // receives the live deltas.
            emit_stream(
                &app,
                StreamEvent::ToolCallProgress {
                    turn_id: turn_id.clone(),
                    call_id: call_id.clone(),
                    name: "bash".into(),
                    kind: "partial_result".into(),
                    delta: Some(delta.clone()),
                    total_bytes: Some(total_bytes),
                },
            );
        }

        // Trim tail to only uncomsumed bytes
        let consumed = last_total as usize;
        if consumed > 0 && consumed <= tail.len() {
            tail = tail[consumed..].to_vec();
            last_total = 0;
        } else if consumed >= tail.len() {
            tail.clear();
            last_total = 0;
        }

        let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
        push_capped(&mut guard, &decoded, sink_cap);
    }
}

/// Append `text` to the sink while keeping it under `cap` bytes — the head
/// is preserved (the caller truncates from the head anyway) and the buffer
/// stays bounded even when a detached descendant keeps the pipe open and
/// the reader runs indefinitely.
fn push_capped(sink: &mut String, text: &str, cap: usize) {
    let room = cap.saturating_sub(sink.len());
    if room == 0 {
        return;
    }
    let take = room.min(text.len());
    sink.push_str(&text[..take]);
}

/// Max characters of captured partial output carried into a timeout error.
const TIMEOUT_PARTIAL_OUTPUT_CHARS: usize = 2000;

/// Render the timeout failure feedback handed back to the model.
///
/// The command was killed at `timeout_secs` — the model must see BOTH what
/// progress was made (captured stdout/stderr) AND how to continue, or a
/// long build/test loses its work and the model blindly retries the same
/// doomed call. Pure so the exact contract is unit-testable.
fn render_timeout_feedback(timeout_secs: u64, stdout: &str, stderr: &str) -> String {
    let mut out = format!(
        "Command timed out after {} seconds — the process tree was killed. \
         The command was still running and produced no exit code.",
        timeout_secs
    );

    let mut partial = String::new();
    if !stdout.is_empty() {
        partial.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !partial.is_empty() {
            partial.push('\n');
        }
        partial.push_str(&format!("STDERR:\n{}", stderr));
    }
    if !partial.is_empty() {
        let truncated = crate::core::str_util::truncate_at_char_boundary(
            &partial,
            TIMEOUT_PARTIAL_OUTPUT_CHARS,
        );
        out.push_str(&format!(
            "\n\nPartial output before timeout:\n{}",
            truncated
        ));
        if partial.len() > TIMEOUT_PARTIAL_OUTPUT_CHARS {
            out.push_str(&format!(
                "\n… ({} more characters of output)",
                partial.len() - TIMEOUT_PARTIAL_OUTPUT_CHARS
            ));
        }
    }

    out.push_str(
        "\n\nTo continue: 1) rerun with a larger \"timeout\" (seconds — long \
         builds/tests often need 600+), 2) split the work into smaller \
         commands, or 3) run long tasks with \"background\": true and poll \
         them. Do not blindly retry with the same timeout.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shell flags must be separate argv elements — `Command::arg` does not
    /// split on spaces, so a single "-NoProfile -Command" string would reach
    /// pwsh as one bogus parameter and break every command.
    #[test]
    fn shell_flags_are_separate_argv_elements() {
        let (shell, flags) = detect_shell();
        assert!(!shell.is_empty());
        assert!(!flags.is_empty());
        assert!(flags.iter().all(|f| !f.contains(' ')));
    }

    #[test]
    fn timeout_feedback_keeps_partial_output_and_guidance() {
        let msg = render_timeout_feedback(120, "Compiling foo...\n50% done", "warning: x");
        assert!(msg.contains("timed out after 120 seconds"));
        assert!(msg.contains("process tree was killed"));
        assert!(msg.contains("Compiling foo..."));
        assert!(msg.contains("50% done"));
        assert!(msg.contains("STDERR:"));
        assert!(msg.contains("warning: x"));
        // Actionable next steps — the model must not blindly retry.
        assert!(msg.contains("larger \"timeout\""));
        assert!(msg.contains("background"));
    }

    #[test]
    fn timeout_feedback_without_output_stays_concise() {
        let msg = render_timeout_feedback(30, "", "");
        assert!(msg.contains("timed out after 30 seconds"));
        assert!(!msg.contains("Partial output"));
        assert!(msg.contains("To continue"));
    }

    #[test]
    fn timeout_feedback_truncates_oversized_output() {
        let big = "x".repeat(TIMEOUT_PARTIAL_OUTPUT_CHARS + 500);
        let msg = render_timeout_feedback(60, &big, "");
        assert!(msg.contains("500 more characters"));
        assert!(!msg.contains("x".repeat(TIMEOUT_PARTIAL_OUTPUT_CHARS + 100).as_str()));
    }

    #[test]
    fn push_capped_keeps_head_and_bounds_size() {
        let mut sink = String::new();
        push_capped(&mut sink, "hello world", 1000);
        assert_eq!(sink, "hello world");

        // Once the cap is reached, further output is dropped entirely — the
        // head (which the result truncation shows) stays intact and the
        // buffer never grows while a detached descendant holds the pipe.
        push_capped(&mut sink, " more", 11);
        assert!(sink.len() <= 11);
        assert!(sink.starts_with("hello world"));
        let cap = sink.len();
        let before = cap;
        push_capped(&mut sink, "xxxx", cap);
        assert_eq!(sink.len(), before, "no room → nothing appended");
    }
}
