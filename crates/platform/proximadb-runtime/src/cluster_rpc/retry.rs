/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Retry Policy and Circuit Breaker for Resilient RPC Calls
//!
//! This module provides:
//! - **RetryPolicy**: Configurable retry behavior with exponential backoff and jitter
//! - **CircuitBreaker**: Prevents cascading failures by failing fast when a node is down
//!
//! ## Circuit Breaker States
//!
//! ```text
//! ┌───────────┐     failures >= threshold      ┌───────────┐
//! │  CLOSED   │ ─────────────────────────────> │   OPEN    │
//! │ (normal)  │                                │ (failing) │
//! └───────────┘                                └───────────┘
//!       ^                                            │
//!       │                                            │ reset_timeout elapsed
//!       │                                            v
//!       │          success                    ┌───────────┐
//!       └──────────────────────────────────── │ HALF-OPEN │
//!                   failure                   │ (testing) │
//!                   ┌────────────────────────>└───────────┘
//!                   │                                │
//!                   └────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! let retry_policy = RetryPolicy::default();
//! let circuit_breaker = CircuitBreaker::new(5, Duration::from_secs(30));
//!
//! // Check if request should be allowed
//! if !circuit_breaker.should_allow_request() {
//!     return Err(RpcError::connection("Circuit breaker is open"));
//! }
//!
//! // Execute with retry
//! for attempt in 0..retry_policy.max_retries {
//!     match do_rpc_call().await {
//!         Ok(response) => {
//!             circuit_breaker.record_success();
//!             return Ok(response);
//!         }
//!         Err(e) if e.is_retryable() => {
//!             circuit_breaker.record_failure();
//!             let delay = retry_policy.compute_delay(attempt);
//!             tokio::time::sleep(delay).await;
//!         }
//!         Err(e) => {
//!             circuit_breaker.record_failure();
//!             return Err(e);
//!         }
//!     }
//! }
//! ```

use rand::Rng;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::error::RpcError;

// ============================================================================
// RETRY POLICY
// ============================================================================

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (0 means no retries)
    pub max_retries: u32,

    /// Base delay between retries
    pub base_delay: Duration,

    /// Maximum delay between retries (caps exponential backoff)
    pub max_delay: Duration,

    /// Whether to use exponential backoff
    pub exponential_backoff: bool,

    /// Backoff multiplier (e.g., 2.0 for doubling)
    pub backoff_multiplier: f64,

    /// Maximum jitter as a fraction of the delay (e.g., 0.1 for 10%)
    pub jitter_fraction: f64,

    /// Whether to retry on timeout errors
    pub retry_on_timeout: bool,

    /// Whether to retry on connection errors
    pub retry_on_connection_error: bool,

    /// Whether to retry on rate limiting
    pub retry_on_rate_limit: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            exponential_backoff: true,
            backoff_multiplier: 2.0,
            jitter_fraction: 0.2,
            retry_on_timeout: true,
            retry_on_connection_error: true,
            retry_on_rate_limit: true,
        }
    }
}

impl RetryPolicy {
    /// Create a new retry policy with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a policy that never retries
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Create a policy for aggressive retry (more retries, shorter delays)
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            base_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(5),
            ..Default::default()
        }
    }

    /// Create a policy for conservative retry (fewer retries, longer delays)
    pub fn conservative() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            ..Default::default()
        }
    }

    /// Set maximum retries
    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// Set base delay
    pub fn with_base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// Set maximum delay
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Enable or disable exponential backoff
    pub fn with_exponential_backoff(mut self, enabled: bool) -> Self {
        self.exponential_backoff = enabled;
        self
    }

    /// Set jitter fraction
    pub fn with_jitter(mut self, fraction: f64) -> Self {
        self.jitter_fraction = fraction.clamp(0.0, 1.0);
        self
    }

    /// Compute the delay for a given attempt number (0-indexed)
    pub fn compute_delay(&self, attempt: u32) -> Duration {
        if attempt >= self.max_retries {
            return Duration::ZERO;
        }

        let base_ms = self.base_delay.as_millis() as f64;
        let delay_ms = if self.exponential_backoff {
            base_ms * self.backoff_multiplier.powi(attempt as i32)
        } else {
            base_ms
        };

        // Apply max delay cap
        let max_ms = self.max_delay.as_millis() as f64;
        let capped_delay_ms = delay_ms.min(max_ms);

        // Apply jitter
        let jitter_range = capped_delay_ms * self.jitter_fraction;
        let jitter = if jitter_range > 0.0 {
            let mut rng = rand::thread_rng();
            rng.gen_range(-jitter_range..jitter_range)
        } else {
            0.0
        };

        let final_delay_ms = (capped_delay_ms + jitter).max(0.0);
        Duration::from_millis(final_delay_ms as u64)
    }

    /// Check if an error should be retried
    pub fn should_retry(&self, error: &RpcError) -> bool {
        if !error.is_retryable() {
            return false;
        }

        match error.kind() {
            super::error::RpcErrorKind::Timeout => self.retry_on_timeout,
            super::error::RpcErrorKind::Connection => self.retry_on_connection_error,
            super::error::RpcErrorKind::RateLimited => self.retry_on_rate_limit,
            _ => error.is_retryable(),
        }
    }
}

