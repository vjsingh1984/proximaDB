//! Circuit Breaker Pattern Implementation
//!
//! The circuit breaker prevents cascading failures by monitoring failures and
//! temporarily blocking requests when a threshold is exceeded.
//!
//! States:
//! - **Closed**: Normal operation, requests pass through
//! - **Open**: Failure threshold exceeded, requests are blocked
//! - **HalfOpen**: Testing if service has recovered

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use thiserror::Error;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests pass through
    Closed,
    /// Failure threshold exceeded - requests blocked
    Open,
    /// Testing recovery - limited requests allowed
    HalfOpen,
}

/// Configuration for the circuit breaker
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening the circuit
    pub failure_threshold: u32,
    /// Number of successes in half-open state before closing
    pub success_threshold: u32,
    /// Time to wait before transitioning from open to half-open
    pub timeout_secs: u64,
    /// Maximum concurrent requests in half-open state
    pub half_open_max_requests: u32,
    /// Name for logging/metrics
    pub name: String,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            timeout_secs: 30,
            half_open_max_requests: 3,
            name: "default".to_string(),
        }
    }
}

/// Error types for circuit breaker operations
#[derive(Error, Debug)]
pub enum CircuitBreakerError {
    /// Circuit is open, request rejected
    #[error("Circuit breaker '{name}' is open - requests are blocked")]
    CircuitOpen { name: String },

    /// Maximum half-open requests exceeded
    #[error("Circuit breaker '{name}' half-open limit exceeded")]
    HalfOpenLimitExceeded { name: String },

