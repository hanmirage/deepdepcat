//! MCP connection pool — manages connection health, reconnection, and pooling.
//!
//! Ensures MCP server connections stay alive and automatically reconnects
//! when connections drop. The pool tracks per-server health; an external
//! reconnect handler (installed by the MCP manager) performs the actual
//! reconnection with the stored server config.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Reconnect handler signature: given a server name, attempt to re-establish
/// the connection. The handler owns the server config / tool registry /
/// app handle needed to reconnect (the pool only tracks health).
pub type ReconnectHandler =
    dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;

/// Connection health status.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    /// Connection is healthy and responsive.
    Healthy,
    /// Connection is dropped and needs reconnection.
    Disconnected,
    /// Connection is being reconnected.
    Reconnecting,
}

/// A pooled connection to an MCP server.
#[derive(Clone)]
pub struct PooledConnection {
    pub status: ConnectionStatus,
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    pub reconnect_count: u32,
}

/// Compute the exponential backoff delay for the given reconnection attempt.
///
/// Schedule: `500ms * 2^(attempt - 2)` capped at 30s, plus ±25% jitter.
/// Attempts below 2 use the base 500ms delay.
fn backoff_delay(attempt: u32) -> Duration {
    let base_ms = if attempt <= 2 {
        500
    } else {
        let exp = 500u64.saturating_mul(1 << (attempt - 2).min(6));
        exp.min(30_000)
    };
    let jitter_range = base_ms / 4;
    let roll = rand::random::<u64>() % (2 * jitter_range + 1);
    let jitter = roll as i64 - jitter_range as i64;
    let clamped = base_ms as i64 + jitter;
    Duration::from_millis(clamped.max(100) as u64)
}

/// The MCP connection pool — manages all MCP server connections.
#[derive(Clone)]
pub struct McpConnectionPool {
    connections: Arc<RwLock<HashMap<String, PooledConnection>>>,
    /// Maximum number of reconnection attempts before giving up.
    max_reconnect_attempts: u32,
    /// Interval between heartbeat checks.
    heartbeat_interval: Duration,
    /// External reconnection callback — performs the actual reconnect
    /// (needs the stored server config, tool registry, and app handle).
    reconnect_handler: Arc<RwLock<Option<Arc<ReconnectHandler>>>>,
}

impl McpConnectionPool {
    /// Create a new connection pool.
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            max_reconnect_attempts: 5,
            heartbeat_interval: Duration::from_secs(60),
            reconnect_handler: Arc::new(RwLock::new(None)),
        }
    }

    /// Install the reconnect handler. The handler is invoked (with the
    /// server name) whenever a connection needs re-establishing; it must
    /// call `record_heartbeat` on success to reset the backoff counter.
    pub async fn set_reconnect_handler(&self, handler: Arc<ReconnectHandler>) {
        *self.reconnect_handler.write().await = Some(handler);
    }

    /// Register a connection in the pool.
    pub async fn register(&self, name: String) {
        let mut conns = self.connections.write().await;
        conns.insert(
            name,
            PooledConnection {
                status: ConnectionStatus::Healthy,
                last_heartbeat: Some(chrono::Utc::now()),
                reconnect_count: 0,
            },
        );
    }

    /// Unregister a connection from the pool.
    pub async fn unregister(&self, name: &str) {
        let mut conns = self.connections.write().await;
        conns.remove(name);
    }

    /// Mark a connection as disconnected (triggers reconnection).
    ///
    /// A connection ALREADY being reconnected is NOT downgraded — the
    /// health checker owns that attempt, and a concurrent liveness failure
    /// must not start a second reconnect for the same server (double
    /// disconnect/connect would churn the child process and tool registry).
    pub async fn mark_disconnected(&self, name: &str) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.get_mut(name) {
            if conn.status != ConnectionStatus::Reconnecting {
                conn.status = ConnectionStatus::Disconnected;
            }
        }
    }

    /// Record a FAILED reconnect so the checker retries on the next cycle
    /// instead of waiting for the heartbeat to go stale (a failed attempt
    /// right after a liveness failure would otherwise sit in `Reconnecting`
    /// with a fresh heartbeat and stall retries for up to 2×interval).
    pub async fn record_reconnect_failure(&self, name: &str) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.get_mut(name) {
            conn.status = ConnectionStatus::Disconnected;
        }
    }

    /// Record a successful heartbeat for a connection.
    ///
    /// Resets the reconnection attempt counter — a connection that stays
    /// alive resets its backoff schedule.
    pub async fn record_heartbeat(&self, name: &str) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.get_mut(name) {
            conn.last_heartbeat = Some(chrono::Utc::now());
            conn.status = ConnectionStatus::Healthy;
            conn.reconnect_count = 0;
        }
    }

    /// Start the background health check task.
    ///
    /// This spawns a background task that periodically checks connection
    /// health and attempts reconnection for disconnected servers.
    pub fn start_health_checker(&self) -> tokio::task::JoinHandle<()> {
        let connections = self.connections.clone();
        let heartbeat_interval = self.heartbeat_interval;
        let max_attempts = self.max_reconnect_attempts;
        let reconnect_handler = self.reconnect_handler.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);

            loop {
                interval.tick().await;

                // Check all connections.
                let names: Vec<String> = {
                    let conns = connections.read().await;
                    conns.keys().cloned().collect()
                };

                for name in names {
                    let needs_reconnect = {
                        let conns = connections.read().await;
                        if let Some(conn) = conns.get(&name) {
                            // Check if the connection has timed out (no heartbeat for 2x interval).
                            let timed_out = conn.last_heartbeat.is_some_and(|last| {
                                chrono::Utc::now().signed_duration_since(last).num_seconds()
                                    > (heartbeat_interval.as_secs() * 2) as i64
                            });
                            timed_out || conn.status == ConnectionStatus::Disconnected
                        } else {
                            false
                        }
                    };

                    if needs_reconnect {
                        let mut attempt = 0;
                        {
                            let mut conns = connections.write().await;
                            if let Some(conn) = conns.get_mut(&name) {
                                if conn.reconnect_count >= max_attempts {
                                    warn!(
                                        server = %name,
                                        attempts = conn.reconnect_count,
                                        "Max reconnection attempts reached — pausing, will retry next cycle"
                                    );
                                    // Reset the counter instead of abandoning
                                    // the server forever: a local stdio server
                                    // or transient network may come back
                                    // minutes later, and a permanent give-up
                                    // left the app with a dead connection and
                                    // no recovery path short of restart.
                                    conn.reconnect_count = 0;
                                    conn.status = ConnectionStatus::Disconnected;
                                    continue;
                                }

                                conn.status = ConnectionStatus::Reconnecting;
                                conn.reconnect_count += 1;
                                attempt = conn.reconnect_count;
                                info!(
                                    server = %name,
                                    attempt,
                                    "Attempting to reconnect MCP server"
                                );
                            }
                        }

                        // Exponential backoff with jitter between attempts.
                        tokio::time::sleep(backoff_delay(attempt)).await;

                        // Delegate the actual reconnect to the installed
                        // handler (McpManager owns the config/registry/app).
                        // On success the handler calls record_heartbeat,
                        // which resets the attempt counter and status.
                        let handler = reconnect_handler.read().await.clone();
                        if let Some(handler) = handler {
                            handler(name).await;
                        } else {
                            warn!(
                                server = %name,
                                "No reconnect handler installed — connection stays down"
                            );
                        }
                    }
                }
            }
        })
    }
}