// ============================================================================
// CIRCUIT BREAKER
// ============================================================================

/// Circuit breaker states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitState {
    /// Normal operation - all requests allowed
    Closed = 0,
    /// Failing fast - all requests rejected
    Open = 1,
    /// Testing - allowing a single request to test recovery
    HalfOpen = 2,
}

impl From<u8> for CircuitState {
    fn from(value: u8) -> Self {
        match value {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        }
    }
}

/// Circuit breaker for preventing cascading failures
///
/// The circuit breaker tracks failures and opens the circuit when
/// the failure threshold is reached. After a reset timeout, it
/// transitions to half-open to test if the target has recovered.
///
/// Thread-safe using atomics.
pub struct CircuitBreaker {
    /// Current state (atomic for thread safety)
    state: AtomicU8,

    /// Number of consecutive failures
    failure_count: AtomicU32,

    /// Number of consecutive successes (used in half-open state)
    success_count: AtomicU32,

    /// Failure threshold before opening the circuit
    failure_threshold: u32,

    /// Number of successes required to close the circuit from half-open
    success_threshold: u32,

    /// Reset timeout after which to try half-open
    reset_timeout: Duration,

    /// When the circuit was last opened (stored as duration since epoch in nanos)
    last_failure_time: AtomicU64,

    /// Instant when the breaker was created (for time calculations)
    created_at: Instant,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    ///
    /// # Arguments
    ///
    /// * `failure_threshold` - Number of consecutive failures before opening
    /// * `reset_timeout` - Duration to wait before testing recovery
    pub fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        Self {
            state: AtomicU8::new(CircuitState::Closed as u8),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            failure_threshold,
            success_threshold: 1, // Default: one success closes the circuit
            reset_timeout,
            last_failure_time: AtomicU64::new(0),
            created_at: Instant::now(),
        }
    }

    /// Create with custom success threshold for closing from half-open
    pub fn with_success_threshold(mut self, threshold: u32) -> Self {
        self.success_threshold = threshold.max(1);
        self
    }

    /// Check if a request should be allowed
    ///
    /// Returns `true` if the request should proceed, `false` if it should fail fast.
    pub fn should_allow_request(&self) -> bool {
        match self.state() {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if reset timeout has elapsed
                if self.should_attempt_reset() {
                    // Transition to half-open
                    self.state
                        .compare_exchange(
                            CircuitState::Open as u8,
                            CircuitState::HalfOpen as u8,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .ok();
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                // In half-open, allow limited requests
                true
            }
        }
    }

    /// Record a successful request
    pub fn record_success(&self) {
        let current_state = self.state();

        match current_state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::Relaxed);
            }
            CircuitState::HalfOpen => {
                let successes = self.success_count.fetch_add(1, Ordering::SeqCst) + 1;
                if successes >= self.success_threshold {
                    // Transition to closed
                    self.state
                        .store(CircuitState::Closed as u8, Ordering::SeqCst);
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.success_count.store(0, Ordering::Relaxed);
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
                self.state
                    .store(CircuitState::Closed as u8, Ordering::SeqCst);
                self.failure_count.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Record a failed request
    pub fn record_failure(&self) {
        let current_state = self.state();

        match current_state {
            CircuitState::Closed => {
                let failures = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
                if failures >= self.failure_threshold {
                    self.open_circuit();
                }
            }
            CircuitState::HalfOpen => {
                // Failure in half-open immediately opens the circuit
                self.open_circuit();
            }
            CircuitState::Open => {
                // Update last failure time to extend the open period
                self.update_last_failure_time();
            }
        }
    }

    /// Get the current state
    pub fn state(&self) -> CircuitState {
        CircuitState::from(self.state.load(Ordering::SeqCst))
    }

    /// Get the failure count
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Relaxed)
    }

    /// Get the success count (in half-open state)
    pub fn success_count(&self) -> u32 {
        self.success_count.load(Ordering::Relaxed)
    }

    /// Check if the circuit is open
    pub fn is_open(&self) -> bool {
        self.state() == CircuitState::Open
    }

    /// Check if the circuit is closed (normal operation)
    pub fn is_closed(&self) -> bool {
        self.state() == CircuitState::Closed
    }

    /// Manually reset the circuit breaker to closed state
    pub fn reset(&self) {
        self.state
            .store(CircuitState::Closed as u8, Ordering::SeqCst);
        self.failure_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
    }

    /// Manually open the circuit breaker
    pub fn force_open(&self) {
        self.open_circuit();
    }

    // Internal: Open the circuit
    fn open_circuit(&self) {
        self.state.store(CircuitState::Open as u8, Ordering::SeqCst);
        self.success_count.store(0, Ordering::Relaxed);
        self.update_last_failure_time();
    }

    // Internal: Update last failure time
    fn update_last_failure_time(&self) {
        let now = Instant::now().duration_since(self.created_at).as_nanos() as u64;
        self.last_failure_time.store(now, Ordering::Relaxed);
    }

    // Internal: Check if reset timeout has elapsed
    fn should_attempt_reset(&self) -> bool {
        let last_failure_nanos = self.last_failure_time.load(Ordering::Relaxed);
        let now_nanos = Instant::now().duration_since(self.created_at).as_nanos() as u64;
        let elapsed_nanos = now_nanos.saturating_sub(last_failure_nanos);
        Duration::from_nanos(elapsed_nanos) >= self.reset_timeout
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("state", &self.state())
            .field("failure_count", &self.failure_count())
            .field("failure_threshold", &self.failure_threshold)
            .field("reset_timeout", &self.reset_timeout)
            .finish()
    }
}

