//! Background task registry — tracks detached processes spawned by the
//! `bash` tool's background mode so `kill_task` can terminate them and
//! `wait_tasks` can poll their output.
//!
//! Each background task writes stdout/stderr to a log file; consumers read
//! it incrementally via [`BackgroundTaskRegistry::read_output`] using an
//! offset they keep themselves (stateless across tool calls).

use crate::core::error::AppResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A chunk of a background task's output read at an offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutputChunk {
    /// New content since `offset`.
    pub content: String,
    /// Byte offset to resume from on the next read.
    pub offset: u64,
    /// Whether the task has finished (callers may stop polling).
    pub done: bool,
}

/// A background task tracked by the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub command: String,
    pub pid: u32,
    pub started_at_ms: u64,
    pub status: String,
    pub session_id: String,
    /// Path to the task's output log file (stdout/stderr).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    /// Exit code once the process finished (None while running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl BackgroundTask {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

/// Registry of background tasks, shared between the `bash` tool
/// (spawn side), the `kill_task` tool (kill side), and the `wait_tasks`
/// tool (output polling side).
pub struct BackgroundTaskRegistry {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
}

impl BackgroundTaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a background process with an optional output log file.
    pub fn register_with_output(
        &self,
        command: &str,
        pid: u32,
        session_id: &str,
        output_file: Option<String>,
    ) -> String {
        let id = format!(
            "bg-{}-{}",
            pid,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let started_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let task = BackgroundTask {
            id: id.clone(),
            command: command.to_string(),
            pid,
            started_at_ms: started_ms,
            status: "running".to_string(),
            session_id: session_id.to_string(),
            output_file,
            exit_code: None,
        };
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), task);
        id
    }

    /// Get a task snapshot by ID.
    pub fn get(&self, id: &str) -> Option<BackgroundTask> {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// Mark a task as naturally finished. Exit code 0 → `completed`,
    /// non-zero → `failed`. Killed tasks keep their `killed` status.
    pub fn mark_exited(&self, id: &str, exit_code: Option<i32>) {
        let mut guard = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = guard.get_mut(id) {
            if task.status == "killed" {
                return;
            }
            task.exit_code = exit_code;
            task.status = if exit_code == Some(0) {
                "completed"
            } else {
                "failed"
            }
            .to_string();
        }
    }

    /// Terminate a background task by ID. Returns `true` when the process
    /// existed and was killed, `false` when the ID was not found.
    pub fn kill(&self, id: &str) -> AppResult<bool> {
        let task = {
            let mut guard = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
            match guard.get_mut(id) {
                Some(t) => t.clone(),
                None => return Ok(false),
            }
        };

        let killed = kill_process(task.pid, task.started_at_ms);
        let mut guard = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = guard.get_mut(id) {
            t.status = if killed { "killed" } else { "exited" }.to_string();
        }
        Ok(killed)
    }

    /// Read new output for a task starting at `offset` (bytes).
    ///
    /// Returns the new content, the new offset, and whether the task has
    /// finished (so callers can stop polling). `None` when the task ID is
    /// unknown or it has no output file.
    pub fn read_output(&self, id: &str, offset: u64, max_bytes: usize) -> Option<TaskOutputChunk> {
        let task = self.get(id)?;
        let path = task.output_file.as_ref()?;
        let mut file = std::fs::File::open(path).ok()?;
        file.seek(SeekFrom::Start(offset)).ok()?;

        let mut buf = vec![0u8; max_bytes.max(1)];
        let read = file.read(&mut buf).unwrap_or(0);
        buf.truncate(read);

        // Hold back an incomplete trailing multi-byte sequence (UTF-8/GBK):
        // decoding a partial char standalone silently corrupts CJK text. The
        // next read starts at the backed-up offset and re-reads these bytes
        // together with the following ones, so no char is ever split across
        // two decoded chunks.
        let hold = crate::core::encoding::incomplete_trailing_bytes(&buf);
        let new_offset = offset + (read as u64 - hold as u64);
        let content = crate::core::encoding::decode_native_output(&buf[..read.saturating_sub(hold)]);
        Some(TaskOutputChunk {
            content,
            offset: new_offset,
            done: !task.is_running(),
        })
    }

    /// List all tasks for a session (running ones first).
    pub fn list(&self, session_id: &str) -> Vec<BackgroundTask> {
        let guard = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let mut tasks: Vec<BackgroundTask> = guard
            .values()
            .filter(|t| t.session_id == session_id)
            .cloned()
            .collect();
        tasks.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        tasks
    }
}

impl Default for BackgroundTaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Tolerance (ms) for the PID-reuse guard: process start times are reported
/// in whole seconds, so a small skew between the recorded spawn time and the
/// queried start time is expected. Anything larger means the PID was
/// reassigned to an unrelated process.
const PID_REUSE_TOLERANCE_MS: u64 = 3000;

