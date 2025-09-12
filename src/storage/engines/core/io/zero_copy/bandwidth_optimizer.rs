// Smart Download Optimizer with Multi-Factor Decision Engine
// Intelligent decisions about selective range requests vs full file downloads

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::config::{
    AccessPredictionConfig, CostOptimizationConfig, DownloadOptimizerConfig, NetworkAdjustments,
    RangeOptimizationConfig,
};
use super::traits::{DataRange, FileAccessRequest, QueryContext, QueryType, RequestPriority};
use crate::core::error::ProximaDBError;

/// Download strategy decision
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStrategy {
    /// Skip file entirely (no download needed)
    SkipFile { reason: String },
    /// Download entire file
    FullDownload { cache_locally: bool, reason: String },
    /// Download specific ranges only
    SelectiveRanges {
        ranges: Vec<OptimizedRange>,
        total_bytes: u64,
        reason: String,
    },
    /// Hybrid strategy with fallback
    HybridStrategy {
        primary: Box<DownloadStrategy>,
        fallback: Box<DownloadStrategy>,
        condition: String,
    },
}

/// Optimized data range with priority and merging information
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizedRange {
    /// Original data range
    pub range: DataRange,
    /// Whether this range was merged from multiple smaller ranges
    pub is_merged: bool,
    /// Number of original ranges that were merged
    pub merge_count: u32,
    /// Estimated access probability
    pub access_probability: f32,
}

impl OptimizedRange {
    pub fn new(range: DataRange) -> Self {
        Self {
            range,
            is_merged: false,
            merge_count: 1,
            access_probability: 1.0,
        }
    }

    pub fn merged(ranges: Vec<DataRange>, access_probability: f32) -> Self {
        if ranges.is_empty() {
            panic!("Cannot create merged range from empty ranges");
        }

        let start = ranges.iter().map(|r| r.offset).min().unwrap();
        let end = ranges.iter().map(|r| r.offset + r.length).max().unwrap();
        let priority = ranges.iter().map(|r| r.priority).max().unwrap();

        Self {
            range: DataRange::new(start, end - start, priority),
            is_merged: true,
            merge_count: ranges.len() as u32,
            access_probability,
        }
    }
}

impl Eq for OptimizedRange {}

impl PartialOrd for OptimizedRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OptimizedRange {
    fn cmp(&self, other: &Self) -> Ordering {
        // Sort by priority first, then by offset
        self.range
            .priority
            .cmp(&other.range.priority)
            .then(self.range.offset.cmp(&other.range.offset))
    }
}

/// Access prediction for future file access
#[derive(Debug, Clone)]
pub struct AccessPrediction {
    /// Probability of accessing this file again (0.0 - 1.0)
    pub future_access_probability: f32,
    /// Predicted time until next access
    pub predicted_next_access: Option<Duration>,
    /// Confidence in the prediction
    pub confidence: f32,
    /// Historical access pattern
    pub access_pattern: AccessPattern,
}

/// Historical access pattern classification
#[derive(Debug, Clone)]
pub enum AccessPattern {
    /// Never accessed before
    Unknown,
    /// Accessed once, unlikely to repeat
    OneTime,
    /// Accessed multiple times in short period
    Burst,
    /// Regular periodic access
    Periodic { interval: Duration },
    /// Trending (increasing access frequency)
    Trending,
    /// Recently popular
    Hot,
    /// Rarely accessed
    Cold,
}

/// Decision factors and their weights
#[derive(Debug, Clone)]
pub struct DecisionFactors {
    /// Percentage of file data actually needed
    pub data_percentage: f32,
    /// Number of HTTP range requests required
    pub request_count: u32,
    /// Network latency to storage (milliseconds)
    pub network_latency_ms: f32,
    /// Future access probability (0.0 - 1.0)
    pub future_access_probability: f32,
    /// Total file size in bytes
    pub file_size_bytes: u64,
    /// Estimated cost savings with selective download
    pub cost_savings_estimate: f64,
    /// Cache value score (benefit of caching full file)
    pub cache_value_score: f32,
}

