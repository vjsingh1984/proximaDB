//! LLM Metrics Collection and Monitoring
//!
//! Comprehensive metrics collection for LLM operations including performance,
//! cost tracking, error rates, and provider health monitoring.

use crate::ai::llm_integration::types::{LLMError, LLMProvider};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio::sync::RwLock;

/// Comprehensive metrics for LLM operations
#[derive(Debug)]
pub struct LLMMetrics {
    provider_metrics: Arc<RwLock<HashMap<LLMProvider, ProviderMetrics>>>,
    global_metrics: Arc<GlobalMetrics>,
}

/// Metrics for a specific LLM provider
#[derive(Debug)]
pub struct ProviderMetrics {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub total_tokens_used: AtomicU64,
    pub total_cost_usd: AtomicU64, // Stored as cents for precision
    pub total_response_time_ms: AtomicU64,
    pub rate_limit_hits: AtomicU32,
    pub authentication_failures: AtomicU32,
    pub last_success: RwLock<Option<DateTime<Utc>>>,
    pub last_failure: RwLock<Option<DateTime<Utc>>>,
    pub error_breakdown: Arc<RwLock<HashMap<String, u32>>>,
}

/// Global metrics across all providers
#[derive(Debug, Default)]
pub struct GlobalMetrics {
    pub total_requests: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub fallback_activations: AtomicU64,
    pub all_providers_failures: AtomicU32,
}

/// Snapshot of metrics at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMetricsSnapshot {
    pub timestamp: DateTime<Utc>,
    pub provider_stats: HashMap<LLMProvider, ProviderStats>,
    pub global_stats: GlobalStats,
    pub performance_summary: PerformanceSummary,
}

/// Statistics for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStats {
    pub total_requests: u64,
    pub success_rate: f64,
    pub average_response_time_ms: f64,
    pub total_tokens_used: u64,
    pub estimated_cost_usd: f64,
    pub rate_limit_hit_rate: f64,
    pub last_success: Option<DateTime<Utc>>,
    pub last_failure: Option<DateTime<Utc>>,
    pub common_errors: Vec<(String, u32)>,
}

/// Global statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalStats {
    pub total_requests: u64,
    pub cache_hit_rate: f64,
    pub fallback_rate: f64,
    pub overall_success_rate: f64,
    pub all_providers_failure_rate: f64,
}

/// Performance summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSummary {
    pub fastest_provider: Option<LLMProvider>,
    pub most_reliable_provider: Option<LLMProvider>,
    pub most_cost_effective_provider: Option<LLMProvider>,
    pub recommended_primary_provider: Option<LLMProvider>,
}

impl LLMMetrics {
    /// Create new metrics collector
    pub fn new() -> Self {
        Self {
            provider_metrics: Arc::new(RwLock::new(HashMap::new())),
            global_metrics: Arc::new(GlobalMetrics::default()),
        }
    }

    /// Record a successful LLM query
    pub async fn record_success(&self, provider: &LLMProvider, response_time_ms: u64) {
        // Update global metrics
        self.global_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);

        // Update provider-specific metrics
        let mut provider_metrics = self.provider_metrics.write().await;
        let metrics = provider_metrics.entry(provider.clone()).or_default();

        metrics.total_requests.fetch_add(1, Ordering::Relaxed);
        metrics.successful_requests.fetch_add(1, Ordering::Relaxed);
        metrics
            .total_response_time_ms
            .fetch_add(response_time_ms, Ordering::Relaxed);

