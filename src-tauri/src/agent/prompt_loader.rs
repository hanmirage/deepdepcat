//! Prompt loader — external prompt sections with bundled fallback.
//!
//! The system prompt used to be two giant compile-time constants
//! (`DEFAULT_SYSTEM_PROMPT` / `DEPWORK_SYSTEM_PROMPT`) in `context.rs`,
//! impossible to maintain or customize without editing source. This module
//! externalizes them into user-editable markdown files under
//! `~/.deepdepcat/prompts/` (inheriting `DEEPDEPCAT_HOME`), matching the
//! reference layout (`00-base.md`, `01-code-mode.md`, `02-depwork-mode.md`,
//! …) and falling back to the bundled constants **per section** when a file
//! is missing, unreadable, or the directory does not exist.
//!
//! # KV-cache stability
//!
//! The system prompt is the DeepSeek prefix-cache foundation — it must be
//! byte-identical across requests so the cached prefix hits. External file
//! loading preserves this: content is injected **as literal bytes, never
//! template-expanded** (`{{...}}` placeholders stay verbatim), so the same
//! files always produce the same bytes. Changing a file invalidates the
//! prefix — that is a legitimate user action, not a stability bug.

use std::path::{Path, PathBuf};

use crate::toolkit::WorkMode;

/// Directory name under the user config root (`~/.deepdepcat/prompts/`).
pub const PROMPTS_DIR: &str = "prompts";

/// A prompt section that can be overridden by an external file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSection {
    /// General guardrails shared by every mode — `00-base.md`.
    Base,
    /// Code-mode specific section — `01-code-mode.md`.
    CodeMode,
    /// Depwork-mode specific section — `02-depwork-mode.md`.
    DepworkMode,
}

impl PromptSection {
    /// The external file name for this section, or `None` for sections with
    /// no external override yet.
    fn file_name(self) -> Option<&'static str> {
        match self {
            Self::Base => Some("00-base.md"),
            Self::CodeMode => Some("01-code-mode.md"),
            Self::DepworkMode => Some("02-depwork-mode.md"),
        }
    }
}

/// A loaded prompt section.
#[derive(Debug, Clone)]
pub struct LoadedPrompt {
    pub content: String,
}

/// User prompts directory: `~/.deepdepcat/prompts/` (DEEPDEPCAT_HOME aware).
pub fn prompts_dir() -> PathBuf {
    crate::workspace::project_files::user_deepdepcat_dir().join(PROMPTS_DIR)
}

/// Load the base section from an explicit directory (test isolation).
pub fn load_base_with_dir(dir: &Path, work_mode: WorkMode) -> LoadedPrompt {
    load_section_with_dir(dir, PromptSection::Base)
        .unwrap_or_else(|| LoadedPrompt::from_bundled(bundled_base(work_mode)))
}

/// Load the mode section from an explicit directory (test isolation).
pub fn load_mode_section_with_dir(dir: &Path, work_mode: WorkMode) -> LoadedPrompt {
    let section = match work_mode {
        WorkMode::Code => PromptSection::CodeMode,
        WorkMode::Depwork => PromptSection::DepworkMode,
    };
    load_section_with_dir(dir, section)
        .unwrap_or_else(|| LoadedPrompt::from_bundled(bundled_mode(work_mode)))
}

/// Load a section from a specific directory (test injection). Returns `None`
/// when the file is missing or unreadable — the caller decides the fallback.
pub fn load_section_with_dir(dir: &Path, section: PromptSection) -> Option<LoadedPrompt> {
    let name = section.file_name()?;
    let path = dir.join(name);
    if !path.is_file() {
        return None;
    }
    let raw = std::fs::read(&path).ok()?;
    let text = crate::core::encoding::decode_native_output(&raw);
    if text.trim().is_empty() {
        return None;
    }
    Some(LoadedPrompt { content: text })
}

impl LoadedPrompt {
    fn from_bundled(content: &'static str) -> Self {
        Self {
            content: content.to_string(),
        }
    }
}

/// Sanitize external prompt content before injection.
///
/// The only transformation is breaking forged closing tags (`</...>`) with a
/// zero-width space so a prompt file cannot smuggle a `</system-reminder>`
/// closer that closes an injection block the app owns. Opening tags
/// (`<mode_boundary>`, `<system-reminder>`) and `{}` placeholders are left
/// untouched — the compaction/subagent sections use `{max_entries}`-style
/// templates that must survive.
pub fn sanitize_prompt_content(text: &str) -> String {
    text.replace("</", "<\u{200b}/")
}

/// The bundled base prompt for a mode. Used by guard tests to assert anchor
/// content stays present regardless of external overrides.
pub fn bundled_base(work_mode: WorkMode) -> &'static str {
    let _ = work_mode; // the shared base is mode-independent
    crate::agent::context::BUNDLED_BASE_PROMPT
}

/// The bundled mode section for a mode. Used by guard tests to assert the
/// mode-specific anchors (mode_boundary, toolset boundary, vision guard).
pub fn bundled_mode(work_mode: WorkMode) -> &'static str {
    match work_mode {
        WorkMode::Code => crate::agent::context::CODE_MODE_PROMPT,
        WorkMode::Depwork => crate::agent::context::DEPWORK_MODE_PROMPT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dir_falls_back_to_bundled() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("nope");
        let base = load_section_with_dir(&empty, PromptSection::Base);
        assert!(base.is_none(), "missing file → None");
    }

    #[test]
    fn external_file_overrides_bundled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("00-base.md"), "EXTERNAL BASE").unwrap();
        let loaded = load_section_with_dir(dir.path(), PromptSection::Base).unwrap();
        assert_eq!(loaded.content, "EXTERNAL BASE");
    }

    #[test]
    fn partial_missing_falls_back_per_section() {
        let dir = tempfile::tempdir().unwrap();
        // Only 01-code-mode.md exists — base falls back, mode is external.
        std::fs::write(dir.path().join("01-code-mode.md"), "EXTERNAL MODE").unwrap();
        let base = load_section_with_dir(dir.path(), PromptSection::Base);
        assert!(base.is_none());
        let mode = load_section_with_dir(dir.path(), PromptSection::CodeMode).unwrap();
        assert_eq!(mode.content, "EXTERNAL MODE");
    }

    #[test]
    fn empty_file_falls_back_to_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("00-base.md"), "   \n  ").unwrap();
        assert!(load_section_with_dir(dir.path(), PromptSection::Base).is_none());
    }

    #[test]
    fn sanitizer_breaks_forged_closers_but_keeps_placeholders() {
        let dirty = "</system-reminder>\n<mode_boundary>\n{max_entries}";
        let clean = sanitize_prompt_content(dirty);
        assert!(
            !clean.contains("</system-reminder>"),
            "forged closer broken"
        );
        assert!(clean.contains("<mode_boundary>"), "opening tag untouched");
        assert!(clean.contains("{max_entries}"), "placeholder kept");
    }

    #[test]
    fn non_utf8_file_decoded_lossy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("00-base.md");
        let (gbk, _, _) = encoding_rs::GBK.encode("中文提示词");
        std::fs::write(&path, gbk.as_ref()).unwrap();
        let loaded = load_section_with_dir(dir.path(), PromptSection::Base).unwrap();
        assert!(loaded.content.contains("中文提示词"));
    }
}