/// Decision rationale with detailed reasoning
#[derive(Debug, Clone)]
pub struct DecisionRationale {
    /// Primary decision factor
    pub primary_factor: String,
    /// Contributing factors
    pub contributing_factors: Vec<String>,
    /// Threshold values used
    pub thresholds_applied: HashMap<String, f32>,
    /// Estimated savings
    pub estimated_savings: f64,
    /// Risk factors considered
    pub risk_factors: Vec<String>,
}

/// Smart download optimizer with multi-factor decision engine
pub struct BandwidthOptimizer {
    /// Configuration settings
    config: DownloadOptimizerConfig,
    /// Network condition tracker
    network_tracker: NetworkConditionTracker,
    /// Access pattern predictor
    access_predictor: AccessPatternPredictor,
    /// Cost calculator
    cost_calculator: CostCalculator,
    /// Range optimizer
    range_optimizer: RangeOptimizer,
}

impl BandwidthOptimizer {
    /// Create new optimizer with configuration
    pub fn new(config: DownloadOptimizerConfig) -> Self {
        Self {
            network_tracker: NetworkConditionTracker::new(&config.network_adjustments),
            access_predictor: AccessPatternPredictor::new(&config.access_prediction),
            cost_calculator: CostCalculator::new(&config.cost_optimization),
            range_optimizer: RangeOptimizer::new(&config.range_optimization),
            config,
        }
    }

    /// Decide download strategy for a file access request with smart thresholds
    pub async fn decide_strategy(
        &self,
        file_path: &str,
        file_size: u64,
        required_ranges: Option<Vec<DataRange>>,
        query_context: &QueryContext,
        request_priority: RequestPriority,
    ) -> Result<DownloadStrategy, ProximaDBError> {
        let start_time = Instant::now();

        // If no ranges required, skip file entirely
        let ranges = match required_ranges {
            Some(ranges) if !ranges.is_empty() => ranges,
            _ => {
                return Ok(DownloadStrategy::SkipFile {
                    reason: "No data ranges required for query".to_string(),
                });
            }
        };

        // Calculate decision factors with smart thresholds
        let decision_factors = self
            .calculate_decision_factors_with_smart_thresholds(
                file_path,
                file_size,
                &ranges,
                query_context,
                request_priority,
            )
            .await?;

        // Make strategy decision with bandwidth optimization
        let strategy = self
            .make_bandwidth_optimized_decision(
                file_path,
                file_size,
                ranges,
                &decision_factors,
                query_context,
            )
            .await?;

        let decision_time = start_time.elapsed();
        debug!(
            file_path,
            decision_time_ms = decision_time.as_millis(),
            strategy = ?strategy,
            data_percentage = decision_factors.data_percentage,
            future_access_prob = decision_factors.future_access_probability,
            cache_value = decision_factors.cache_value_score,
            "Bandwidth-optimized download strategy decided"
        );

        Ok(strategy)
    }

    /// Batch optimize multiple file access requests
    pub async fn batch_optimize(
        &self,
        requests: Vec<FileAccessRequest>,
    ) -> Result<Vec<DownloadStrategy>, ProximaDBError> {
        let mut strategies = Vec::with_capacity(requests.len());

        // Sort requests by priority
        let mut sorted_requests = requests;
        sorted_requests.sort_by(|a, b| b.priority.cmp(&a.priority));

        // Process requests in batches to optimize cross-file patterns
        for chunk in sorted_requests.chunks(self.config.range_optimization.max_concurrent_requests)
        {
            let mut chunk_strategies = Vec::new();

            for request in chunk {
                // For batch optimization, we would need file size and ranges
                // This is a simplified version
                let strategy = self
                    .decide_strategy(
                        &request.file_path,
                        0,    // Would need actual file size
                        None, // Would need actual ranges
                        &request.query_context,
                        request.priority.clone(),
                    )
                    .await?;

                chunk_strategies.push(strategy);
            }

            strategies.extend(chunk_strategies);
        }

        Ok(strategies)
    }

