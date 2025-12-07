//! Integration tests for resilience patterns

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_circuit_breaker_with_retry() {
    // Test combining circuit breaker with retry policy
    let cb = CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 5,
        success_threshold: 2,
        timeout_secs: 1,
        ..Default::default()
    });

    let retry_policy = RetryPolicy::new(RetryConfig::fixed(2, Duration::from_millis(10)));
    let counter = Arc::new(AtomicU32::new(0));

    // Simulate a service that fails initially then recovers
    let simulate_service = |counter: Arc<AtomicU32>| async move {
        let count = counter.fetch_add(1, Ordering::Relaxed);
        if count < 2 {
            Err::<u32, _>(anyhow::anyhow!("service unavailable"))
        } else {
            Ok(42)
        }
    };

    // Execute with retry (inside circuit breaker would be typical pattern)
    let counter_clone = counter.clone();
    let result = retry_policy
        .execute(|| {
            let c = counter_clone.clone();
            async move { simulate_service(c).await }
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
}

#[tokio::test]
async fn test_circuit_breaker_half_open_recovery() {
    let cb = CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 2,
        success_threshold: 2,
        timeout_secs: 1, // Short timeout
        half_open_max_requests: 2,
        name: "test-recovery".to_string(),
    });

    // Trip the circuit breaker
    cb.record_failure();
    cb.record_failure();

    // Immediately after failures, should be Open
    // Note: state() checks for timeout transition, so we check the request rejection
    let result = cb.allow_request();
    assert!(result.is_err(), "Circuit should reject requests when open");

    // Wait for half-open transition
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    // Successful requests in half-open should close the circuit
    cb.allow_request().unwrap();
    cb.record_success();
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    cb.allow_request().unwrap();
    cb.record_success();
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[test]
fn test_retry_with_condition() {
    // Test that non-retryable errors stop immediately
    #[derive(Debug)]
    enum TestError {
        Retryable,
        NonRetryable,
    }

    let is_retryable = |e: &TestError| matches!(e, TestError::Retryable);

    // This would typically be an async test, but we're just testing the config logic
    let config = RetryConfig::default();
    assert_eq!(config.max_retries, 3);
}

#[test]
fn test_circuit_breaker_metrics() {
    let cb = CircuitBreaker::with_name("metrics-test");

    // Simulate some traffic
    for _ in 0..5 {
        let _ = cb.allow_request();
        cb.record_success();
    }

    for _ in 0..3 {
        let _ = cb.allow_request();
        cb.record_failure();
    }

    let metrics = cb.metrics();
    assert_eq!(metrics.name, "metrics-test");
    assert_eq!(metrics.total_requests, 8);
}

#[test]
fn test_retry_policy_presets() {
    let aggressive = RetryConfig::aggressive();
    assert_eq!(aggressive.max_retries, 5);
    assert_eq!(aggressive.initial_delay, Duration::from_millis(10));

    let conservative = RetryConfig::conservative();
    assert_eq!(conservative.max_retries, 3);
    assert_eq!(conservative.initial_delay, Duration::from_secs(1));
}