/// Kill a process tree by PID, cross-platform, with a PID-reuse guard.
///
/// The tracked process was spawned at `started_at_ms` (epoch ms). If the PID
/// now belongs to a different, later process, nothing is killed — the
/// original is already gone and killing would hit an innocent process.
/// Returns whether the tracked process is gone/terminated.
fn kill_process(pid: u32, started_at_ms: u64) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::everything(),
    );

    let Some(process) = system.process(Pid::from_u32(pid)) else {
        return true; // already gone — treat as killed
    };

    // PID-reuse guard: sysinfo reports start time in seconds since boot;
    // (now − started_at) is the same boot-relative scale, so a mismatch
    // beyond tolerance means the PID was reassigned.
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let start_secs = process.start_time();
    let expected_boot_ms = now_ms.saturating_sub(started_at_ms);
    if start_secs.saturating_mul(1000).abs_diff(expected_boot_ms) > PID_REUSE_TOLERANCE_MS {
        tracing::warn!(
            pid,
            start_secs,
            expected_boot_ms,
            "PID reused since task start — not killing"
        );
        return true; // PID reused — the tracked process is gone
    }

    #[cfg(windows)]
    {
        // taskkill /T /F terminates the WHOLE process tree (grandchildren
        // included) — a plain single-process kill would leave them orphaned.
        let mut cmd = std::process::Command::new("taskkill");
        crate::core::proc::no_window(&mut cmd);
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        match cmd.status() {
            Ok(status) => status.success(),
            Err(e) => {
                tracing::warn!(error = %e, "taskkill failed — falling back to direct kill");
                process.kill_with(Signal::Kill).unwrap_or(false)
            }
        }
    }
    #[cfg(not(windows))]
    {
        // Collect the full descendant set, then SIGKILL each (children first
        // so re-parenting cannot resurface them under a live parent).
        system.refresh_processes(ProcessesToUpdate::All, true);
        let parent_of: std::collections::HashMap<Pid, Pid> = system
            .processes()
            .iter()
            .filter_map(|(p, proc)| proc.parent().map(|par| (*p, par)))
            .collect();
        let mut ordered: Vec<Pid> = Vec::new();
        let mut stack = vec![Pid::from_u32(pid)];
        let mut seen = std::collections::HashSet::new();
        while let Some(p) = stack.pop() {
            if !seen.insert(p) {
                continue;
            }
            ordered.push(p);
            for (child, par) in &parent_of {
                if *par == p {
                    stack.push(*child);
                }
            }
        }
        ordered.reverse();
        let mut ok = true;
        for p in ordered {
            if let Some(proc) = system.process(p) {
                if !proc.kill_with(Signal::Kill).unwrap_or(false) && p != Pid::from_u32(pid) {
                    ok = false;
                }
            }
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn kill_unknown_id_returns_false() {
        let registry = BackgroundTaskRegistry::new();
        let killed = registry.kill("nope").unwrap();
        assert!(!killed);
    }

    #[test]
    fn read_output_is_incremental() {
        let registry = BackgroundTaskRegistry::new();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("task.log");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello world").unwrap();
        }
        let id = registry.register_with_output(
            "cmd",
            111,
            "s1",
            Some(path.to_string_lossy().into_owned()),
        );

        let chunk = registry.read_output(&id, 0, 5).unwrap();
        assert_eq!(chunk.content, "hello");
        assert_eq!(chunk.offset, 5);
        assert!(!chunk.done);

        let chunk2 = registry.read_output(&id, chunk.offset, 100).unwrap();
        assert_eq!(chunk2.content, " world");
        assert_eq!(chunk2.offset, 11);

        // Past EOF → empty chunk, offset unchanged.
        let chunk3 = registry.read_output(&id, chunk2.offset, 100).unwrap();
        assert_eq!(chunk3.content, "");
        assert_eq!(chunk3.offset, 11);
    }

    #[test]
    fn read_output_holds_back_incomplete_cjk_char() {
        // "中文" is E4 B8 AD E6 96 87 (6 UTF-8 bytes). Reading 5 bytes cuts
        // the second char mid-sequence; the reader must HOLD the incomplete
        // tail (E6 96) and re-read it with the next chunk — never decode a
        // partial char as a wrong GBK character.
        let registry = BackgroundTaskRegistry::new();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("task.log");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all("中文".as_bytes()).unwrap();
        }
        let id = registry.register_with_output(
            "cmd",
            222,
            "s1",
            Some(path.to_string_lossy().into_owned()),
        );

        // First read (5 bytes) ends at E6 96 — mid-way through 文. The chunk
        // must contain only the COMPLETE 中 and back the offset up by 2.
        let chunk1 = registry.read_output(&id, 0, 5).unwrap();
        assert_eq!(chunk1.content, "中", "partial char must be held back");
        assert_eq!(chunk1.offset, 3, "offset backed up past the held bytes");

        // Second read re-reads the held bytes + the rest → 文.
        let chunk2 = registry.read_output(&id, chunk1.offset, 100).unwrap();
        assert_eq!(chunk2.content, "文");
        assert_eq!(chunk2.offset, 6);

        // A clean boundary (even split between the two 3-byte chars) needs
        // no hold.
        let chunk3 = registry.read_output(&id, 0, 3).unwrap();
        assert_eq!(chunk3.content, "中");
        assert_eq!(chunk3.offset, 3);
    }
}
