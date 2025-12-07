//! Health Aggregator for Enterprise Dependency Tracking
//!
//! Provides centralized health monitoring for all system dependencies,
//! integrating with circuit breakers to provide accurate health status.

use super::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Health status for a dependency
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DependencyHealth {
    /// Dependency is healthy and responding
    Healthy,
    /// Dependency is experiencing issues but still functional
    Degraded,
    /// Dependency is unavailable
    Unhealthy,
    /// Dependency health is unknown (not yet checked)
    Unknown,
}

impl From<CircuitState> for DependencyHealth {
    fn from(state: CircuitState) -> Self {
        match state {
            CircuitState::Closed => DependencyHealth::Healthy,
            CircuitState::HalfOpen => DependencyHealth::Degraded,
            CircuitState::Open => DependencyHealth::Unhealthy,
        }
    }
}

/// Information about a tracked dependency
#[derive(Debug, Clone, Serialize)]
pub struct DependencyInfo {
    /// Dependency name
    pub name: String,
    /// Current health status
    pub health: DependencyHealth,
    /// Whether this is a critical dependency
    pub critical: bool,
    /// Last successful check time
    pub last_success: Option<u64>,
    /// Last failure time
    pub last_failure: Option<u64>,
    /// Number of consecutive failures
    pub consecutive_failures: u32,
    /// Average response time (ms)
    pub avg_response_time_ms: u64,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Tracked dependency with circuit breaker
struct TrackedDependency {
    info: DependencyInfo,
    circuit_breaker: Arc<CircuitBreaker>,
    response_times: Vec<u64>,
    max_response_samples: usize,
}

/// Health Aggregator for enterprise dependency tracking
pub struct HealthAggregator {
    /// Tracked dependencies
    dependencies: RwLock<HashMap<String, TrackedDependency>>,
    /// Overall health status
    overall_health: RwLock<DependencyHealth>,
    /// Configuration
    config: HealthAggregatorConfig,
}

/// Configuration for the health aggregator
#[derive(Debug, Clone)]
pub struct HealthAggregatorConfig {
    /// Default circuit breaker config for new dependencies
    pub default_circuit_breaker: CircuitBreakerConfig,
    /// Maximum response time samples to keep for averaging
    pub max_response_samples: usize,
    /// Threshold for degraded status (% of degraded/unhealthy dependencies)
    pub degraded_threshold: f32,
    /// Threshold for unhealthy status (% of unhealthy critical dependencies)
    pub unhealthy_threshold: f32,
}

impl Default for HealthAggregatorConfig {
    fn default() -> Self {
        Self {
            default_circuit_breaker: CircuitBreakerConfig::default(),
            max_response_samples: 100,
            degraded_threshold: 0.3,  // 30% degraded = overall degraded
            unhealthy_threshold: 0.5, // 50% critical unhealthy = overall unhealthy
        }
    }
}

impl HealthAggregator {
    /// Create a new health aggregator with default configuration
    pub fn new() -> Arc<Self> {
        Self::with_config(HealthAggregatorConfig::default())
    }

    /// Create a new health aggregator with custom configuration
    pub fn with_config(config: HealthAggregatorConfig) -> Arc<Self> {
        Arc::new(Self {
            dependencies: RwLock::new(HashMap::new()),
            overall_health: RwLock::new(DependencyHealth::Unknown),
            config,
        })
    }

    /// Register a dependency for health tracking
    pub fn register_dependency(
        &self,
        name: impl Into<String>,
        critical: bool,
        circuit_breaker_config: Option<CircuitBreakerConfig>,
    ) -> Arc<CircuitBreaker> {
        let name = name.into();
        let cb_config = circuit_breaker_config.unwrap_or_else(|| {
            let mut config = self.config.default_circuit_breaker.clone();
            config.name = name.clone();
            config
        });

        let circuit_breaker = CircuitBreaker::new(cb_config);

        let tracked = TrackedDependency {
            info: DependencyInfo {
                name: name.clone(),
                health: DependencyHealth::Unknown,
                critical,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                avg_response_time_ms: 0,
                metadata: HashMap::new(),
            },
            circuit_breaker: circuit_breaker.clone(),
            response_times: Vec::new(),
            max_response_samples: self.config.max_response_samples,
        };

        self.dependencies.write().insert(name, tracked);
        circuit_breaker
    }

