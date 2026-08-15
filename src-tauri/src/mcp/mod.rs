//! MCP (Model Context Protocol) integration.
//!
//! Supports three transport types:
//! - **stdio**: Spawn a child process and communicate via stdin/stdout
//! - **SSE**: Connect to a Server-Sent Events endpoint
//! - **HTTP**: Connect to an HTTP endpoint
//!
//! Each MCP server exposes tools, resources, and prompts that are
//! wrapped as native DeepDepCat tools.

pub mod auto_setup;
pub mod client;
pub mod connection_pool;
pub mod credential_crypto;
pub mod credentials;
pub mod manager;
pub mod tool_bridge;
pub mod transport;
pub mod types;

#[cfg(test)]
mod smoke_test;
