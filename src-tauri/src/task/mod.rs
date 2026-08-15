//! Task manager — tracks depwork tasks (create/list).
//!
//! A lightweight store: task execution is handled by the agent loop itself,
//! this module only tracks the task list and persists it to the `tasks`
//! table when a database is attached. The task-type enum lives in
//! `core/types/task.rs` (`TaskType`); this module is the store.

pub mod manager;

pub use manager::TaskManager;
