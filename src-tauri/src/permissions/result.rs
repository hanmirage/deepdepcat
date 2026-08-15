//! The permission decision type — the single outcome of a check.

/// The result of a permission check.
#[derive(Debug, Clone)]
pub enum PermissionResult {
    Allow,
    Deny(String),
    Ask,
}
