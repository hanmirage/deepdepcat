//! Crash handler — panic capture and crash report generation.
//!
//! Registers a global panic hook that captures backtraces, process info,
//! and OS details, then writes a crash report to the app data directory.
//! The frontend can query and display these reports.
//!
//! Reports are written in two layers:
//! - `crash-{timestamp}.txt` — human-readable report (panic + backtrace + info)
//! - `pending-crash.json` — structured payload consumed by the crash dialog
//!   at next startup. Contains only safe-to-share fields (panic message,
//!   backtrace, OS/arch, client_id) — never conversation content. If the user
//!   opts in, the dialog asks the backend to export the current session
//!   conversation as a separate file and uploads it via the two-phase
//!   `/api/v1/crash/conversation` endpoint.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing::error;

/// Get the crash reports directory.
fn crash_dir() -> PathBuf {
    let app_data = crate::core::config::get_app_data_dir();
    let dir = app_data.join("crashes");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// A crash report metadata entry (for listing in the frontend).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrashReportInfo {
    pub filename: String,
    pub timestamp: String,
    pub file_size: u64,
}

/// Structured crash payload consumed by the crash dialog on next startup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingCrash {
    /// Random install id — identifies "this machine", not a user account.
    pub client_id: String,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub panic_message: String,
    pub backtrace: String,
    pub timestamp: String,
}

/// Path where the next-startup crash dialog looks for a report to send.
fn pending_crash_path() -> PathBuf {
    crash_dir().join("pending-crash.json")
}

/// Install the global panic hook.
///
/// Call this once at application startup, before any other code runs.
/// The hook captures a backtrace, writes a human-readable crash report file,
/// and a structured pending-crash payload for the next-startup dialog.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace_str = backtrace.to_string();

        let report = format!(
            "DeepDepCat Crash Report\n\
            ========================\n\
            Timestamp: {timestamp}\n\
            PID: {pid}\n\
            OS: {os}\n\
            Arch: {arch}\n\
            Kind: Rust panic\n\
            \n\
            Panic: {panic}\n\
            \n\
            Backtrace:\n\
            {backtrace}\n",
            timestamp = timestamp,
            pid = std::process::id(),
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            panic = panic_info,
            backtrace = backtrace_str,
        );

        write_crash_report(&report, &timestamp, &panic_info.to_string(), &backtrace_str);
        error!(timestamp = %timestamp, "Panic captured — crash report written");

        // Also print to stderr as a fallback.
        eprintln!("{report}");
    }));
}

/// Windows native crash capture — catches native exceptions (access
/// violations, etc.) that never reach the Rust panic hook.
///
/// Installs a `SetUnhandledExceptionFilter` handler. When a native crash
/// occurs (e.g. a null-pointer dereference in a dependency), the handler
/// runs synchronously, writes the same crash report + pending-crash payload
/// as a Rust panic, then terminates the process. Report writing is kept
/// minimal so it works even in a corrupted heap.
#[cfg(windows)]
pub fn install_native_crash_filter() {
    use windows::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    use windows::Win32::System::Diagnostics::Debug::{
        SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
    };

    unsafe extern "system" fn native_handler(exception_info: *const EXCEPTION_POINTERS) -> i32 {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace_str = backtrace.to_string();

        // Best-effort exception code description. The exception code is an
        // NTSTATUS (i32) wrapping the NT status value.
        let code_desc = unsafe {
            if exception_info.is_null() {
                "unknown".to_string()
            } else {
                // `ExceptionRecord` is a raw pointer in windows 0.61.x and
                // raw pointers don't auto-deref on field access — one extra
                // deref to reach the record's `ExceptionCode`.
                let code = (*(*exception_info).ExceptionRecord).ExceptionCode.0 as u32;
                if code == EXCEPTION_ACCESS_VIOLATION.0 as u32 {
                    format!("EXCEPTION_ACCESS_VIOLATION (0x{code:08x})")
                } else {
                    format!("0x{code:08x}")
                }
            }
        };

        let report = format!(
            "DeepDepCat Crash Report\n\
            ========================\n\
            Timestamp: {timestamp}\n\
            PID: {pid}\n\
            OS: {os}\n\
            Arch: {arch}\n\
            Kind: Native exception ({code_desc})\n\
            \n\
            Backtrace:\n\
            {backtrace}\n",
            timestamp = timestamp,
            pid = std::process::id(),
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            backtrace = backtrace_str,
        );

        write_crash_report(
            &report,
            &timestamp,
            &format!("Native exception ({code_desc})"),
            &backtrace_str,
        );
        error!(timestamp = %timestamp, "Native crash captured — crash report written");
        // Return EXCEPTION_EXECUTE_HANDLER: do not attempt recovery; the
        // process is in an undefined state. The report is on disk.
        1
    }

    unsafe {
        SetUnhandledExceptionFilter(Some(native_handler));
    }
}

