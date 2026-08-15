//! Work mode & tool scope — the product surface an agent runs under.
//!
//! Two product surfaces share one backend but are isolated where it
//! matters: Code (coding assistant) and Depwork (document automation).
//! `WorkMode` selects the active surface per chat request; `ToolScope`
//! declares which surface a tool belongs to. Registries are filtered by
//! mode at agent build time (see `ToolRegistry::for_mode`).

/// The product work mode for a chat request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkMode {
    /// Code mode — coding assistant (local, high privilege, no sandbox).
    #[default]
    Code,
    /// Depwork mode — document automation for knowledge workers.
    Depwork,
}

/// Tool availability scope relative to work modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolScope {
    /// Available in every work mode (default).
    #[default]
    All,
    /// Code mode only (bash, code editing, LSP, code intelligence).
    Code,
    /// Depwork mode only (future office/document tools).
    Depwork,
}

impl WorkMode {
    /// Parse a work mode from the frontend request string.
    ///
    /// Unknown or missing values default to `Code` — the primary surface
    /// and the historical behavior of the application. Case/whitespace
    /// insensitive ("DEPWORK " → Depwork).
    pub fn parse(raw: Option<&str>) -> Self {
        let normalized = raw.map(str::trim).map(str::to_ascii_lowercase);
        match normalized.as_deref() {
            Some("depwork") => WorkMode::Depwork,
            _ => WorkMode::Code,
        }
    }

    /// Whether this mode allows a tool with the given scope.
    pub fn allows(&self, scope: ToolScope) -> bool {
        match scope {
            ToolScope::All => true,
            ToolScope::Code => matches!(self, WorkMode::Code),
            ToolScope::Depwork => matches!(self, WorkMode::Depwork),
        }
    }

    /// Stable wire string for logging and frontend round-trips.
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkMode::Code => "code",
            WorkMode::Depwork => "depwork",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults_to_code() {
        assert_eq!(WorkMode::parse(None), WorkMode::Code);
        assert_eq!(WorkMode::parse(Some("")), WorkMode::Code);
        assert_eq!(WorkMode::parse(Some("unknown")), WorkMode::Code);
    }

    #[test]
    fn parse_depwork() {
        assert_eq!(WorkMode::parse(Some("depwork")), WorkMode::Depwork);
        assert_eq!(WorkMode::parse(Some("  depwork  ")), WorkMode::Depwork);
    }

    #[test]
    fn allows_scopes() {
        let code = WorkMode::Code;
        let depwork = WorkMode::Depwork;
        assert!(code.allows(ToolScope::All));
        assert!(code.allows(ToolScope::Code));
        assert!(!code.allows(ToolScope::Depwork));
        assert!(depwork.allows(ToolScope::All));
        assert!(!depwork.allows(ToolScope::Code));
        assert!(depwork.allows(ToolScope::Depwork));
    }
}