impl Default for McpConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_at_base() {
        for attempt in [1u32, 2] {
            let delay = backoff_delay(attempt).as_millis() as i64;
            assert!((375..=625).contains(&delay), "attempt {attempt}: {delay}");
        }
    }

    #[test]
    fn backoff_grows_exponentially() {
        for (attempt, nominal) in [(3u32, 1000i64), (4, 2000), (5, 4000)] {
            let delay = backoff_delay(attempt).as_millis() as i64;
            assert!(
                (delay - nominal).abs() <= nominal / 4,
                "attempt {attempt}: {delay} vs nominal {nominal}"
            );
        }
    }

    #[test]
    fn backoff_caps_at_30s() {
        for attempt in [6u32, 10, 20, 100] {
            let delay = backoff_delay(attempt).as_millis();
            assert!(
                delay <= 30_000 + 30_000 / 4,
                "attempt {attempt} delay {delay} exceeds cap with jitter"
            );
            assert!(delay >= 100, "attempt {attempt} delay {delay} below floor");
        }
    }

    #[test]
    fn backoff_stays_within_jitter_bounds() {
        for attempt in 1..=8 {
            let delay = backoff_delay(attempt).as_millis() as i64;
            let nominal = if attempt <= 2 {
                500
            } else {
                (500u64 * (1 << (attempt - 2).min(6))).min(30_000) as i64
            };
            assert!(
                (delay - nominal).abs() <= nominal / 4 + 1,
                "attempt {attempt}: delay {delay} out of ±25% of {nominal}"
            );
        }
    }

    #[tokio::test]
    async fn mark_disconnected_does_not_downgrade_reconnecting() {
        let pool = McpConnectionPool::new();
        pool.register("srv".into()).await;
        {
            let mut conns = pool.connections.write().await;
            conns.get_mut("srv").unwrap().status = ConnectionStatus::Reconnecting;
        }
        // A liveness failure during an in-flight reconnect must not start a
        // second reconnect for the same server.
        pool.mark_disconnected("srv").await;
        assert_eq!(
            pool.connections.read().await.get("srv").unwrap().status,
            ConnectionStatus::Reconnecting
        );
    }

    #[tokio::test]
    async fn mark_disconnected_transitions_healthy() {
        let pool = McpConnectionPool::new();
        pool.register("srv".into()).await;
        pool.mark_disconnected("srv").await;
        assert_eq!(
            pool.connections.read().await.get("srv").unwrap().status,
            ConnectionStatus::Disconnected
        );
    }

    #[tokio::test]
    async fn record_reconnect_failure_reenables_retry() {
        let pool = McpConnectionPool::new();
        pool.register("srv".into()).await;
        {
            let mut conns = pool.connections.write().await;
            conns.get_mut("srv").unwrap().status = ConnectionStatus::Reconnecting;
        }
        // A failed reconnect must not stall in Reconnecting with a fresh
        // heartbeat — the checker retries on the next cycle.
        pool.record_reconnect_failure("srv").await;
        assert_eq!(
            pool.connections.read().await.get("srv").unwrap().status,
            ConnectionStatus::Disconnected
        );
    }
}
