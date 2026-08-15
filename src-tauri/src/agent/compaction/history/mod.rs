//! Conversation history filtering — removes redundant or obsolete items
//! before compaction to reduce the summarization load.
//!
//! Submodules:
//! - `types` — configuration and shared types
//! - `filter` — conversation filtering (strip reasoning, truncate tool results, dedup)
//! - `validate` — post-compaction invariant validation (orphan detection)

pub mod filter;
pub mod types;
pub mod validate;

pub use filter::filter_history;
pub use types::FilterConfig;
pub use validate::validate_no_orphans;