// ============================================================================
// RETRY EXECUTOR
// ============================================================================

/// Executes an operation with retry and circuit breaker logic
///
/// This is a helper struct that combines RetryPolicy and CircuitBreaker
/// for convenient use.
pub struct RetryExecutor {
    /// Retry policy configuration
    pub retry_policy: RetryPolicy,

    /// Circuit breaker for the target (Arc for shared state)
    pub circuit_breaker: std::sync::Arc<CircuitBreaker>,
}

impl RetryExecutor {
    /// Create a new retry executor
    pub fn new(retry_policy: RetryPolicy, circuit_breaker: std::sync::Arc<CircuitBreaker>) -> Self {
        Self {
            retry_policy,
            circuit_breaker,
        }
    }

    /// Create a new retry executor with an owned circuit breaker
    pub fn with_owned(retry_policy: RetryPolicy, circuit_breaker: CircuitBreaker) -> Self {
        Self {
            retry_policy,
            circuit_breaker: std::sync::Arc::new(circuit_breaker),
        }
    }

    /// Create with default settings
    pub fn default_for_node() -> Self {
        Self {
            retry_policy: RetryPolicy::default(),
            circuit_breaker: std::sync::Arc::new(CircuitBreaker::new(5, Duration::from_secs(30))),
        }
    }

    /// Execute an async operation with retry and circuit breaker
    pub async fn execute<F, Fut, T>(&self, mut operation: F) -> Result<T, RpcError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, RpcError>>,
    {
        // Check circuit breaker
        if !self.circuit_breaker.should_allow_request() {
            return Err(RpcError::connection("Circuit breaker is open"));
        }

        let mut last_error = None;

        for attempt in 0..=self.retry_policy.max_retries {
            match operation().await {
                Ok(result) => {
                    self.circuit_breaker.record_success();
                    return Ok(result);
                }
                Err(e) => {
                    self.circuit_breaker.record_failure();

                    if attempt < self.retry_policy.max_retries && self.retry_policy.should_retry(&e)
                    {
                        let delay = self.retry_policy.compute_delay(attempt);
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        last_error = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RpcError::internal("Retry exhausted")))
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert!(policy.exponential_backoff);
        assert_eq!(policy.backoff_multiplier, 2.0);
    }

    #[test]
    fn test_retry_policy_no_retry() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_retries, 0);
    }

