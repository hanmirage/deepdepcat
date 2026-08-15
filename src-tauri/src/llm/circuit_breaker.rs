//! Circuit breaker for LLM API calls — prevents cascade failures.
//!
//! State machine: Closed → Open → HalfOpen → Closed/Open.
//!
//! - **Closed**: All requests pass through. Failures increment the counter.
//!   When consecutive failures reach `failure_threshold`, transition to Open.
//! - **Open**: All requests are rejected immediately with `AppError::LlmCircuitOpen`.
//!   After `open_timeout_secs`, transition to HalfOpen.
//! - **HalfOpen**: A single probe request is allowed. If it succeeds, transition
//!   back to Closed. If it fails, transition back to Open.
//!
//! Each provider has its own independent circuit breaker instance.

use crate::core::error::{AppError, AppResult};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Configuration for a single circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// How long the circuit stays open before allowing a probe (seconds).
    pub open_timeout_secs: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            open_timeout_secs: 30,
        }
    }
}

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Closed — requests flow normally.
    Closed,
    /// Open — requests are rejected immediately.
    Open,
    /// HalfOpen — a single probe request is allowed.
    HalfOpen,
}

impl CircuitState {
    /// Stable string identifier for logging and event payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// How long a HalfOpen probe may stay in-flight before it is considered
/// lost. A probe that neither succeeds nor fails (e.g. the caller errored
/// between `check()` and the request, or a non-tripping 429/400 ended the
/// call without a record) would otherwise wedge the circuit permanently —
/// every subsequent request rejected with "probe in flight" until restart.
///
/// 150s is longer than the stream idle watchdog (120s) and shorter than the
/// non-stream request timeout, so a live probe always resolves through its
/// normal error path first; this timeout is the last-resort release.
const PROBE_TIMEOUT: Duration = Duration::from_secs(150);

/// Internal state for a single provider's circuit breaker.
#[derive(Debug)]
struct CircuitInner {
    state: CircuitState,
    consecutive_failures: u32,
    /// When the circuit transitioned to Open (for timeout calculation).
    opened_at: Option<Instant>,
    /// Whether a HalfOpen probe is currently in-flight.
    probe_in_flight: bool,
    /// When the current probe started (for stale-probe release).
    probe_started_at: Option<Instant>,
}

impl Default for CircuitInner {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            opened_at: None,
            probe_in_flight: false,
            probe_started_at: None,
        }
    }
}

