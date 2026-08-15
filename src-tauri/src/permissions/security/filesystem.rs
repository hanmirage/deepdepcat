//! Filesystem validation — protects against dangerous file operations.
//!
//! Checks for:
//! - Path traversal (../../etc/passwd)
//! - Symlink attacks
//! - Dangerous system paths (~/.ssh, /etc, etc.)
//! - Dotfile access restrictions

use std::path::{Path, PathBuf};

/// The result of a filesystem path validation.
#[derive(Debug, Clone)]
pub enum ValidationResult {
    Allow,
    Deny(String),
    Ask,
}

/// Validates file paths before tool execution.
#[derive(Clone)]
pub struct FilesystemValidator {
    /// Paths that are always denied.
    denied_paths: Vec<PathBuf>,
    /// Paths that require explicit permission.
    ask_paths: Vec<PathBuf>,
}

impl Default for FilesystemValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesystemValidator {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

        let denied_paths = vec![
            home.join(".ssh"),
            home.join(".env"),
            PathBuf::from("/etc/shadow"),
            PathBuf::from("/etc/passwd"),
            PathBuf::from("/etc/sudoers"),
            // Windows
            PathBuf::from("C:\\Windows\\System32\\config"),
        ];

        let ask_paths = vec![
            home.join(".config"),
            home.join(".gitconfig"),
            home.join(".bashrc"),
            home.join(".zshrc"),
            home.join(".profile"),
        ];

        Self {
            denied_paths,
            ask_paths,
        }
    }

    /// Validate a file path.
    pub fn validate(&self, path: &str) -> ValidationResult {
        let path_buf = PathBuf::from(path);

        // Check for path traversal
        if self.contains_traversal(&path_buf) {
            return ValidationResult::Deny("Path traversal detected".to_string());
        }

        // Canonicalize the path (follow symlinks). For a path that does not
        // exist yet (write operations), canonicalize the NEAREST EXISTING
        // parent and re-append the tail — otherwise a symlinked parent
        // directory (ws/.ssh-link/id_rsa → ~/.ssh/id_rsa) slips past the
        // deny check: the leaf doesn't exist so canonicalize fails, and the
        // raw path doesn't match ~/.ssh (#88 audit H9).
        let canonical = match path_buf.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                let mut ancestor = path_buf.as_path();
                let mut tail: Vec<std::path::Component> = Vec::new();
                // Walk up until an existing ancestor resolves.
                loop {
                    match ancestor.canonicalize() {
                        Ok(resolved) => {
                            let mut full = resolved;
                            for comp in tail.iter().rev() {
                                full.push(comp.as_os_str());
                            }
                            break full;
                        }
                        Err(_) => {
                            let Some(parent) = ancestor.parent() else {
                                break path_buf.clone();
                            };
                            let name = ancestor.file_name();
                            if let Some(name) = name {
                                tail.push(std::path::Component::Normal(name));
                            }
                            ancestor = parent;
                        }
                    }
                }
            }
        };

        // Check denied paths
        for denied in &self.denied_paths {
            if Self::path_starts_with(&canonical, denied)
                || Self::path_starts_with(&path_buf, denied)
            {
                return ValidationResult::Deny(format!(
                    "Access to this path is denied: {}",
                    denied.display()
                ));
            }
        }

        // Check ask paths
        for ask in &self.ask_paths {
            if Self::path_starts_with(&canonical, ask) || Self::path_starts_with(&path_buf, ask) {
                return ValidationResult::Ask;
            }
        }

        // Check for symlinks
        if let Ok(metadata) = std::fs::symlink_metadata(&path_buf) {
            if metadata.file_type().is_symlink() {
                return ValidationResult::Ask;
            }
        }

        ValidationResult::Allow
    }

    /// Whether `path` is `base` or lives under it.
    ///
    /// On Windows the comparison is CASE-INSENSITIVE per component: Rust's
    /// `Path::starts_with` compares OsStr bytes case-sensitively, so
    /// `C:\Users\X\.SSH\newkey` could otherwise bypass the `~/.ssh` deny
    /// for a not-yet-existing leaf (canonicalize keeps the caller's case
    /// on the appended tail).
    fn path_starts_with(path: &Path, base: &Path) -> bool {
        #[cfg(windows)]
        {
            let lower = |p: &Path| -> Vec<String> {
                p.components()
                    .filter_map(|c| match c {
                        std::path::Component::Prefix(prefix) => {
                            Some(prefix.as_os_str().to_string_lossy().to_lowercase())
                        }
                        std::path::Component::Normal(s) => {
                            Some(s.to_string_lossy().to_lowercase())
                        }
                        _ => None,
                    })
                    .collect()
            };
            let p = lower(path);
            let b = lower(base);
            p.len() >= b.len() && p[..b.len()] == b[..]
        }
        #[cfg(not(windows))]
        {
            path.starts_with(base)
        }
    }

    /// Check if a path contains directory traversal patterns.
    fn contains_traversal(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // Check for .. in the path
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                return true;
            }
        }

        // Also check for encoded traversal
        if path_str.contains("%2e%2e") || path_str.contains("..%2f") || path_str.contains("..%5c") {
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_paths_reject_direct_and_symlinked_ancestors() {
        // Regression for #88 audit H9: writing THROUGH a symlinked parent
        // (workspace/.ssh-link/id_rsa → ~/.ssh/id_rsa) used to slip past the
        // deny check because the leaf doesn't exist and canonicalize failed
        // on the raw path. The validator must resolve the nearest existing
        // ancestor and re-append the tail before comparing.
        let tmp = std::env::temp_dir().join(format!(
            "ddc-fs-validator-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let validator = FilesystemValidator::new();

        // Real ~/.ssh path (may or may not exist) is always denied.
        let home = dirs::home_dir().unwrap();
        let direct = home.join(".ssh").join("id_rsa");
        assert!(
            matches!(
                validator.validate(&direct.to_string_lossy()),
                ValidationResult::Deny(_)
            ),
            "direct ~/.ssh path must be denied"
        );

        // Symlinked parent → ~/.ssh: leaf doesn't exist, but the parent
        // resolves into the deny zone.
        #[cfg(unix)]
        {
            let link = tmp.join("ssh-link");
            let _ = std::fs::remove_dir_all(&link);
            std::os::unix::fs::symlink(home.join(".ssh"), &link).unwrap();
            let through = link.join("id_rsa");
            assert!(
                matches!(
                    validator.validate(&through.to_string_lossy()),
                    ValidationResult::Deny(_)
                ),
                "write through a symlinked parent into ~/.ssh must be denied"
            );
        }

        // A normal path under temp is allowed.
        let fine = tmp.join("normal").join("file.txt");
        assert!(
            matches!(
                validator.validate(&fine.to_string_lossy()),
                ValidationResult::Allow
            ),
            "ordinary workspace path must be allowed"
        );
    }

    #[cfg(windows)]
    #[test]
    fn deny_paths_match_case_insensitively_on_windows() {
        // Rust's Path::starts_with is byte-sensitive on Windows; an
        // uppercased `.SSH` leaf must still hit the `~/.ssh` deny zone.
        let validator = FilesystemValidator::new();
        let home = dirs::home_dir().unwrap();
        let upper = home.join(".SSH").join("new_key");
        assert!(
            matches!(
                validator.validate(&upper.to_string_lossy()),
                ValidationResult::Deny(_)
            ),
            "uppercased .SSH must be denied"
        );
    }
}
