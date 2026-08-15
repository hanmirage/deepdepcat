//! Agent System — Core agent loop with tool integration.
//!
//! Inspired by upstream patterns:
//! - Async generator pipeline for streaming
//! - Multi-turn conversation with context management
//! - Tool execution with lifecycle hooks

pub mod agent_builder;
pub mod agent_loop;
pub mod budget;
pub mod chat_state;
pub mod compaction;
pub mod context;
pub mod definition;
pub mod discovery;
pub mod handlers;
pub mod image_transcribe;
pub mod intent;
pub mod intent_effort;
pub mod interjection;
pub mod multi_agent;
pub mod notification;
pub mod prompt_loader;
pub mod prompt_queue;
pub mod prompts;
pub mod running;
pub mod sanitize;
pub mod session;
pub mod streaming;
pub mod system_reminder;
pub mod token;
pub mod workflow;