    #[test]
    fn test_retry_policy_compute_delay_constant() {
        let policy = RetryPolicy::default()
            .with_exponential_backoff(false)
            .with_jitter(0.0);

        let delay0 = policy.compute_delay(0);
        let delay1 = policy.compute_delay(1);
        let delay2 = policy.compute_delay(2);

        assert_eq!(delay0, policy.base_delay);
        assert_eq!(delay1, policy.base_delay);
        assert_eq!(delay2, policy.base_delay);
    }

    #[test]
    fn test_retry_policy_compute_delay_exponential() {
        let policy = RetryPolicy::default()
            .with_base_delay(Duration::from_millis(100))
            .with_jitter(0.0);

        let delay0 = policy.compute_delay(0);
        let delay1 = policy.compute_delay(1);
        let delay2 = policy.compute_delay(2);

        assert_eq!(delay0, Duration::from_millis(100)); // 100 * 2^0
        assert_eq!(delay1, Duration::from_millis(200)); // 100 * 2^1
        assert_eq!(delay2, Duration::from_millis(400)); // 100 * 2^2
    }

    #[test]
    fn test_retry_policy_max_delay_cap() {
        let policy = RetryPolicy::default()
            .with_max_retries(10) // Allow more retries
            .with_base_delay(Duration::from_secs(1))
            .with_max_delay(Duration::from_secs(5))
            .with_jitter(0.0);

        let delay5 = policy.compute_delay(5); // Would be 32 seconds without cap
        assert_eq!(delay5, Duration::from_secs(5));
    }

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(5, Duration::from_secs(30));
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_closed());
        assert!(!cb.is_open());
        assert!(cb.should_allow_request());
    }

    #[test]
    fn test_circuit_breaker_opens_on_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 1);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 2);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_rejects_when_open() {
        let cb = CircuitBreaker::new(1, Duration::from_secs(60)); // Long timeout

        cb.record_failure(); // Opens the circuit
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.should_allow_request()); // Should reject
    }

    #[test]
    fn test_circuit_breaker_success_resets_failure_count() {
        let cb = CircuitBreaker::new(5, Duration::from_secs(30));

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count(), 2);

        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_manual_reset() {
        let cb = CircuitBreaker::new(1, Duration::from_secs(60));

        cb.record_failure(); // Opens
        assert!(cb.is_open());

        cb.reset();
        assert!(cb.is_closed());
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_force_open() {
        let cb = CircuitBreaker::new(100, Duration::from_secs(30));

        assert!(cb.is_closed());
        cb.force_open();
        assert!(cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_half_open_success() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1)); // Very short timeout

        cb.record_failure(); // Opens
        assert!(cb.is_open());

        // Wait for reset timeout
        std::thread::sleep(Duration::from_millis(10));

        // Should transition to half-open
        assert!(cb.should_allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Success should close
        cb.record_success();
        assert!(cb.is_closed());
    }

    #[test]
    fn test_circuit_breaker_half_open_failure() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));

        cb.record_failure(); // Opens

        // Wait for reset timeout
        std::thread::sleep(Duration::from_millis(10));

        cb.should_allow_request(); // Transitions to half-open
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Failure should reopen
        cb.record_failure();
        assert!(cb.is_open());
    }

    #[tokio::test]
    async fn test_retry_executor_success() {
        let executor = RetryExecutor::new(
            RetryPolicy::default(),
            Arc::new(CircuitBreaker::new(5, Duration::from_secs(30))),
        );

        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let result = executor
            .execute(|| {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, RpcError>(42)
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_executor_retries_on_failure() {
        let executor = RetryExecutor::new(
            RetryPolicy::default()
                .with_max_retries(3)
                .with_base_delay(Duration::from_millis(1)),
            Arc::new(CircuitBreaker::new(10, Duration::from_secs(30))),
        );

        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let result = executor
            .execute(|| {
                let count = call_count_clone.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                    if n < 3 {
                        Err(RpcError::connection("temporary failure"))
                    } else {
                        Ok::<_, RpcError>(42)
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 3); // Initial + 2 retries
    }

    #[tokio::test]
    async fn test_retry_executor_circuit_breaker_open() {
        let cb = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
        cb.force_open();

        let executor = RetryExecutor::new(RetryPolicy::default(), cb);

        let result = executor.execute(|| async { Ok::<_, RpcError>(42) }).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Circuit breaker"));
    }
}
