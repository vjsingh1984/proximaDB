//! Retry Pattern Implementation with Exponential Backoff
//!
//! Provides configurable retry logic for transient failures:
//! - Exponential backoff with jitter
//! - Maximum attempts
//! - Configurable delay and max delay
//! - Retry condition functions

use rand::Rng;
use std::future::Future;
use std::time::Duration;
use thiserror::Error;

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (total attempts = max_retries + 1)
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff (typically 2.0)
    pub backoff_multiplier: f64,
    /// Whether to add random jitter to delays
    pub jitter: bool,
    /// Jitter factor (0.0-1.0), e.g., 0.1 means ±10% jitter
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_factor: 0.1,
        }
    }
}

impl RetryConfig {
    /// Create a config with exponential backoff
    pub fn exponential(max_retries: u32, initial_delay: Duration) -> Self {
        Self {
            max_retries,
            initial_delay,
            ..Default::default()
        }
    }

    /// Create a config with fixed delay (no backoff)
    pub fn fixed(max_retries: u32, delay: Duration) -> Self {
        Self {
            max_retries,
            initial_delay: delay,
            max_delay: delay,
            backoff_multiplier: 1.0,
            jitter: false,
            jitter_factor: 0.0,
        }
    }

    /// Create a config for aggressive retry (fast initial, short max)
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_factor: 0.2,
        }
    }

    /// Create a config for conservative retry (slow, longer delays)
    pub fn conservative() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_factor: 0.1,
        }
    }
}

/// Error types for retry operations
#[derive(Error, Debug)]
pub enum RetryError<E> {
    /// All retry attempts exhausted
    #[error("All {attempts} retry attempts exhausted")]
    RetriesExhausted {
        attempts: u32,
        #[source]
        last_error: E,
    },
}

/// Retry policy for executing operations with automatic retry
pub struct RetryPolicy {
    config: RetryConfig,
}