    /// Update network conditions based on measurements
    pub fn update_network_conditions(&mut self, latency_ms: f32, bandwidth_mbps: f32) {
        self.network_tracker.update(latency_ms, bandwidth_mbps);
    }

    /// Record access pattern for learning
    pub fn record_file_access(&mut self, file_path: &str, access_type: QueryType) {
        self.access_predictor.record_access(file_path, access_type);
    }

    /// Get current network conditions
    pub fn get_network_conditions(&self) -> (f32, f32) {
        self.network_tracker.get_current_conditions()
    }

    async fn calculate_decision_factors_with_smart_thresholds(
        &self,
        file_path: &str,
        file_size: u64,
        ranges: &[DataRange],
        query_context: &QueryContext,
        request_priority: RequestPriority,
    ) -> Result<DecisionFactors, ProximaDBError> {
        // Calculate required data percentage
        let required_bytes: u64 = ranges.iter().map(|r| r.length).sum();
        let data_percentage = if file_size > 0 {
            (required_bytes as f64 / file_size as f64 * 100.0) as f32
        } else {
            100.0
        };

        // Optimize ranges and count requests
        let optimized_ranges = self.range_optimizer.optimize_ranges(ranges.to_vec());
        let request_count = optimized_ranges.len() as u32;

        // Get network conditions
        let (network_latency_ms, _bandwidth_mbps) = self.network_tracker.get_current_conditions();

        // Predict future access
        let access_prediction = self
            .access_predictor
            .predict_access(file_path, query_context);
        let future_access_probability = access_prediction.future_access_probability;

        // Calculate cost savings
        let cost_savings_estimate =
            self.cost_calculator
                .estimate_cost_savings(file_size, required_bytes, request_count);

        // Calculate cache value score
        let cache_value_score = self.calculate_cache_value_score(
            file_path,
            file_size,
            query_context,
            &access_prediction,
            request_priority,
        );

        Ok(DecisionFactors {
            data_percentage,
            request_count,
            network_latency_ms,
            future_access_probability,
            file_size_bytes: file_size,
            cost_savings_estimate,
            cache_value_score,
        })
    }

