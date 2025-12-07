//! # Resilience Module - Enterprise Patterns for Reliability
//!
//! This module provides resilience patterns for building robust, fault-tolerant systems:
//!
//! - **Circuit Breaker**: Prevents cascading failures by stopping requests to failing services
//! - **Retry with Backoff**: Exponential backoff retry logic for transient failures
//! - **Bulkhead**: Isolation pattern for resource protection (planned)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::core::resilience::{CircuitBreaker, RetryPolicy, CircuitBreakerConfig};
//!
//! // Create a circuit breaker for an external service
//! let cb = CircuitBreaker::new(CircuitBreakerConfig {
//!     failure_threshold: 5,
//!     success_threshold: 3,
//!     timeout_secs: 30,
//!     ..Default::default()
//! });
//!
//! // Execute with circuit breaker protection
//! let result = cb.execute(|| async {
//!     external_service_call().await
//! }).await;
//!
//! // Execute with retry and backoff
//! let policy = RetryPolicy::exponential_backoff(3, Duration::from_millis(100));
//! let result = policy.execute(|| async {
//!     transient_operation().await
//! }).await;
//! ```

mod circuit_breaker;
mod retry;
mod health_aggregator;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError, CircuitState};
pub use retry::{RetryPolicy, RetryConfig, RetryError};
pub use health_aggregator::{
    HealthAggregator, HealthAggregatorConfig, DependencyHealth, DependencyInfo, HealthSummary,
};

#[cfg(test)]
mod tests;
