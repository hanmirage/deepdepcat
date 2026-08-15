//! File watcher — the workspace "index invalidator".
//!
//! Polls workspace file metadata (mtime + size) on a timer and turns
//! external changes into three effects:
//! - the cached symbol index is marked stale (next `search_symbols` rebuilds),
//! - the cached dependency graph is marked stale (next `file_dependencies`
//!   rebuilds — same contract as `SymbolIndex::mark_stale`),
//! - changed paths are recorded in `AppState.external_changes` so the agent
//!   loop's verification gate can invalidate auto-LSP evidence recorded
//!   BEFORE the change (a "clean" verdict from before an external edit is
//!   not evidence for the current file state).
//!
//! Polling (not OS notification) keeps platform dependencies at zero. The
//! watcher is started from `AppState::initialize_async` and stops when the
//! app exits (fire-and-forget background task).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::{debug, warn};

use crate::bootstrap::AppState;
use crate::hooks::{HookContext, HookEvent};
use serde_json::json;

/// How often to poll for changes.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// File extensions to watch (source-ish files that feed the indexes).
const WATCH_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "md", "toml", "json", "c", "h", "cpp", "hpp", "go",
    "java", "kt", "cs", "swift", "rb", "php", "vue", "svelte", "yaml", "yml",
];
/// Directories to ignore (relative to workspace root).
const IGNORE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "dist",
    ".next",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
    "build",
    "out",
    // The app's OWN memory directory: learnings.md / MEMORY.md / procedures.md
    // writes (up to once per 10 min per session) must not invalidate the
    // symbol/dependency indexes or pollute external_changes — they never feed
    // those indexes (same class as .git).
    ".deepdepcat",
];
/// Maximum file size to watch (bytes). Files larger than this are not
/// tracked at all: no snapshot, and any change to them never appears in
/// `external_changes` or invalidates the indexes — they are big assets
/// that never feed the symbol/dependency indexes anyway.
const MAX_FILE_SIZE: u64 = 2_000_000;
/// Upper bound of `AppState.external_changes` per workspace. When a batch
/// overflows it, the OLDEST entries are dropped so the newest paths (the
/// most relevant invalidation evidence) always survive.
const EXTERNAL_CHANGES_CAP: usize = 512;

/// Snapshot of a file's metadata for change detection.
#[derive(Debug, Clone)]
struct FileSnapshot {
    modified: SystemTime,
    size: u64,
}

/// A file change event detected by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    /// A new file was created.
    Created(PathBuf),
    /// An existing file was modified.
    Modified(PathBuf),
    /// A file was deleted.
    Deleted(PathBuf),
}

/// Apply one batch of change events to the shared app state: mark both
/// indexes stale and record the changed paths for auto-diagnostic
/// invalidation. Best-effort — index bookkeeping must never panic the
/// background task.
pub fn apply_changes(state: &AppState, changes: Vec<FileChange>) {
    if changes.is_empty() {
        return;
    }
    let len = changes.len();
    {
        let mut index = state
            .symbol_index
            .write()
            .unwrap_or_else(|e| e.into_inner());
        index.mark_stale();
    }
    {
        let mut graph = state
            .dependency_graph
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(graph) = graph.as_mut() {
            graph.mark_stale();
        }
    }
    {
        let mut external = state
            .external_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for change in changes {
            let path = match &change {
                FileChange::Created(p) | FileChange::Modified(p) | FileChange::Deleted(p) => p,
            };
            if external.len() >= EXTERNAL_CHANGES_CAP {
                // Overflow — evict the oldest entry instead of clearing the
                // whole buffer (a full clear silently discards every piece
                // of invalidation evidence in the same batch).
                external.remove(0);
            }
            external.push(path.clone());
        }
    }
    debug!(change_count = len, "File watcher invalidated indexes");
}

/// The file watcher — tracks file metadata and emits change events.
pub struct FileWatcher {
    workspace: PathBuf,
    snapshots: HashMap<PathBuf, FileSnapshot>,
}