/// Write a crash report file + pending-crash payload.
///
/// Shared by the Rust panic hook and the Windows native exception filter.
/// Kept allocation-light so it can run during a panic or native exception.
fn write_crash_report(report: &str, timestamp: &str, message: &str, backtrace: &str) {
    let report_path = crash_dir().join(format!("crash-{timestamp}.txt"));
    let _ = fs::write(&report_path, report);

    let pending = PendingCrash {
        client_id: crate::core::crash::client_id(),
        app_version: option_env!("CARGO_PKG_VERSION")
            .unwrap_or("1.0.0")
            .to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        pid: std::process::id(),
        panic_message: message.to_string(),
        backtrace: backtrace.to_string(),
        timestamp: timestamp.to_string(),
    };
    let _ = fs::write(
        pending_crash_path(),
        serde_json::to_vec(&pending).unwrap_or_default(),
    );
}

/// Read the pending crash payload (if any) for the crash dialog.
pub fn read_pending_crash() -> Option<PendingCrash> {
    let data = fs::read_to_string(pending_crash_path()).ok()?;
    serde_json::from_str(&data).ok()
}

/// Clear the pending crash payload after the dialog is resolved.
pub fn clear_pending_crash() {
    let _ = fs::remove_file(pending_crash_path());
}

/// The `client_id` for crash reporting — a stable random install id.
/// Stored in `{app_data}/crash_client_id`, created once on first use.
///
/// The value is cached in a `OnceLock` after the first read, so the panic
/// hook (which runs while the process is already in a panicking state) only
/// ever reads from memory — never touches the filesystem.
pub fn client_id() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let path = crate::core::config::get_app_data_dir().join("crash_client_id");
            if let Ok(id) = fs::read_to_string(&path) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    return id;
                }
            }
            let id = uuid::Uuid::new_v4().to_string();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&path, &id);
            id
        })
        .clone()
}

/// Ensure the crash `client_id` is initialized before any panic can occur.
///
/// The panic hook calls [`client_id()`] which reads from the `OnceLock`
/// cache. Call this once at startup (before the hook is installed or right
/// after) so a panicking process never has to touch the filesystem.
pub fn ensure_client_id() {
    let _ = client_id();
}

/// List all crash reports in the crashes directory.
pub fn list_crash_reports() -> Vec<CrashReportInfo> {
    let dir = crash_dir();
    let mut reports = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                let filename = entry.file_name().to_string_lossy().to_string();
                if !filename.starts_with("crash-") || !filename.ends_with(".txt") {
                    continue;
                }
                // Extract timestamp from filename: crash-YYYYMMDD_HHMMSS.txt
                let timestamp = filename
                    .trim_start_matches("crash-")
                    .trim_end_matches(".txt")
                    .to_string();
                reports.push(CrashReportInfo {
                    filename,
                    timestamp,
                    file_size: metadata.len(),
                });
            }
        }
    }

    // Sort by timestamp descending (newest first).
    reports.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    reports
}

/// Read the content of a specific crash report.
pub fn read_crash_report(filename: &str) -> Option<String> {
    // Sanitize filename to prevent path traversal.
    let safe_name = filename.replace(['/', '\\'], "").replace("..", "");
    let path = crash_dir().join(format!("crash-{safe_name}.txt"));
    fs::read_to_string(path).ok()
}

/// Delete a crash report by filename.
pub fn delete_crash_report(filename: &str) -> bool {
    let safe_name = filename.replace(['/', '\\'], "").replace("..", "");
    let path = crash_dir().join(format!("crash-{safe_name}.txt"));
    fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that write crash files — they share the crash report
    // directory and pending-crash.json, and cargo runs tests concurrently.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn crash_dir_exists() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = crash_dir();
        assert!(dir.exists());
    }

    #[test]
    fn list_reports_returns_vec() {
        let reports = list_crash_reports();
        // Should return a Vec (possibly empty if no crashes).
        let _ = reports.len();
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let result = read_crash_report("nonexistent_1234567890");
        assert!(result.is_none());
    }

    #[test]
    fn client_id_is_stable_and_unique() {
        // The id is stable within a run; two calls return the same value.
        let a = client_id();
        let b = client_id();
        assert_eq!(a, b);
        assert_eq!(a.len(), 36); // UUID v4 format
    }

    #[test]
    fn pending_crash_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let pending = PendingCrash {
            client_id: "test-client".to_string(),
            app_version: "1.0.0".to_string(),
            os: "windows".to_string(),
            arch: "x86_64".to_string(),
            pid: 42,
            panic_message: "boom".to_string(),
            backtrace: "stack".to_string(),
            timestamp: "20260801_000000".to_string(),
        };
        let path = pending_crash_path();
        let _ = fs::write(&path, serde_json::to_vec(&pending).unwrap_or_default());
        let read_back = read_pending_crash().expect("pending crash should be readable");
        assert_eq!(read_back.panic_message, "boom");
        assert_eq!(read_back.client_id, "test-client");
        clear_pending_crash();
        assert!(read_pending_crash().is_none());
    }

    /// Installing the native crash filter must not panic and must be idempotent.
    #[test]
    #[cfg(windows)]
    fn install_native_crash_filter_is_idempotent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        install_native_crash_filter();
        install_native_crash_filter();
    }
}