    async fn make_bandwidth_optimized_decision(
        &self,
        file_path: &str,
        file_size: u64,
        ranges: Vec<DataRange>,
        factors: &DecisionFactors,
        query_context: &QueryContext,
    ) -> Result<DownloadStrategy, ProximaDBError> {
        let mut rationale = DecisionRationale {
            primary_factor: String::new(),
            contributing_factors: Vec::new(),
            thresholds_applied: HashMap::new(),
            estimated_savings: factors.cost_savings_estimate,
            risk_factors: Vec::new(),
        };

        // Get size-based threshold
        let base_threshold = self.get_size_based_threshold(file_size);
        rationale
            .thresholds_applied
            .insert("base_threshold".to_string(), base_threshold);

        // Apply network adjustments
        let adjusted_threshold = self.apply_network_adjustments(base_threshold, factors);
        rationale
            .thresholds_applied
            .insert("adjusted_threshold".to_string(), adjusted_threshold);

        // Check primary decision factors in order of importance

        // Smart threshold decision with bandwidth optimization integration

        // 1. Small file optimization - always cache for fast future access
        if file_size < 16 * 1024 * 1024 {
            // 16MB threshold
            rationale.primary_factor = format!("Small file {} bytes - cache for speed", file_size);
            return Ok(DownloadStrategy::FullDownload {
                cache_locally: true,
                reason: rationale.primary_factor,
            });
        }

        // 2. Data percentage with bandwidth-aware thresholds
        let bandwidth_adjusted_threshold =
            self.get_bandwidth_aware_threshold(factors, adjusted_threshold);
        rationale.thresholds_applied.insert(
            "bandwidth_threshold".to_string(),
            bandwidth_adjusted_threshold,
        );

        if factors.data_percentage >= bandwidth_adjusted_threshold {
            rationale.primary_factor = format!(
                "Data percentage {:.1}% >= bandwidth-optimized threshold {:.1}%",
                factors.data_percentage, bandwidth_adjusted_threshold
            );
            rationale
                .contributing_factors
                .push("Network conditions favor full download".to_string());

            return Ok(DownloadStrategy::FullDownload {
                cache_locally: factors.future_access_probability > 0.4,
                reason: format!(
                    "{} - {}",
                    rationale.primary_factor,
                    rationale.contributing_factors.join(", ")
                ),
            });
        }

        // 3. Request count with latency optimization
        let max_requests = self.get_latency_optimized_request_limit(factors);
        if factors.request_count > max_requests {
            rationale.primary_factor = format!(
                "Request count {} > latency-optimized limit {}",
                factors.request_count, max_requests
            );
            rationale
                .contributing_factors
                .push("Reducing round-trips for better performance".to_string());

            return Ok(DownloadStrategy::FullDownload {
                cache_locally: true,
                reason: format!(
                    "{} - {}",
                    rationale.primary_factor,
                    rationale.contributing_factors.join(", ")
                ),
            });
        }

        // 4. Future access prediction with cache value optimization
        let cache_threshold = self.get_cache_value_threshold(file_size, query_context);
        if factors.future_access_probability > cache_threshold || factors.cache_value_score > 0.8 {
            rationale.primary_factor = format!(
                "High cache value (access_prob: {:.1}%, cache_score: {:.1}%)",
                factors.future_access_probability * 100.0,
                factors.cache_value_score * 100.0
            );
            rationale
                .contributing_factors
                .push("Predictive caching optimization".to_string());

            return Ok(DownloadStrategy::FullDownload {
                cache_locally: true,
                reason: format!(
                    "{} - {}",
                    rationale.primary_factor,
                    rationale.contributing_factors.join(", ")
                ),
            });
        }

        // 4. Cost optimization check
        if factors.cost_savings_estimate
            < self.config.cost_optimization.min_savings_for_selective as f64
        {
            rationale.primary_factor = format!(
                "Cost savings ${:.3} < minimum ${:.3}",
                factors.cost_savings_estimate,
                self.config.cost_optimization.min_savings_for_selective as f64 / 1_000_000.0
            );

            return Ok(DownloadStrategy::FullDownload {
                cache_locally: false,
                reason: rationale.primary_factor,
            });
        }

        // 5. High cache value for small files
        if file_size < self.config.size_thresholds.small_file_threshold
            && factors.cache_value_score > 0.6
        {
            rationale.primary_factor = format!(
                "Small file with high cache value (score: {:.2})",
                factors.cache_value_score
            );

            return Ok(DownloadStrategy::FullDownload {
                cache_locally: true,
                reason: rationale.primary_factor,
            });
        }

        // Default: Use selective ranges
        let optimized_ranges = self.range_optimizer.optimize_ranges(ranges);
        let total_bytes: u64 = optimized_ranges.iter().map(|r| r.range.length).sum();

        rationale.primary_factor = format!(
            "Selective ranges: {:.1}% of file, {} requests",
            factors.data_percentage, factors.request_count
        );
        rationale.contributing_factors.push(format!(
            "Estimated savings: ${:.3}",
            factors.cost_savings_estimate
        ));

        Ok(DownloadStrategy::SelectiveRanges {
            ranges: optimized_ranges,
            total_bytes,
            reason: format!(
                "{} - {}",
                rationale.primary_factor,
                rationale.contributing_factors.join(", ")
            ),
        })
    }

    fn get_size_based_threshold(&self, file_size: u64) -> f32 {
        let thresholds = &self.config.size_thresholds;

        if file_size < thresholds.small_file_threshold {
            thresholds.small_file_download_percent
        } else if file_size < thresholds.medium_file_threshold {
            thresholds.medium_file_download_percent
        } else if file_size < thresholds.large_file_threshold {
            thresholds.large_file_download_percent
        } else {
            thresholds.huge_file_download_percent
        }
    }