impl RetryPolicy {
    /// Create a new retry policy with the given configuration
    pub fn new(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Create a retry policy with exponential backoff
    pub fn exponential_backoff(max_retries: u32, initial_delay: Duration) -> Self {
        Self::new(RetryConfig::exponential(max_retries, initial_delay))
    }

    /// Create a retry policy with fixed delay
    pub fn fixed_delay(max_retries: u32, delay: Duration) -> Self {
        Self::new(RetryConfig::fixed(max_retries, delay))
    }

    /// Calculate the delay for a given attempt number
    fn calculate_delay(&self, attempt: u32) -> Duration {
        let base_delay = self.config.initial_delay.as_millis() as f64
            * self.config.backoff_multiplier.powi(attempt as i32);

        let capped_delay = base_delay.min(self.config.max_delay.as_millis() as f64);

        let final_delay = if self.config.jitter {
            let mut rng = rand::thread_rng();
            let jitter_range = capped_delay * self.config.jitter_factor;
            let jitter = rng.gen_range(-jitter_range..=jitter_range);
            (capped_delay + jitter).max(0.0)
        } else {
            capped_delay
        };

        Duration::from_millis(final_delay as u64)
    }

    /// Execute an async operation with retry
    pub async fn execute<F, Fut, T, E>(&self, mut f: F) -> Result<T, RetryError<E>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: std::fmt::Debug,
    {
        let mut last_error = None;
        let total_attempts = self.config.max_retries + 1;

        for attempt in 0..total_attempts {
            match f().await {
                Ok(result) => {
                    if attempt > 0 {
                        tracing::info!(
                            attempt = attempt + 1,
                            total_attempts = total_attempts,
                            "Operation succeeded after retry"
                        );
                    }
                    return Ok(result);
                }
                Err(e) => {
                    last_error = Some(e);

                    if attempt < self.config.max_retries {
                        let delay = self.calculate_delay(attempt);
                        tracing::warn!(
                            attempt = attempt + 1,
                            total_attempts = total_attempts,
                            delay_ms = delay.as_millis(),
                            "Operation failed, retrying after delay"
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        Err(RetryError::RetriesExhausted {
            attempts: total_attempts,
            last_error: last_error.expect("At least one attempt must have been made"),
        })
    }

    /// Execute an async operation with retry and a condition for retryable errors
    pub async fn execute_with_condition<F, Fut, T, E, C>(
        &self,
        mut f: F,
        should_retry: C,
    ) -> Result<T, RetryError<E>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: std::fmt::Debug,
        C: Fn(&E) -> bool,
    {
        let mut last_error = None;
        let total_attempts = self.config.max_retries + 1;

        for attempt in 0..total_attempts {
            match f().await {
                Ok(result) => {
                    if attempt > 0 {
                        tracing::info!(
                            attempt = attempt + 1,
                            total_attempts = total_attempts,
                            "Operation succeeded after retry"
                        );
                    }
                    return Ok(result);
                }
                Err(e) => {
                    let can_retry = attempt < self.config.max_retries && should_retry(&e);

                    if can_retry {
                        let delay = self.calculate_delay(attempt);
                        tracing::warn!(
                            attempt = attempt + 1,
                            total_attempts = total_attempts,
                            delay_ms = delay.as_millis(),
                            "Retryable error, retrying after delay"
                        );
                        last_error = Some(e);
                        tokio::time::sleep(delay).await;
                    } else {
                        // Non-retryable error or exhausted retries
                        return Err(RetryError::RetriesExhausted {
                            attempts: attempt + 1,
                            last_error: e,
                        });
                    }
                }
            }
        }

        Err(RetryError::RetriesExhausted {
            attempts: total_attempts,
            last_error: last_error.expect("At least one attempt must have been made"),
        })
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(RetryConfig::default())
    }
}

/// Helper trait for checking if an error is retryable
pub trait RetryableError {
    fn is_retryable(&self) -> bool;
}

// Implement for std::io::Error
impl RetryableError for std::io::Error {
    fn is_retryable(&self) -> bool {
        use std::io::ErrorKind;
        matches!(
            self.kind(),
            ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::TimedOut
                | ErrorKind::Interrupted
                | ErrorKind::WouldBlock
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay, Duration::from_millis(100));
        assert_eq!(config.backoff_multiplier, 2.0);
        assert!(config.jitter);
    }

    #[test]
    fn test_retry_config_fixed() {
        let config = RetryConfig::fixed(5, Duration::from_millis(500));
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_delay, Duration::from_millis(500));
        assert_eq!(config.backoff_multiplier, 1.0);
        assert!(!config.jitter);
    }

    #[test]
    fn test_calculate_delay_exponential() {
        let policy = RetryPolicy::new(RetryConfig {
            initial_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(10),
            jitter: false,
            ..Default::default()
        });

        assert_eq!(policy.calculate_delay(0), Duration::from_millis(100));
        assert_eq!(policy.calculate_delay(1), Duration::from_millis(200));
        assert_eq!(policy.calculate_delay(2), Duration::from_millis(400));
        assert_eq!(policy.calculate_delay(3), Duration::from_millis(800));
    }

    #[test]
    fn test_calculate_delay_capped() {
        let policy = RetryPolicy::new(RetryConfig {
            initial_delay: Duration::from_secs(1),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(5),
            jitter: false,
            ..Default::default()
        });

        // Should be capped at 5 seconds
        assert_eq!(policy.calculate_delay(10), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_retry_succeeds_first_attempt() {
        let policy = RetryPolicy::default();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<u32, RetryError<&str>> = policy
            .execute(|| {
                let c = counter_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Ok::<_, &str>(42)
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let policy = RetryPolicy::new(RetryConfig::fixed(3, Duration::from_millis(10)));
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<u32, RetryError<&str>> = policy
            .execute(|| {
                let c = counter_clone.clone();
                async move {
                    let count = c.fetch_add(1, Ordering::Relaxed);
                    if count < 2 {
                        Err("transient error")
                    } else {
                        Ok(42)
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let policy = RetryPolicy::new(RetryConfig::fixed(2, Duration::from_millis(10)));
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result: Result<u32, RetryError<&str>> = policy
            .execute(|| {
                let c = counter_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Err::<u32, _>("persistent error")
                }
            })
            .await;

        assert!(result.is_err());
        match result {
            Err(RetryError::RetriesExhausted { attempts, .. }) => {
                assert_eq!(attempts, 3); // max_retries + 1
            }
            Ok(_) => unreachable!("Expected error, got Ok"),
        }
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }
}
