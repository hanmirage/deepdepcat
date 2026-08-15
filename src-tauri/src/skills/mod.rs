//! Skills system — reusable agent instruction templates.
//!
//! 3-tier structure:
//! 1. **Bundled** — built-in skills shipped with the app
//! 2. **File-based** — user-defined skills in ~/.deepdepcat/skills/
//! 3. **MCP** — skills exposed by MCP servers
//!
//! Skills can have:
//! - A system prompt that replaces or augments the default
//! - A restricted set of allowed tools
//! - A specific permission mode
//! - A model override
//! - A `paths` field with glob patterns for conditional activation

pub mod activation;
pub mod bundled;
pub mod format;
pub mod loader;
pub mod template;
pub mod types;

pub use activation::{extract_file_path, is_file_tool};
pub use loader::SkillLoader;