    /// Record a successful health check for a dependency
    pub fn record_success(&self, name: &str, response_time_ms: u64) {
        let mut deps = self.dependencies.write();
        if let Some(dep) = deps.get_mut(name) {
            dep.circuit_breaker.record_success();
            dep.info.health = DependencyHealth::from(dep.circuit_breaker.state());
            dep.info.last_success = Some(current_timestamp());
            dep.info.consecutive_failures = 0;

            // Update response time average
            dep.response_times.push(response_time_ms);
            if dep.response_times.len() > dep.max_response_samples {
                dep.response_times.remove(0);
            }
            dep.info.avg_response_time_ms = dep.response_times.iter().sum::<u64>()
                / dep.response_times.len() as u64;
        }
        drop(deps);
        self.update_overall_health();
    }

    /// Record a failed health check for a dependency
    pub fn record_failure(&self, name: &str) {
        let mut deps = self.dependencies.write();
        if let Some(dep) = deps.get_mut(name) {
            dep.circuit_breaker.record_failure();
            dep.info.health = DependencyHealth::from(dep.circuit_breaker.state());
            dep.info.last_failure = Some(current_timestamp());
            dep.info.consecutive_failures += 1;
        }
        drop(deps);
        self.update_overall_health();
    }

    /// Update overall health based on dependency states
    fn update_overall_health(&self) {
        let deps = self.dependencies.read();
        let total = deps.len() as f32;
        if total == 0.0 {
            *self.overall_health.write() = DependencyHealth::Unknown;
            return;
        }

        let mut unhealthy_critical = 0;
        let mut degraded_or_worse = 0;

        for dep in deps.values() {
            match dep.info.health {
                DependencyHealth::Unhealthy => {
                    if dep.info.critical {
                        unhealthy_critical += 1;
                    }
                    degraded_or_worse += 1;
                }
                DependencyHealth::Degraded => {
                    degraded_or_worse += 1;
                }
                _ => {}
            }
        }

        let critical_count = deps.values().filter(|d| d.info.critical).count() as f32;
        let unhealthy_ratio = if critical_count > 0.0 {
            unhealthy_critical as f32 / critical_count
        } else {
            0.0
        };
        let degraded_ratio = degraded_or_worse as f32 / total;

        let overall = if unhealthy_ratio >= self.config.unhealthy_threshold {
            DependencyHealth::Unhealthy
        } else if degraded_ratio >= self.config.degraded_threshold {
            DependencyHealth::Degraded
        } else {
            DependencyHealth::Healthy
        };

        *self.overall_health.write() = overall;
    }

    /// Get the overall health status
    pub fn overall_health(&self) -> DependencyHealth {
        *self.overall_health.read()
    }

    /// Get health information for all dependencies
    pub fn get_all_dependencies(&self) -> Vec<DependencyInfo> {
        self.dependencies
            .read()
            .values()
            .map(|d| d.info.clone())
            .collect()
    }

    /// Get health information for a specific dependency
    pub fn get_dependency(&self, name: &str) -> Option<DependencyInfo> {
        self.dependencies.read().get(name).map(|d| d.info.clone())
    }

    /// Get the circuit breaker for a dependency
    pub fn get_circuit_breaker(&self, name: &str) -> Option<Arc<CircuitBreaker>> {
        self.dependencies
            .read()
            .get(name)
            .map(|d| d.circuit_breaker.clone())
    }

    /// Check if the system is healthy enough to serve traffic
    pub fn is_ready(&self) -> bool {
        matches!(
            self.overall_health(),
            DependencyHealth::Healthy | DependencyHealth::Degraded
        )
    }

    /// Get a summary of the health status
    pub fn summary(&self) -> HealthSummary {
        let deps = self.dependencies.read();
        let mut healthy = 0;
        let mut degraded = 0;
        let mut unhealthy = 0;
        let mut unknown = 0;

        for dep in deps.values() {
            match dep.info.health {
                DependencyHealth::Healthy => healthy += 1,
                DependencyHealth::Degraded => degraded += 1,
                DependencyHealth::Unhealthy => unhealthy += 1,
                DependencyHealth::Unknown => unknown += 1,
            }
        }

        HealthSummary {
            overall: self.overall_health(),
            total_dependencies: deps.len(),
            healthy,
            degraded,
            unhealthy,
            unknown,
            timestamp: current_timestamp(),
        }
    }