    fn apply_network_adjustments(&self, base_threshold: f32, factors: &DecisionFactors) -> f32 {
        let adjustments = &self.config.network_adjustments;
        let mut adjusted = base_threshold;

        // High latency adjustment (increase threshold to prefer full downloads)
        if factors.network_latency_ms > adjustments.high_latency_threshold {
            adjusted += adjustments.high_latency_adjustment;
        }

        // Note: bandwidth adjustment would require current bandwidth measurement
        // which would come from network_tracker

        adjusted.clamp(5.0, 95.0) // Keep within reasonable bounds
    }

    fn calculate_cache_value_score(
        &self,
        _file_path: &str,
        file_size: u64,
        query_context: &QueryContext,
        access_prediction: &AccessPrediction,
        request_priority: RequestPriority,
    ) -> f32 {
        let mut score = 0.0;

        // Future access probability
        score += access_prediction.future_access_probability * 0.4;

        // Query complexity (more complex queries benefit from caching)
        score += query_context.complexity_score() * 0.3;

        // Request priority
        let priority_weight = match request_priority {
            RequestPriority::Critical => 0.3,
            RequestPriority::High => 0.2,
            RequestPriority::Normal => 0.1,
            RequestPriority::Low => 0.05,
            RequestPriority::Background => 0.0,
        };
        score += priority_weight;

        // File size factor (smaller files have higher cache value)
        let size_factor = if file_size < 10 * 1024 * 1024 {
            // 10MB
            0.2
        } else if file_size < 100 * 1024 * 1024 {
            // 100MB
            0.1
        } else {
            0.0
        };
        score += size_factor;

        score.clamp(0.0, 1.0)
    }

    /// Get bandwidth-aware threshold that considers network conditions
    fn get_bandwidth_aware_threshold(&self, factors: &DecisionFactors, base_threshold: f32) -> f32 {
        let (latency_ms, bandwidth_mbps) = self.network_tracker.get_current_conditions();
        let mut threshold = base_threshold;

        // High latency reduces threshold (favor full downloads to reduce round trips)
        if latency_ms > 100.0 {
            threshold *= 0.7; // Reduce by 30%
        } else if latency_ms > 50.0 {
            threshold *= 0.85; // Reduce by 15%
        }

        // Low bandwidth increases threshold (favor range requests to save bandwidth)
        if bandwidth_mbps < 10.0 {
            threshold *= 1.5; // Increase by 50%
        } else if bandwidth_mbps < 50.0 {
            threshold *= 1.2; // Increase by 20%
        }

        threshold.clamp(5.0, 95.0) // Keep within reasonable bounds
    }

    /// Get latency-optimized request limit
    fn get_latency_optimized_request_limit(&self, factors: &DecisionFactors) -> u32 {
        let (latency_ms, _) = self.network_tracker.get_current_conditions();
        let base_limit = self.config.cost_optimization.max_range_requests;

        // Reduce request limit for high latency connections
        if latency_ms > 200.0 {
            (base_limit as f32 * 0.5) as u32 // Halve for very high latency
        } else if latency_ms > 100.0 {
            (base_limit as f32 * 0.7) as u32 // Reduce by 30%
        } else if latency_ms > 50.0 {
            (base_limit as f32 * 0.85) as u32 // Reduce by 15%
        } else {
            base_limit as u32 // Normal limit for low latency
        }
    }

    /// Get cache value threshold based on file size and query context
    fn get_cache_value_threshold(&self, file_size: u64, query_context: &QueryContext) -> f32 {
        let mut threshold = 0.6; // Base threshold

        // Smaller files have lower threshold (easier to cache)
        if file_size < 10 * 1024 * 1024 {
            // 10MB
            threshold = 0.3;
        } else if file_size < 100 * 1024 * 1024 {
            // 100MB
            threshold = 0.5;
        } else if file_size > 1024 * 1024 * 1024 {
            // 1GB
            threshold = 0.8;
        }

        // Complex queries benefit more from caching
        if query_context.complexity_score() > 0.8 {
            threshold *= 0.8; // Lower threshold for complex queries
        }

        threshold
    }
}