    /// Underlying operation failed
    #[error("Operation failed: {0}")]
    OperationFailed(#[from] anyhow::Error),
}

/// Circuit breaker for protecting against cascading failures
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: RwLock<CircuitState>,
    failure_count: AtomicU32,
    success_count: AtomicU32,
    half_open_requests: AtomicU32,
    last_failure_time: RwLock<Option<Instant>>,
    /// Metrics: total requests
    total_requests: AtomicU64,
    /// Metrics: rejected requests
    rejected_requests: AtomicU64,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration
    pub fn new(config: CircuitBreakerConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            state: RwLock::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            half_open_requests: AtomicU32::new(0),
            last_failure_time: RwLock::new(None),
            total_requests: AtomicU64::new(0),
            rejected_requests: AtomicU64::new(0),
        })
    }

    /// Create a circuit breaker with default configuration
    pub fn with_name(name: impl Into<String>) -> Arc<Self> {
        Self::new(CircuitBreakerConfig {
            name: name.into(),
            ..Default::default()
        })
    }

    /// Get the current state of the circuit breaker
    pub fn state(&self) -> CircuitState {
        self.check_state_transition();
        *self.state.read()
    }

    /// Check if the circuit allows a request
    pub fn allow_request(&self) -> Result<(), CircuitBreakerError> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.check_state_transition();

        let state = *self.state.read();
        match state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                self.rejected_requests.fetch_add(1, Ordering::Relaxed);
                Err(CircuitBreakerError::CircuitOpen {
                    name: self.config.name.clone(),
                })
            }
            CircuitState::HalfOpen => {
                let current = self.half_open_requests.fetch_add(1, Ordering::Relaxed);
                if current >= self.config.half_open_max_requests {
                    self.half_open_requests.fetch_sub(1, Ordering::Relaxed);
                    self.rejected_requests.fetch_add(1, Ordering::Relaxed);
                    Err(CircuitBreakerError::HalfOpenLimitExceeded {
                        name: self.config.name.clone(),
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Record a successful operation
    pub fn record_success(&self) {
        let state = *self.state.read();
        match state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::Relaxed);
            }
            CircuitState::HalfOpen => {
                self.half_open_requests.fetch_sub(1, Ordering::Relaxed);
                let success = self.success_count.fetch_add(1, Ordering::Relaxed) + 1;
                if success >= self.config.success_threshold {
                    // Transition to closed
                    *self.state.write() = CircuitState::Closed;
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.success_count.store(0, Ordering::Relaxed);
                    tracing::info!(
                        circuit_breaker = %self.config.name,
                        "Circuit breaker closed - service recovered"
                    );
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
            }
        }
    }

    /// Record a failed operation
    pub fn record_failure(&self) {
        let state = *self.state.read();
        match state {
            CircuitState::Closed => {
                let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                if failures >= self.config.failure_threshold {
                    // Transition to open
                    *self.state.write() = CircuitState::Open;
                    *self.last_failure_time.write() = Some(Instant::now());
                    tracing::warn!(
                        circuit_breaker = %self.config.name,
                        failures = failures,
                        "Circuit breaker opened - failure threshold exceeded"
                    );
                }
            }
            CircuitState::HalfOpen => {
                self.half_open_requests.fetch_sub(1, Ordering::Relaxed);
                // Any failure in half-open immediately opens the circuit
                *self.state.write() = CircuitState::Open;
                *self.last_failure_time.write() = Some(Instant::now());
                self.success_count.store(0, Ordering::Relaxed);
                tracing::warn!(
                    circuit_breaker = %self.config.name,
                    "Circuit breaker re-opened - failure in half-open state"
                );
            }
            CircuitState::Open => {
                // Already open, just update the failure time
                *self.last_failure_time.write() = Some(Instant::now());
            }
        }
    }

    /// Check if we should transition states
    fn check_state_transition(&self) {
        let state = *self.state.read();
        if state == CircuitState::Open {
            if let Some(last_failure) = *self.last_failure_time.read() {
                let timeout = Duration::from_secs(self.config.timeout_secs);
                if last_failure.elapsed() >= timeout {
                    // Transition to half-open
                    *self.state.write() = CircuitState::HalfOpen;
                    self.success_count.store(0, Ordering::Relaxed);
                    self.half_open_requests.store(0, Ordering::Relaxed);
                    tracing::info!(
                        circuit_breaker = %self.config.name,
                        "Circuit breaker half-open - testing recovery"
                    );
                }
            }
        }
    }

    /// Execute an async operation with circuit breaker protection
    pub async fn execute<F, Fut, T, E>(
        self: &Arc<Self>,
        f: F,
    ) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: Into<anyhow::Error>,
    {
        self.allow_request()?;

        match f().await {
            Ok(result) => {
                self.record_success();
                Ok(result)
            }
            Err(e) => {
                self.record_failure();
                Err(CircuitBreakerError::OperationFailed(e.into()))
            }
        }
    }

    /// Get metrics for monitoring
    pub fn metrics(&self) -> CircuitBreakerMetrics {
        CircuitBreakerMetrics {
            name: self.config.name.clone(),
            state: self.state(),
            failure_count: self.failure_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            total_requests: self.total_requests.load(Ordering::Relaxed),
            rejected_requests: self.rejected_requests.load(Ordering::Relaxed),
        }
    }

    /// Reset the circuit breaker to closed state
    pub fn reset(&self) {
        *self.state.write() = CircuitState::Closed;
        self.failure_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
        self.half_open_requests.store(0, Ordering::Relaxed);
        *self.last_failure_time.write() = None;
        tracing::info!(
            circuit_breaker = %self.config.name,
            "Circuit breaker manually reset"
        );
    }
}

/// Metrics for circuit breaker monitoring
#[derive(Debug, Clone)]
pub struct CircuitBreakerMetrics {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub total_requests: u64,
    pub rejected_requests: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::with_name("test");
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_opens_after_failures() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        assert_eq!(cb.state(), CircuitState::Closed);

        // Record failures
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_rejects_when_open() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_secs: 1000, // Long timeout to stay open
            ..Default::default()
        });

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        let result = cb.allow_request();
        assert!(matches!(result, Err(CircuitBreakerError::CircuitOpen { .. })));
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 2);

        cb.record_success();
        assert_eq!(cb.failure_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_circuit_breaker_metrics() {
        let cb = CircuitBreaker::with_name("test-metrics");

        let _ = cb.allow_request();
        cb.record_failure();
        let _ = cb.allow_request();
        cb.record_success();

        let metrics = cb.metrics();
        assert_eq!(metrics.name, "test-metrics");
        assert_eq!(metrics.total_requests, 2);
    }
}