/// A circuit breaker that tracks failures per provider.
///
/// Thread-safe via `Mutex<HashMap>`. Each provider name maps to its own
/// independent circuit breaker state.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    circuits: Mutex<HashMap<String, CircuitInner>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            circuits: Mutex::new(HashMap::new()),
        }
    }

    /// Check whether a request to the given provider is allowed.
    ///
    /// Returns `Ok(())` if the request should proceed, or `Err` if the
    /// circuit is Open. If the circuit is HalfOpen, this reserves the
    /// single probe slot (returns `Ok(())` only for the first caller).
    pub fn check(&self, provider: &str) -> AppResult<()> {
        let mut circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
        let inner = circuits.entry(provider.to_string()).or_default();

        match inner.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                // Check if the open timeout has elapsed.
                if let Some(opened) = inner.opened_at {
                    let elapsed = opened.elapsed();
                    if elapsed >= Duration::from_secs(self.config.open_timeout_secs) {
                        inner.state = CircuitState::HalfOpen;
                        inner.probe_in_flight = true;
                        inner.probe_started_at = Some(Instant::now());
                        info!(
                            provider = %provider,
                            "Circuit breaker transitioning Open → HalfOpen"
                        );
                        return Ok(());
                    }
                }
                warn!(
                    provider = %provider,
                    "Circuit breaker is Open — rejecting request"
                );
                Err(AppError::Internal(format!(
                    "circuit breaker open for provider '{}'",
                    provider
                )))
            }
            CircuitState::HalfOpen => {
                if inner.probe_in_flight {
                    // A probe that outlived PROBE_TIMEOUT never produced an
                    // outcome (caller error path, lost record) — treat it as
                    // stale, release the slot, and let this request become
                    // the new probe. Without this, the circuit wedges
                    // permanently on any unrecorded probe outcome.
                    let stale = inner
                        .probe_started_at
                        .map(|started| started.elapsed() >= PROBE_TIMEOUT)
                        .unwrap_or(true);
                    if stale {
                        warn!(
                            provider = %provider,
                            timeout_secs = PROBE_TIMEOUT.as_secs(),
                            "Circuit breaker HalfOpen — stale probe released, starting new probe"
                        );
                        inner.probe_started_at = Some(Instant::now());
                        return Ok(());
                    }
                    warn!(
                        provider = %provider,
                        "Circuit breaker HalfOpen — probe already in flight, rejecting"
                    );
                    return Err(AppError::Internal(format!(
                        "circuit breaker half-open for provider '{}' — probe in flight",
                        provider
                    )));
                }
                inner.probe_in_flight = true;
                inner.probe_started_at = Some(Instant::now());
                Ok(())
            }
        }
    }

    /// Record a successful call to the given provider.
    ///
    /// Resets the failure counter and transitions to Closed from any state.
    pub fn record_success(&self, provider: &str) {
        let mut circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(inner) = circuits.get_mut(provider) {
            if inner.state != CircuitState::Closed {
                info!(
                    provider = %provider,
                    old_state = inner.state.as_str(),
                    "Circuit breaker recovered → Closed"
                );
            }
            inner.state = CircuitState::Closed;
            inner.consecutive_failures = 0;
            inner.opened_at = None;
            inner.probe_in_flight = false;
            inner.probe_started_at = None;
        }
    }

    /// Record a failed call to the given provider.
    ///
    /// Increments the failure counter. If the threshold is reached while
    /// Closed, transitions to Open. If already HalfOpen, transitions back
    /// to Open (probe failed).
    pub fn record_failure(&self, provider: &str) {
        let mut circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
        let inner = circuits.entry(provider.to_string()).or_default();

        match inner.state {
            CircuitState::Closed => {
                inner.consecutive_failures += 1;
                if inner.consecutive_failures >= self.config.failure_threshold {
                    inner.state = CircuitState::Open;
                    inner.opened_at = Some(Instant::now());
                    inner.probe_in_flight = false;
                    inner.probe_started_at = None;
                    warn!(
                        provider = %provider,
                        failures = inner.consecutive_failures,
                        threshold = self.config.failure_threshold,
                        "Circuit breaker tripped Closed → Open"
                    );
                }
            }
            CircuitState::HalfOpen => {
                warn!(
                    provider = %provider,
                    "Circuit breaker HalfOpen probe failed → Open"
                );
                inner.state = CircuitState::Open;
                inner.opened_at = Some(Instant::now());
                inner.probe_in_flight = false;
                inner.probe_started_at = None;
            }
            CircuitState::Open => {
                // Already open — no action needed.
            }
        }
    }

    /// Get the current state for a provider (for frontend display).
    pub fn state(&self, provider: &str) -> CircuitState {
        let circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
        circuits
            .get(provider)
            .map(|i| i.state)
            .unwrap_or(CircuitState::Closed)
    }

    /// Get all provider states plus their consecutive failure counts
    /// (for frontend display).
    pub fn all_states(&self) -> Vec<(String, CircuitState, u32)> {
        let circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
        circuits
            .iter()
            .map(|(k, v)| (k.clone(), v.state, v.consecutive_failures))
            .collect()
    }

    /// Manually reset a provider's circuit (for frontend "reset" button).
    pub fn reset(&self, provider: &str) {
        let mut circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(inner) = circuits.get_mut(provider) {
            inner.state = CircuitState::Closed;
            inner.consecutive_failures = 0;
            inner.opened_at = None;
            inner.probe_in_flight = false;
            inner.probe_started_at = None;
            info!(provider = %provider, "Circuit breaker manually reset");
        }
    }
}