/// Network condition tracker
pub struct NetworkConditionTracker {
    current_latency_ms: f32,
    current_bandwidth_mbps: f32,
    high_latency_threshold: f32,
    low_bandwidth_threshold: f32,
}

impl NetworkConditionTracker {
    fn new(config: &NetworkAdjustments) -> Self {
        Self {
            current_latency_ms: 50.0,      // Default assumption
            current_bandwidth_mbps: 100.0, // Default assumption
            high_latency_threshold: config.high_latency_threshold,
            low_bandwidth_threshold: config.low_bandwidth_threshold,
        }
    }

    fn update(&mut self, latency_ms: f32, bandwidth_mbps: f32) {
        self.current_latency_ms = latency_ms;
        self.current_bandwidth_mbps = bandwidth_mbps;
    }

    fn get_current_conditions(&self) -> (f32, f32) {
        (self.current_latency_ms, self.current_bandwidth_mbps)
    }
}

/// Access pattern predictor
pub struct AccessPatternPredictor {
    file_access_history: HashMap<String, Vec<AccessEvent>>,
    config: AccessPredictionConfig,
}

#[derive(Debug, Clone)]
struct AccessEvent {
    timestamp: Instant,
    query_type: QueryType,
}

impl AccessPatternPredictor {
    fn new(config: &AccessPredictionConfig) -> Self {
        Self {
            file_access_history: HashMap::new(),
            config: config.clone(),
        }
    }

    fn record_access(&mut self, file_path: &str, query_type: QueryType) {
        let event = AccessEvent {
            timestamp: Instant::now(),
            query_type,
        };

        self.file_access_history
            .entry(file_path.to_string())
            .or_default()
            .push(event);
    }

    fn predict_access(&self, file_path: &str, _query_context: &QueryContext) -> AccessPrediction {
        let history = self.file_access_history.get(file_path);

        match history {
            None => AccessPrediction {
                future_access_probability: 0.1, // Low default for unknown files
                predicted_next_access: None,
                confidence: 0.2,
                access_pattern: AccessPattern::Unknown,
            },
            Some(events) => {
                if events.len() < self.config.min_accesses_for_prediction as usize {
                    return AccessPrediction {
                        future_access_probability: 0.3,
                        predicted_next_access: None,
                        confidence: 0.4,
                        access_pattern: AccessPattern::OneTime,
                    };
                }

                // Simple prediction based on recent access frequency
                let recent_events = events
                    .iter()
                    .filter(|e| e.timestamp.elapsed() < self.config.history_window)
                    .count();

                let probability = if recent_events > 5 {
                    0.8 // Hot file
                } else if recent_events > 2 {
                    0.6 // Warm file
                } else if recent_events > 0 {
                    0.3 // Cold file
                } else {
                    0.1 // Very cold file
                };

                AccessPrediction {
                    future_access_probability: probability,
                    predicted_next_access: Some(Duration::from_secs(300)), // 5 minutes default
                    confidence: self.config.confidence_threshold,
                    access_pattern: if recent_events > 5 {
                        AccessPattern::Hot
                    } else if recent_events > 0 {
                        AccessPattern::Cold
                    } else {
                        AccessPattern::Unknown
                    },
                }
            }
        }
    }
}

/// Cost calculator for bandwidth optimization
pub struct CostCalculator {
    config: CostOptimizationConfig,
}