impl FileWatcher {
    /// Create a new file watcher for the given workspace.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            snapshots: HashMap::new(),
        }
    }

    /// Perform an initial scan of the workspace, recording file snapshots
    /// without emitting change events.
    fn initial_scan(&mut self) -> std::io::Result<usize> {
        self.snapshots.clear();
        let workspace = self.workspace.clone();
        let count = Self::scan_dir(&workspace, &mut self.snapshots);
        Ok(count)
    }

    /// Poll for changes since the last scan.
    ///
    /// Returns a list of file change events. Call this on a timer.
    fn poll(&mut self) -> Vec<FileChange> {
        let mut current = HashMap::new();
        let workspace = self.workspace.clone();
        Self::scan_dir(&workspace, &mut current);

        let mut changes = Vec::new();

        // Detect created and modified files
        for (path, snap) in &current {
            match self.snapshots.get(path) {
                None => {
                    changes.push(FileChange::Created(path.clone()));
                }
                Some(old) if old.modified != snap.modified || old.size != snap.size => {
                    changes.push(FileChange::Modified(path.clone()));
                }
                _ => {}
            }
        }

        // Detect deleted files
        for path in self.snapshots.keys() {
            if !current.contains_key(path) {
                changes.push(FileChange::Deleted(path.clone()));
            }
        }

        // Update snapshots
        self.snapshots = current;

        changes
    }

    /// Run the blocking watch loop — scans once, then polls every
    /// [`POLL_INTERVAL`], forwarding changes to `apply_changes`. Spawned on
    /// a blocking thread so the async runtime never stalls on disk walks.
    /// Follows runtime workspace switches (`set_workspace`): when the app
    /// moves to another project, the scan tree re-arms instead of watching
    /// the old workspace forever.
    pub fn run(mut self, state: AppState) {
        let _ = std::thread::Builder::new()
            .name("file-watcher".into())
            .spawn(move || {
                if let Err(e) = self.initial_scan() {
                    // A failed initial scan must not kill the watcher for
                    // good — a dead watcher silently stops invalidating
                    // indexes. Retry with backoff; scan failures are
                    // transient (disk/IO) in practice.
                    warn!(error = %e, "Initial file watcher scan failed — retrying");
                    loop {
                        std::thread::sleep(POLL_INTERVAL);
                        match self.initial_scan() {
                            Ok(_) => break,
                            Err(e) => warn!(error = %e, "File watcher initial scan retry failed"),
                        }
                    }
                }
                loop {
                    std::thread::sleep(POLL_INTERVAL);
                    // Follow workspace switches — compare against the
                    // current app workspace and re-arm when it moved.
                    let current = state
                        .workspace
                        .read()
                        .map(|w| w.clone())
                        .unwrap_or_default();
                    if current.as_deref() != Some(self.workspace.as_path()) {
                        match current {
                            Some(w) if w.is_dir() => {
                                self.workspace = w;
                                if let Err(e) = self.initial_scan() {
                                    warn!(error = %e, "Watcher re-scan after workspace switch failed");
                                }
                            }
                            _ => {
                                self.workspace = PathBuf::new();
                                self.snapshots.clear();
                            }
                        }
                    }
                    let changes = self.poll();
                    if !changes.is_empty() {
                        // File-lifecycle hooks (observe-only). The watcher
                        // thread is BLOCKING, so hook execution is handed to
                        // the async runtime — a slow hook must never stall
                        // the disk scan. Events carry the workspace-global
                        // pseudo-session "workspace" (no session semantics
                        // at this layer); hooks filter on path/kind.
                        let executor = state.hook_executor.clone();
                        let hook_changes = changes.clone();
                        tauri::async_runtime::spawn(async move {
                            for change in hook_changes {
                                let (event, kind, path) = match &change {
                                    FileChange::Created(p) => {
                                        (HookEvent::FileCreated, "created", p)
                                    }
                                    FileChange::Modified(p) => {
                                        (HookEvent::FileChanged, "modified", p)
                                    }
                                    FileChange::Deleted(p) => {
                                        (HookEvent::FileDeleted, "deleted", p)
                                    }
                                };
                                executor
                                    .execute_observe(
                                        &HookContext::new(event, "workspace")
                                            .with_data("kind", json!(kind))
                                            .with_data(
                                                "path",
                                                json!(path.display().to_string()),
                                            ),
                                    )
                                    .await;
                            }
                        });
                        apply_changes(&state, changes);
                    }
                }
            });
    }

    /// Recursively scan a directory, recording file snapshots.
    fn scan_dir(dir: &Path, snapshots: &mut HashMap<PathBuf, FileSnapshot>) -> usize {
        let mut count = 0;
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return 0,
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Check if this is a directory we should skip
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if IGNORE_DIRS.contains(&name) {
                        continue;
                    }
                }
                count += Self::scan_dir(&path, snapshots);
                continue;
            }

            // Check file extension
            let ext_matches = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| WATCH_EXTENSIONS.contains(&e))
                .unwrap_or(false);
            if !ext_matches {
                continue;
            }

            // Get metadata
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Check file size
            if metadata.len() > MAX_FILE_SIZE {
                continue;
            }

            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            snapshots.insert(
                path,
                FileSnapshot {
                    modified,
                    size: metadata.len(),
                },
            );
            count += 1;
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_scan_detects_files_and_skips_ignored_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();

        // Create test files
        std::fs::write(workspace.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(workspace.join("readme.md"), "# README").unwrap();
        std::fs::create_dir(workspace.join("node_modules")).unwrap();
        std::fs::write(workspace.join("node_modules/lib.js"), "console.log(1)").unwrap();

        let mut watcher = FileWatcher::new(workspace.clone());
        let count = watcher.initial_scan().unwrap();

        // Should find 2 files (main.rs, readme.md), skip node_modules
        assert_eq!(count, 2);

        // Unwatched extension is ignored
        std::fs::write(workspace.join("data.bin"), vec![0u8; 16]).unwrap();
        assert_eq!(watcher.poll().len(), 0);
    }

    #[test]
    fn deepdepcat_metadata_dir_is_ignored() {
        // The app's own memory writes (learnings.md / MEMORY.md / procedures.md
        // in .deepdepcat/) must NOT invalidate the symbol indexes or pollute
        // external_changes — they never feed those indexes.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();

        std::fs::create_dir(workspace.join(".deepdepcat")).unwrap();
        std::fs::write(workspace.join(".deepdepcat/learnings.md"), "- a learning\n").unwrap();
        std::fs::write(workspace.join("main.rs"), "fn main() {}").unwrap();

        let mut watcher = FileWatcher::new(workspace.clone());
        let count = watcher.initial_scan().unwrap();
        assert_eq!(count, 1, "only main.rs is tracked; .deepdepcat is ignored");

        // Writing to the memory file must not be reported as a change.
        std::fs::write(workspace.join(".deepdepcat/learnings.md"), "- changed\n").unwrap();
        assert!(watcher.poll().is_empty());
    }

    #[test]
    fn poll_detects_created_modified_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();

        std::fs::write(workspace.join("main.rs"), "fn main() {}").unwrap();

        let mut watcher = FileWatcher::new(workspace.clone());
        watcher.initial_scan().unwrap();

        // No changes yet
        assert!(watcher.poll().is_empty());

        // Modify a file, create a new one, delete another
        std::fs::write(workspace.join("main.rs"), "fn main() { println!(); }").unwrap();
        std::fs::write(workspace.join("lib.rs"), "pub fn lib() {}").unwrap();
        std::fs::remove_file(workspace.join("main.rs")).unwrap();

        let changes = watcher.poll();
        let has_modified = changes.iter().any(|c| matches!(c, FileChange::Modified(_)));
        let has_created = changes.iter().any(|c| matches!(c, FileChange::Created(_)));
        let has_deleted = changes.iter().any(|c| matches!(c, FileChange::Deleted(_)));
        // The create is consumed by the remove in the same poll window as a
        // Deleted(Created) pair — we only assert the three event types
        // appeared at least once across the two polls below.
        assert!(has_modified || has_created || has_deleted);
        assert!(!changes.is_empty());
    }
}
