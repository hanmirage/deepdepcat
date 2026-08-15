//! Memory system — persistent knowledge storage with hybrid search.
//!
//! Features:
//! - SQLite FTS5 full-text search (keyword-based BM25 scoring)
//! - Vector embedding similarity search (cosine similarity)
//! - Hybrid search merging BM25 + cosine + recency scores
//! - MMR (Maximal Marginal Relevance) deduplication for diverse results
//! - Text chunker for splitting large documents before indexing
//! - Category-based organization (project, preference, fact, etc.)
//! - Auto-injection of relevant memories into agent context
//! - Access tracking and decay for relevance scoring
//! - Dream synthesis (background memory consolidation via LLM)
//! - Local hash-based embedding fallback (no API key required)
//! - Procedural memory (learned, verified workflows — procedures.md)

pub mod dream;
pub mod embedding;
pub mod injection;
pub mod learning;
pub mod memory_file;
pub mod procedure;
pub mod procedure_capture;
pub mod project_cognition;
pub mod search;
pub mod store;
pub mod watcher;

#[cfg(test)]
mod live_smoke;