impl CostCalculator {
    fn new(config: &CostOptimizationConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    fn estimate_cost_savings(
        &self,
        file_size: u64,
        required_bytes: u64,
        request_count: u32,
    ) -> f64 {
        // Calculate bandwidth cost savings
        let bytes_saved = file_size.saturating_sub(required_bytes);
        let bandwidth_savings =
            (bytes_saved as f64 / (1024.0 * 1024.0 * 1024.0)) * self.config.bandwidth_cost_per_gb;

        // Calculate request cost impact (negative for additional requests)
        let additional_requests = request_count.saturating_sub(1) as f64; // Assume 1 request for full download
        let request_cost_impact =
            additional_requests * self.config.request_cost_weight as f64 * 0.001; // Small per-request cost

        bandwidth_savings - request_cost_impact
    }
}

/// Range optimizer for merging and optimizing data ranges
pub struct RangeOptimizer {
    config: RangeOptimizationConfig,
}

impl RangeOptimizer {
    fn new(config: &RangeOptimizationConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    fn optimize_ranges(&self, mut ranges: Vec<DataRange>) -> Vec<OptimizedRange> {
        if ranges.is_empty() {
            return Vec::new();
        }

        // Sort ranges by offset
        ranges.sort_by_key(|r| r.offset);

        let mut optimized = Vec::new();
        let mut current_ranges = vec![ranges[0]];

        for range in ranges.into_iter().skip(1) {
            let last_range = current_ranges.last().unwrap();

            // Check if ranges should be merged
            let gap = range
                .offset
                .saturating_sub(last_range.offset + last_range.length);

            if gap <= self.config.max_merge_gap {
                // Merge ranges
                current_ranges.push(range);
            } else {
                // Finalize current group and start new one
                if current_ranges.len() == 1 {
                    optimized.push(OptimizedRange::new(current_ranges[0]));
                } else {
                    optimized.push(OptimizedRange::merged(current_ranges, 0.8));
                }
                current_ranges = vec![range];
            }
        }

        // Handle last group
        if current_ranges.len() == 1 {
            optimized.push(OptimizedRange::new(current_ranges[0]));
        } else {
            optimized.push(OptimizedRange::merged(current_ranges, 0.8));
        }

        // Sort by priority
        optimized.sort();
        optimized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimized_range_creation() {
        let range = DataRange::new(100, 200, 255);
        let opt_range = OptimizedRange::new(range);

        assert_eq!(opt_range.range.offset, 100);
        assert_eq!(opt_range.range.length, 200);
        assert!(!opt_range.is_merged);
        assert_eq!(opt_range.merge_count, 1);
    }

    #[test]
    fn test_range_merging() {
        let ranges = vec![DataRange::new(100, 50, 255), DataRange::new(200, 50, 128)];

        let merged = OptimizedRange::merged(ranges, 0.9);
        assert_eq!(merged.range.offset, 100);
        assert_eq!(merged.range.length, 150);
        assert!(merged.is_merged);
        assert_eq!(merged.merge_count, 2);
        assert_eq!(merged.access_probability, 0.9);
        assert_eq!(merged.range.priority, 255);
    }

    #[test]
    fn test_size_based_thresholds() {
        let config = DownloadOptimizerConfig::default();
        let optimizer = BandwidthOptimizer::new(config);

        // Small file
        let small_threshold = optimizer.get_size_based_threshold(5 * 1024 * 1024);
        assert_eq!(small_threshold, 25.0);

        // Large file
        let large_threshold = optimizer.get_size_based_threshold(500 * 1024 * 1024);
        assert_eq!(large_threshold, 50.0);

        // Huge file
        let huge_threshold = optimizer.get_size_based_threshold(2 * 1024 * 1024 * 1024);
        assert_eq!(huge_threshold, 60.0);
    }

    #[test]
    fn test_access_prediction() {
        let config = AccessPredictionConfig::default();
        let mut predictor = AccessPatternPredictor::new(&config);

        // Unknown file
        let prediction = predictor.predict_access("unknown.sst", &QueryContext::default());
        assert!(prediction.future_access_probability < 0.2);
        assert!(matches!(prediction.access_pattern, AccessPattern::Unknown));

        // Record some accesses
        for _ in 0..6 {
            predictor.record_access("hot.sst", QueryType::SimilaritySearch);
        }

        let hot_prediction = predictor.predict_access("hot.sst", &QueryContext::default());
        assert!(hot_prediction.future_access_probability > 0.7);
        assert!(matches!(hot_prediction.access_pattern, AccessPattern::Hot));
    }
}