        // Update last success timestamp
        let mut last_success = metrics.last_success.write().await;
        *last_success = Some(Utc::now());
    }

    /// Record a failed LLM query
    pub async fn record_failure(&self, provider: &LLMProvider, error: &LLMError) {
        // Update global metrics
        self.global_metrics
            .total_requests
            .fetch_add(1, Ordering::Relaxed);

        // Update provider-specific metrics
        let mut provider_metrics = self.provider_metrics.write().await;
        let metrics = provider_metrics.entry(provider.clone()).or_default();

        metrics.total_requests.fetch_add(1, Ordering::Relaxed);
        metrics.failed_requests.fetch_add(1, Ordering::Relaxed);

        // Update last failure timestamp
        let mut last_failure = metrics.last_failure.write().await;
        *last_failure = Some(Utc::now());

        // Track error types
        let error_type = format!("{:?}", error);
        let mut error_breakdown = metrics.error_breakdown.write().await;
        *error_breakdown.entry(error_type).or_insert(0) += 1;

        // Update specific error counters
        match error {
            LLMError::RateLimitExceeded { .. } => {
                metrics.rate_limit_hits.fetch_add(1, Ordering::Relaxed);
            }
            LLMError::AuthenticationFailed { .. } => {
                metrics
                    .authentication_failures
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// Record token usage and cost
    pub async fn record_token_usage(&self, provider: &LLMProvider, tokens: u64, cost_usd: f64) {
        let mut provider_metrics = self.provider_metrics.write().await;
        let metrics = provider_metrics.entry(provider.clone()).or_default();

        metrics
            .total_tokens_used
            .fetch_add(tokens, Ordering::Relaxed);
        // Store cost in millicents (0.001 cents) for atomic operations to preserve precision
        let cost_millicents = (cost_usd * 100000.0) as u64;
        metrics
            .total_cost_usd
            .fetch_add(cost_millicents, Ordering::Relaxed);
    }

    /// Record rate limit exceeded
    pub async fn record_rate_limit_exceeded(&self, provider: &LLMProvider) {
        let mut provider_metrics = self.provider_metrics.write().await;
        let metrics = provider_metrics.entry(provider.clone()).or_default();
        metrics.rate_limit_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record cache hit
    pub async fn record_cache_hit(&self) {
        self.global_metrics
            .cache_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record cache miss
    pub async fn record_cache_miss(&self) {
        self.global_metrics
            .cache_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record fallback activation
    pub async fn record_fallback_activation(&self) {
        self.global_metrics
            .fallback_activations
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record all providers failed scenario
    pub async fn record_all_providers_failed(&self) {
        self.global_metrics
            .all_providers_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get comprehensive metrics snapshot
    pub async fn get_snapshot(&self) -> LlmMetricsSnapshot {
        let provider_metrics = self.provider_metrics.read().await;
        let mut provider_stats = HashMap::new();

        for (provider, metrics) in provider_metrics.iter() {
            let total_requests = metrics.total_requests.load(Ordering::Relaxed);
            let successful_requests = metrics.successful_requests.load(Ordering::Relaxed);
            let _failed_requests = metrics.failed_requests.load(Ordering::Relaxed);
            let total_response_time = metrics.total_response_time_ms.load(Ordering::Relaxed);
            let rate_limit_hits = metrics.rate_limit_hits.load(Ordering::Relaxed);

            let success_rate = if total_requests > 0 {
                (successful_requests as f64) / (total_requests as f64) * 100.0
            } else {
                0.0
            };

            let average_response_time = if successful_requests > 0 {
                (total_response_time as f64) / (successful_requests as f64)
            } else {
                0.0
            };

            let rate_limit_hit_rate = if total_requests > 0 {
                (rate_limit_hits as f64) / (total_requests as f64) * 100.0
            } else {
                0.0
            };

            let cost_millicents = metrics.total_cost_usd.load(Ordering::Relaxed);
            let estimated_cost_usd = (cost_millicents as f64) / 100000.0;

            let last_success = *metrics.last_success.read().await;
            let last_failure = *metrics.last_failure.read().await;

            // Get top 5 error types
            let error_breakdown = metrics.error_breakdown.read().await;
            let mut common_errors: Vec<(String, u32)> = error_breakdown
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            common_errors.sort_by_key(|e| std::cmp::Reverse(e.1));
            common_errors.truncate(5);

            provider_stats.insert(
                provider.clone(),
                ProviderStats {
                    total_requests,
                    success_rate,
                    average_response_time_ms: average_response_time,
                    total_tokens_used: metrics.total_tokens_used.load(Ordering::Relaxed),
                    estimated_cost_usd,
                    rate_limit_hit_rate,
                    last_success,
                    last_failure,
                    common_errors,
                },
            );
        }

        // Calculate global stats
        let global_total_requests = self.global_metrics.total_requests.load(Ordering::Relaxed);
        let cache_hits = self.global_metrics.cache_hits.load(Ordering::Relaxed);
        let cache_misses = self.global_metrics.cache_misses.load(Ordering::Relaxed);
        let fallback_activations = self
            .global_metrics
            .fallback_activations
            .load(Ordering::Relaxed);
        let all_providers_failures = self
            .global_metrics
            .all_providers_failures
            .load(Ordering::Relaxed);

        let cache_hit_rate = if cache_hits + cache_misses > 0 {
            (cache_hits as f64) / ((cache_hits + cache_misses) as f64) * 100.0
        } else {
            0.0
        };

        let fallback_rate = if global_total_requests > 0 {
            (fallback_activations as f64) / (global_total_requests as f64) * 100.0
        } else {
            0.0
        };

        let total_successful_requests: u64 = provider_stats
            .values()
            .map(|stats| (stats.total_requests as f64 * stats.success_rate / 100.0) as u64)
            .sum();

        let overall_success_rate = if global_total_requests > 0 {
            (total_successful_requests as f64) / (global_total_requests as f64) * 100.0
        } else {
            0.0
        };

        let all_providers_failure_rate = if global_total_requests > 0 {
            (all_providers_failures as f64) / (global_total_requests as f64) * 100.0
        } else {
            0.0
        };

        let global_stats = GlobalStats {
            total_requests: global_total_requests,
            cache_hit_rate,
            fallback_rate,
            overall_success_rate,
            all_providers_failure_rate,
        };

        // Calculate performance summary
        let performance_summary = self.calculate_performance_summary(&provider_stats);

        LlmMetricsSnapshot {
            timestamp: Utc::now(),
            provider_stats,
            global_stats,
            performance_summary,
        }
    }

    /// Calculate performance summary and recommendations
    fn calculate_performance_summary(
        &self,
        provider_stats: &HashMap<LLMProvider, ProviderStats>,
    ) -> PerformanceSummary {
        let mut fastest_provider = None;
        let mut fastest_time = f64::INFINITY;

        let mut most_reliable_provider = None;
        let mut highest_success_rate = 0.0;

        let mut most_cost_effective_provider = None;
        let mut lowest_cost_per_token = f64::INFINITY;

        for (provider, stats) in provider_stats {
            // Find fastest provider
            if stats.average_response_time_ms < fastest_time && stats.total_requests > 10 {
                fastest_time = stats.average_response_time_ms;
                fastest_provider = Some(provider.clone());
            }

            // Find most reliable provider
            if stats.success_rate > highest_success_rate && stats.total_requests > 10 {
                highest_success_rate = stats.success_rate;
                most_reliable_provider = Some(provider.clone());
            }

            // Find most cost-effective provider
            if stats.total_tokens_used > 0 {
                let cost_per_token = stats.estimated_cost_usd / (stats.total_tokens_used as f64);
                if cost_per_token < lowest_cost_per_token {
                    lowest_cost_per_token = cost_per_token;
                    most_cost_effective_provider = Some(provider.clone());
                }
            }
        }

        // Determine recommended primary provider (balance of speed, reliability, cost)
        let recommended_primary_provider = provider_stats
            .iter()
            .filter(|(_, stats)| stats.total_requests > 10 && stats.success_rate > 90.0)
            .max_by(|(_, a), (_, b)| {
                // Score = success_rate * speed_factor * cost_factor
                let a_speed_factor = 1000.0 / a.average_response_time_ms.max(100.0);
                let a_cost_factor = if a.total_tokens_used > 0 {
                    1.0 / (a.estimated_cost_usd / a.total_tokens_used as f64).max(0.001)
                } else {
                    1.0
                };
                let a_score = a.success_rate * a_speed_factor * a_cost_factor;

                let b_speed_factor = 1000.0 / b.average_response_time_ms.max(100.0);
                let b_cost_factor = if b.total_tokens_used > 0 {
                    1.0 / (b.estimated_cost_usd / b.total_tokens_used as f64).max(0.001)
                } else {
                    1.0
                };
                let b_score = b.success_rate * b_speed_factor * b_cost_factor;

                a_score
                    .partial_cmp(&b_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(provider, _)| provider.clone());

        PerformanceSummary {
            fastest_provider,
            most_reliable_provider,
            most_cost_effective_provider,
            recommended_primary_provider,
        }
    }

    /// Generate a detailed metrics report
    pub async fn generate_report(&self) -> String {
        let snapshot = self.get_snapshot().await;

        let mut report = String::new();
        report.push_str("=== LLM Integration Metrics Report ===\n");
        report.push_str(&format!(
            "Generated: {}\n\n",
            snapshot.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
        ));

        // Global statistics
        report.push_str("GLOBAL STATISTICS:\n");
        report.push_str(&format!(
            "  Total Requests: {}\n",
            snapshot.global_stats.total_requests
        ));
        report.push_str(&format!(
            "  Overall Success Rate: {:.1}%\n",
            snapshot.global_stats.overall_success_rate
        ));
        report.push_str(&format!(
            "  Cache Hit Rate: {:.1}%\n",
            snapshot.global_stats.cache_hit_rate
        ));
        report.push_str(&format!(
            "  Fallback Rate: {:.1}%\n",
            snapshot.global_stats.fallback_rate
        ));
        report.push_str(&format!(
            "  All Providers Failure Rate: {:.1}%\n\n",
            snapshot.global_stats.all_providers_failure_rate
        ));

        // Provider-specific statistics
        report.push_str("PROVIDER STATISTICS:\n");
        for (provider, stats) in &snapshot.provider_stats {
            report.push_str(&format!("  {}:\n", provider));
            report.push_str(&format!(
                "    Requests: {} (Success: {:.1}%)\n",
                stats.total_requests, stats.success_rate
            ));
            report.push_str(&format!(
                "    Avg Response Time: {:.1}ms\n",
                stats.average_response_time_ms
            ));
            report.push_str(&format!(
                "    Tokens Used: {} (Cost: ${:.4})\n",
                stats.total_tokens_used, stats.estimated_cost_usd
            ));
            report.push_str(&format!(
                "    Rate Limit Hits: {:.1}%\n",
                stats.rate_limit_hit_rate
            ));

            if let Some(last_success) = stats.last_success {
                report.push_str(&format!(
                    "    Last Success: {}\n",
                    last_success.format("%Y-%m-%d %H:%M:%S UTC")
                ));
            }

            if !stats.common_errors.is_empty() {
                report.push_str("    Common Errors:\n");
                for (error_type, count) in &stats.common_errors {
                    report.push_str(&format!("      {}: {} occurrences\n", error_type, count));
                }
            }
            report.push('\n');
        }

        // Performance recommendations
        report.push_str("PERFORMANCE RECOMMENDATIONS:\n");
        if let Some(fastest) = &snapshot.performance_summary.fastest_provider {
            report.push_str(&format!("  Fastest Provider: {}\n", fastest));
        }
        if let Some(most_reliable) = &snapshot.performance_summary.most_reliable_provider {
            report.push_str(&format!("  Most Reliable Provider: {}\n", most_reliable));
        }
        if let Some(most_cost_effective) =
            &snapshot.performance_summary.most_cost_effective_provider
        {
            report.push_str(&format!(
                "  Most Cost-Effective Provider: {}\n",
                most_cost_effective
            ));
        }
        if let Some(recommended) = &snapshot.performance_summary.recommended_primary_provider {
            report.push_str(&format!(
                "  Recommended Primary Provider: {}\n",
                recommended
            ));
        }

        report
    }

    /// Get metrics for a specific provider
    pub async fn get_provider_stats(&self, provider: &LLMProvider) -> Option<ProviderStats> {
        let snapshot = self.get_snapshot().await;
        snapshot.provider_stats.get(provider).cloned()
    }

    /// Reset metrics (useful for testing or periodic resets)
    pub async fn reset_metrics(&self) {
        let mut provider_metrics = self.provider_metrics.write().await;
        provider_metrics.clear();

        // Reset global metrics
        self.global_metrics
            .total_requests
            .store(0, Ordering::Relaxed);
        self.global_metrics.cache_hits.store(0, Ordering::Relaxed);
        self.global_metrics.cache_misses.store(0, Ordering::Relaxed);
        self.global_metrics
            .fallback_activations
            .store(0, Ordering::Relaxed);
        self.global_metrics
            .all_providers_failures
            .store(0, Ordering::Relaxed);
    }
}

impl Default for LLMMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for ProviderMetrics {
    fn default() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            total_tokens_used: AtomicU64::new(0),
            total_cost_usd: AtomicU64::new(0),
            total_response_time_ms: AtomicU64::new(0),
            rate_limit_hits: AtomicU32::new(0),
            authentication_failures: AtomicU32::new(0),
            last_success: RwLock::new(None),
            last_failure: RwLock::new(None),
            error_breakdown: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recording() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let metrics = LLMMetrics::new();

            // Record some successful operations
            metrics.record_success(&LLMProvider::OpenAI, 1500).await;
            metrics.record_success(&LLMProvider::OpenAI, 2000).await;
            metrics
                .record_token_usage(&LLMProvider::OpenAI, 100, 0.002)
                .await;

            // Record a failure
            let error = LLMError::RateLimitExceeded {
                provider: LLMProvider::OpenAI,
                retry_after_seconds: 60,
            };
            metrics.record_failure(&LLMProvider::OpenAI, &error).await;

            // Get snapshot and verify metrics
            let snapshot = metrics.get_snapshot().await;
            let openai_stats = snapshot.provider_stats.get(&LLMProvider::OpenAI).unwrap();

            assert_eq!(openai_stats.total_requests, 3);
            // Success rate should be ~66.7% (2/3)
            let expected_rate = 2.0 / 3.0 * 100.0;
            assert!((openai_stats.success_rate - expected_rate).abs() < 0.1);
            assert_eq!(openai_stats.average_response_time_ms, 1750.0); // (1500 + 2000) / 2
            assert_eq!(openai_stats.total_tokens_used, 100);
            // Check estimated cost with floating point tolerance
            assert!((openai_stats.estimated_cost_usd - 0.002).abs() < 0.0001);
        });
    }

    #[tokio::test]
    async fn test_performance_summary_calculation() {
        let metrics = LLMMetrics::new();

        // Record different provider performance
        for _ in 0..20 {
            metrics.record_success(&LLMProvider::OpenAI, 1000).await; // Fast
            metrics.record_success(&LLMProvider::Anthropic, 2000).await; // Slower
        }

        metrics
            .record_token_usage(&LLMProvider::OpenAI, 1000, 0.10)
            .await; // More expensive
        metrics
            .record_token_usage(&LLMProvider::Anthropic, 1000, 0.05)
            .await; // Cheaper

        let snapshot = metrics.get_snapshot().await;

        assert_eq!(
            snapshot.performance_summary.fastest_provider,
            Some(LLMProvider::OpenAI)
        );
        assert_eq!(
            snapshot.performance_summary.most_cost_effective_provider,
            Some(LLMProvider::Anthropic)
        );
    }

    #[tokio::test]
    async fn test_metrics_report_generation() {
        let metrics = LLMMetrics::new();

        metrics.record_success(&LLMProvider::OpenAI, 1500).await;
        metrics
            .record_token_usage(&LLMProvider::OpenAI, 150, 0.003)
            .await;

        let report = metrics.generate_report().await;

        assert!(report.contains("LLM Integration Metrics Report"));
        assert!(report.contains("OpenAI"));
        assert!(report.contains("Success Rate"));
        assert!(report.contains("PERFORMANCE RECOMMENDATIONS"));
    }
}