/// Whether a final LLM call failure should count toward opening the circuit.
///
/// Rate-limit (429), client (400/404), prompt-too-long, and max-tokens
/// exceeded are deterministic request-side signals — a rate-limit storm, a
/// bad request, or a context-window overflow must not trip the breaker.
/// `PromptTooLong` / `MaxTokensExceeded` in particular are addressed by the
/// loop's OWN recovery (compaction to shrink the request, escalating the
/// output cap); counting them would open the provider circuit and then
/// reject the very retry that fixed the underlying problem. Server errors
/// (5xx, 529), transport failures, and auth problems still count.
pub fn trips_circuit_breaker(err: &AppError) -> bool {
    !matches!(
        err,
        AppError::LlmRateLimited { .. }
            | AppError::LlmApi {
                status_code: Some(400 | 404 | 429),
                ..
            }
            | AppError::PromptTooLong { .. }
            | AppError::MaxTokensExceeded { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_allows_requests() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert!(cb.check("deepseek").is_ok());
    }

    #[test]
    fn opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            open_timeout_secs: 60,
        });
        cb.record_failure("deepseek");
        cb.record_failure("deepseek");
        assert!(cb.check("deepseek").is_ok());
        cb.record_failure("deepseek");
        assert!(cb.check("deepseek").is_err());
        assert_eq!(cb.state("deepseek"), CircuitState::Open);
    }

    #[test]
    fn success_resets_failures() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            open_timeout_secs: 60,
        });
        cb.record_failure("deepseek");
        cb.record_failure("deepseek");
        cb.record_success("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Closed);
        cb.record_failure("deepseek");
        assert!(cb.check("deepseek").is_ok());
    }

    #[test]
    fn half_open_after_timeout() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 0,
        });
        cb.record_failure("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Open);

        // open_timeout_secs=0 means the transition check happens immediately.
        // We need to yield to let the Instant advance past 0.
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.check("deepseek").is_ok());
        assert_eq!(cb.state("deepseek"), CircuitState::HalfOpen);
    }

    #[test]
    fn half_open_probe_success_closes() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 0,
        });
        cb.record_failure("deepseek");
        std::thread::sleep(Duration::from_millis(10));
        cb.check("deepseek").unwrap();
        cb.record_success("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Closed);
    }

    #[test]
    fn half_open_probe_failure_reopens() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 0,
        });
        cb.record_failure("deepseek");
        std::thread::sleep(Duration::from_millis(10));
        cb.check("deepseek").unwrap();
        cb.record_failure("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Open);
    }

    #[test]
    fn half_open_probe_rejected_while_in_flight() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 0,
        });
        cb.record_failure("deepseek");
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.check("deepseek").is_ok());
        // Second caller while the probe is in flight is rejected.
        assert!(cb.check("deepseek").is_err());
    }

    #[test]
    fn half_open_stale_probe_is_released_and_replaced() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 0,
        });
        cb.record_failure("deepseek");
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.check("deepseek").is_ok());
        // A probe whose outcome was never recorded (e.g. a 429/400 ended
        // the call without a record) must not wedge the circuit forever:
        // backdate the probe and verify a new caller gets through.
        {
            let mut circuits = cb.circuits.lock().unwrap_or_else(|e| e.into_inner());
            let inner = circuits.get_mut("deepseek").unwrap();
            inner.probe_started_at = Some(Instant::now() - PROBE_TIMEOUT - Duration::from_secs(1));
        }
        assert!(cb.check("deepseek").is_ok());
        // The stale slot was replaced by the new probe.
        assert!(cb.check("deepseek").is_err());
    }

    #[test]
    fn providers_are_independent() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 60,
        });
        cb.record_failure("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Open);
        assert_eq!(cb.state("openai"), CircuitState::Closed);
        assert!(cb.check("openai").is_ok());
        assert!(cb.check("deepseek").is_err());
    }

    #[test]
    fn manual_reset_works() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_timeout_secs: 60,
        });
        cb.record_failure("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Open);
        cb.reset("deepseek");
        assert_eq!(cb.state("deepseek"), CircuitState::Closed);
        assert!(cb.check("deepseek").is_ok());
    }

    #[test]
    fn rate_limit_and_client_errors_do_not_trip_breaker() {
        assert!(!trips_circuit_breaker(&AppError::LlmRateLimited {
            retry_after_secs: Some(5),
        }));
        assert!(!trips_circuit_breaker(&AppError::LlmApi {
            source: "bad request".into(),
            status_code: Some(400),
        }));
        assert!(!trips_circuit_breaker(&AppError::LlmApi {
            source: "not found".into(),
            status_code: Some(404),
        }));
        assert!(!trips_circuit_breaker(&AppError::LlmApi {
            source: "too many".into(),
            status_code: Some(429),
        }));
    }

    #[test]
    fn prompt_too_long_and_max_tokens_do_not_trip_breaker() {
        // These deterministic request-side errors are fixed by the loop's own
        // recovery (compaction to shrink the request / escalating the output
        // cap). Counting them would open the provider circuit and then reject
        // the very retry that resolves the underlying problem.
        assert!(!trips_circuit_breaker(&AppError::PromptTooLong {
            max_tokens: Some(8192),
        }));
        assert!(!trips_circuit_breaker(&AppError::MaxTokensExceeded {
            requested: 8192,
            max: 4096,
        }));
    }

    #[test]
    fn server_errors_trip_breaker_including_529() {
        assert!(trips_circuit_breaker(&AppError::LlmApi {
            source: "server error".into(),
            status_code: Some(500),
        }));
        assert!(trips_circuit_breaker(&AppError::LlmApi {
            source: "overloaded".into(),
            status_code: Some(529),
        }));
        assert!(trips_circuit_breaker(&AppError::LlmAuth("bad key".into())));
        assert!(trips_circuit_breaker(&AppError::Timeout(30)));
    }
}
