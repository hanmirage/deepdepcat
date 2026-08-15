//! Tool system — the extensible capability layer.
//!
//! Every agent capability (file I/O, shell execution, web access, etc.) is
//! exposed through the `Tool` trait. The registry manages available tools,
//! and the dispatcher handles concurrent execution with permission checks.

pub mod background;
pub mod builtin;
pub mod dispatch;
pub mod failure_guidance;
pub mod registry;
pub mod reminders;
pub mod stale_edit;