    /// Set custom metadata for a dependency
    pub fn set_metadata(&self, name: &str, key: impl Into<String>, value: impl Into<String>) {
        let mut deps = self.dependencies.write();
        if let Some(dep) = deps.get_mut(name) {
            dep.info.metadata.insert(key.into(), value.into());
        }
    }
}

impl Default for HealthAggregator {
    fn default() -> Self {
        Self {
            dependencies: RwLock::new(HashMap::new()),
            overall_health: RwLock::new(DependencyHealth::Unknown),
            config: HealthAggregatorConfig::default(),
        }
    }
}

/// Summary of health status across all dependencies
#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    /// Overall health status
    pub overall: DependencyHealth,
    /// Total number of tracked dependencies
    pub total_dependencies: usize,
    /// Number of healthy dependencies
    pub healthy: usize,
    /// Number of degraded dependencies
    pub degraded: usize,
    /// Number of unhealthy dependencies
    pub unhealthy: usize,
    /// Number of unknown status dependencies
    pub unknown: usize,
    /// Timestamp of this summary
    pub timestamp: u64,
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_aggregator_creation() {
        let aggregator = HealthAggregator::new();
        assert_eq!(aggregator.overall_health(), DependencyHealth::Unknown);
    }

    #[test]
    fn test_register_dependency() {
        let aggregator = HealthAggregator::new();
        let cb = aggregator.register_dependency("database", true, None);

        assert!(aggregator.get_dependency("database").is_some());
        let info = aggregator.get_dependency("database").unwrap();
        assert_eq!(info.name, "database");
        assert!(info.critical);
        assert_eq!(info.health, DependencyHealth::Unknown);
    }

    #[test]
    fn test_record_success() {
        let aggregator = HealthAggregator::new();
        aggregator.register_dependency("api", false, None);

        aggregator.record_success("api", 50);
        let info = aggregator.get_dependency("api").unwrap();
        assert_eq!(info.health, DependencyHealth::Healthy);
        assert_eq!(info.consecutive_failures, 0);
        assert!(info.last_success.is_some());
    }

    #[test]
    fn test_record_failure() {
        let aggregator = HealthAggregator::new();
        aggregator.register_dependency("api", false, None);

        for _ in 0..5 {
            aggregator.record_failure("api");
        }

        let info = aggregator.get_dependency("api").unwrap();
        assert_eq!(info.health, DependencyHealth::Unhealthy);
        assert_eq!(info.consecutive_failures, 5);
    }

    #[test]
    fn test_overall_health_calculation() {
        let aggregator = HealthAggregator::new();

        // Register 3 dependencies, 2 critical
        aggregator.register_dependency("db", true, None);
        aggregator.register_dependency("cache", true, None);
        aggregator.register_dependency("metrics", false, None);

        // All healthy
        aggregator.record_success("db", 10);
        aggregator.record_success("cache", 5);
        aggregator.record_success("metrics", 2);
        assert_eq!(aggregator.overall_health(), DependencyHealth::Healthy);

        // One critical unhealthy (fail enough to open circuit)
        let cb_config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        };
        let aggregator2 = HealthAggregator::new();
        aggregator2.register_dependency("db", true, Some(cb_config.clone()));
        aggregator2.register_dependency("cache", true, Some(cb_config.clone()));

        aggregator2.record_success("cache", 5);
        aggregator2.record_failure("db");
        aggregator2.record_failure("db");

        // 50% of critical dependencies unhealthy
        assert_eq!(aggregator2.overall_health(), DependencyHealth::Unhealthy);
    }

    #[test]
    fn test_summary() {
        let aggregator = HealthAggregator::new();
        aggregator.register_dependency("a", false, None);
        aggregator.register_dependency("b", false, None);

        aggregator.record_success("a", 10);

        let summary = aggregator.summary();
        assert_eq!(summary.total_dependencies, 2);
        assert_eq!(summary.healthy, 1);
        assert_eq!(summary.unknown, 1);
    }

    #[test]
    fn test_is_ready() {
        let aggregator = HealthAggregator::new();
        aggregator.register_dependency("api", true, None);
        aggregator.record_success("api", 10);

        assert!(aggregator.is_ready());
    }
}
