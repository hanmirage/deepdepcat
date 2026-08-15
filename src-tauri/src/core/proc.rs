//! Process spawning helpers.
//!
//! On Windows, spawning a console application (pwsh, cmd, git, …) via
//! `std::process::Command` / `tokio::process::Command` opens a visible CMD
//! window unless the `CREATE_NO_WINDOW` creation flag is set. All internal
//! process launches must go through these helpers so the Tauri app never
//! flashes console windows during startup or tool execution.

#[cfg(windows)]
mod win;

/// Windows Job Object for process-tree isolation. On non-Windows platforms
/// this is an empty placeholder so call sites can use a uniform signature.
#[cfg(windows)]
pub use win::job::JobObject;

/// Placeholder type on non-Windows — no process-tree isolation is applied.
#[cfg(not(windows))]
#[derive(Debug)]
pub struct JobObject;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// `CREATE_NO_WINDOW` — the child runs without a console window.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Suppress the console window for a synchronous [`std::process::Command`].
#[cfg(windows)]
pub fn no_window(cmd: &mut std::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Suppress the console window for a synchronous [`std::process::Command`].
#[cfg(not(windows))]
pub fn no_window(_cmd: &mut std::process::Command) {}

/// Suppress the console window for an async [`tokio::process::Command`].
#[cfg(windows)]
pub fn no_window_tokio(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Suppress the console window for an async [`tokio::process::Command`].
#[cfg(not(windows))]
pub fn no_window_tokio(_cmd: &mut tokio::process::Command) {}

/// Attach a tokio child to a Job Object for process-tree isolation.
///
/// No-op on non-Windows platforms or when no job is provided. Assignment is
/// best-effort — failure logs and leaves the direct-child kill as the only
/// guarantee (the whole tree still gets the timeout kill when it works).
#[cfg(windows)]
pub fn attach_job(child: &tokio::process::Child, job: &JobObject) {
    if let Err(e) = job.assign_child(child) {
        tracing::warn!(error = %e, "Job Object attach failed — running without process-tree isolation");
    }
}

/// Attach a tokio child to a Job Object (no-op on non-Windows).
#[cfg(not(windows))]
pub fn attach_job(_child: &tokio::process::Child, _job: &JobObject) {}

/// Kill a child and its entire process tree.
///
/// Windows: terminates the Job Object (kills the whole tree). Non-Windows:
/// relies on the caller's process-group setup — the provided job is unused.
#[cfg(windows)]
pub fn kill_tree(child: &mut tokio::process::Child, job: Option<&JobObject>) {
    if let Some(job) = job {
        if job.terminate().is_ok() {
            // Tree terminated — the direct child handle is reclaimed by wait().
            return;
        }
        tracing::warn!("Job Object terminate failed — falling back to direct kill");
    }
    let _ = child.start_kill();
}

/// Kill a child and its entire process tree (Unix: kill the child directly;
/// the sandbox executor sets a process group so `kill` reaches descendants).
#[cfg(not(windows))]
pub fn kill_tree(child: &mut tokio::process::Child, _job: Option<&JobObject>) {
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_window_is_noop_on_sync_command() {
        let mut cmd = std::process::Command::new("echo");
        no_window(&mut cmd);
        // Must not panic. On Windows, setting the flag via creation_flags
        // succeeds; the exact bit value is verified by compile-time typing.
        let _ = &cmd;
    }

    #[test]
    fn no_window_tokio_is_noop() {
        let mut cmd = tokio::process::Command::new("echo");
        no_window_tokio(&mut cmd);
        let _ = &cmd;
    }
}
