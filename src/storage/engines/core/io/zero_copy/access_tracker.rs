// Access Pattern Tracker for Learning and Prediction
// Tracks file access patterns to improve future optimization decisions

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::Serialize;
use tracing::{debug, trace};

use super::traits::QueryType;

/// Access event for pattern tracking
#[derive(Debug, Clone, Serialize)]
pub struct AccessEvent {
    /// File path that was accessed
    pub file_path: String,
    /// Collection ID
    pub collection_id: String,
    /// Type of query that triggered the access
    pub query_type: QueryType,
    /// When the access occurred
    #[serde(skip)]
    pub timestamp: Instant,
    /// Result of the access optimization
    pub result_type: String,
}

/// Access pattern statistics for a file
#[derive(Debug, Clone, Serialize)]
pub struct AccessStats {
    /// Total number of accesses
    pub total_accesses: u64,
    /// Last access time
    #[serde(skip)]
    pub last_accessed: Instant,
    /// First access time
    #[serde(skip)]
    pub first_accessed: Instant,
    /// Access frequency (accesses per hour)
    pub access_frequency: f64,
    /// Most common query type
    pub primary_query_type: QueryType,
    /// Distribution of query types
    pub query_type_distribution: HashMap<QueryType, u64>,
    /// Recent access pattern (last 10 accesses)
    pub recent_pattern: VecDeque<AccessEvent>,
    /// Access timing pattern
    pub timing_pattern: TimingPattern,
}

/// Timing pattern classification
#[derive(Debug, Clone, Serialize)]
pub enum TimingPattern {
    /// Random access times
    Random,
    /// Regular periodic access
    Periodic { interval: Duration, confidence: f64 },
    /// Burst access (many accesses in short time)
    Burst {
        burst_duration: Duration,
        quiet_duration: Duration,
    },
    /// Trending (increasing frequency)
    Trending { growth_rate: f64 },
    /// Declining (decreasing frequency)
    Declining { decay_rate: f64 },
}

/// Collection-level access patterns
#[derive(Debug, Clone)]
pub struct CollectionAccessPattern {
    /// Collection ID
    #[allow(dead_code)]
    pub collection_id: String,
    /// Total files accessed in this collection
    #[allow(dead_code)]
    pub files_accessed: u64,
    /// Most active files
    #[allow(dead_code)]
    pub hot_files: Vec<String>,
    /// Dominant query types for this collection
    #[allow(dead_code)]
    pub dominant_query_types: Vec<QueryType>,
    /// Access velocity (accesses per hour)
    #[allow(dead_code)]
    pub access_velocity: f64,
    /// Last activity time
    #[allow(dead_code)]
    pub last_activity: Instant,
}

/// Access pattern predictor
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AccessPrediction {
    /// Probability of access in next time window (0.0-1.0)
    pub access_probability: f64,
    /// Predicted time until next access
    #[allow(dead_code)]
    pub predicted_next_access: Option<Duration>,
    /// Confidence in prediction (0.0-1.0)
    #[allow(dead_code)]
    pub confidence: f64,
    /// Predicted query type
    pub predicted_query_type: Option<QueryType>,
    /// Reasoning for the prediction
    pub prediction_rationale: String,
}

/// Access pattern tracker with learning capabilities
pub struct AccessPatternTracker {
    /// File-level access statistics
    file_stats: HashMap<String, AccessStats>,
    /// Collection-level patterns
    collection_patterns: HashMap<String, CollectionAccessPattern>,
    /// Recent access events (sliding window)
    recent_events: VecDeque<AccessEvent>,
    /// Maximum number of events to keep in memory
    max_events: usize,
    /// Time window for pattern analysis
    analysis_window: Duration,
    /// Learning parameters
    #[allow(dead_code)]
    learning_params: LearningParameters,
}

/// Parameters for pattern learning
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LearningParameters {
    /// Minimum accesses before making predictions
    pub min_accesses_for_prediction: u32,
    /// Weight for recent vs historical data
    pub recency_weight: f64,
    /// Threshold for detecting periodic patterns
    pub periodicity_threshold: f64,
    /// Minimum confidence for predictions
    pub min_prediction_confidence: f64,
    /// Maximum time between accesses to consider burst
    pub burst_threshold: Duration,
}

impl Default for LearningParameters {
    fn default() -> Self {
        Self {
            min_accesses_for_prediction: 5,
            recency_weight: 0.7,
            periodicity_threshold: 0.8,
            min_prediction_confidence: 0.6,
            burst_threshold: Duration::from_secs(300), // 5 minutes
        }
    }
}

#[allow(dead_code)]
impl AccessPatternTracker {
    /// Create new access pattern tracker
    pub fn new(max_events: usize, analysis_window: Duration) -> Self {
        Self {
            file_stats: HashMap::new(),
            collection_patterns: HashMap::new(),
            recent_events: VecDeque::new(),
            max_events,
            analysis_window,
            learning_params: LearningParameters::default(),
        }
    }

    /// Record a file access event
    pub fn record_access(&mut self, event: AccessEvent) {
        let file_key = self.create_file_key(&event.file_path, &event.collection_id);

        // Update file-level statistics
        self.update_file_stats(&file_key, &event);

        // Update collection-level patterns
        self.update_collection_patterns(&event);

        // Log before moving
        trace!(
            file_path = event.file_path,
            collection_id = event.collection_id,
            query_type = ?event.query_type,
            "Recorded file access event"
        );

        // Add to recent events
        self.recent_events.push_back(event);

        // Maintain sliding window
        if self.recent_events.len() > self.max_events {
            self.recent_events.pop_front();
        }

        // Clean up old events
        self.cleanup_old_events();
    }

    /// Predict future access for a file
    pub fn predict_access(
        &self,
        file_path: &str,
        collection_id: &str,
        prediction_window: Duration,
    ) -> AccessPrediction {
        let file_key = self.create_file_key(file_path, collection_id);

        match self.file_stats.get(&file_key) {
            None => AccessPrediction {
                access_probability: 0.1,
                predicted_next_access: None,
                confidence: 0.2,
                predicted_query_type: None,
                prediction_rationale: "No historical data available".to_string(),
            },
            Some(stats) => self.make_prediction(stats, prediction_window),
        }
    }

    /// Get access statistics for a file
    pub fn get_file_stats(&self, file_path: &str, collection_id: &str) -> Option<&AccessStats> {
        let file_key = self.create_file_key(file_path, collection_id);
        self.file_stats.get(&file_key)
    }

    /// Get collection access patterns
    pub fn get_collection_pattern(&self, collection_id: &str) -> Option<&CollectionAccessPattern> {
        self.collection_patterns.get(collection_id)
    }

    /// Get recently accessed files for a collection
    pub fn get_recent_files(&self, collection_id: &str, limit: usize) -> Vec<String> {
        self.recent_events
            .iter()
            .rev() // Most recent first
            .filter(|event| event.collection_id == collection_id)
            .take(limit)
            .map(|event| event.file_path.clone())
            .collect()
    }

    /// Get hot files across all collections
    pub fn get_hot_files(&self, limit: usize) -> Vec<(String, String, f64)> {
        let mut hot_files: Vec<_> = self
            .file_stats
            .iter()
            .map(|(key, stats)| {
                let (file_path, collection_id) = self.parse_file_key(key);
                (file_path, collection_id, stats.access_frequency)
            })
            .collect();

        hot_files.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        hot_files.into_iter().take(limit).collect()
    }

    /// Clear access patterns for a collection
    pub fn clear_collection_patterns(&mut self, collection_id: &str) {
        // Remove file stats for this collection
        let keys_to_remove: Vec<String> = self
            .file_stats
            .keys()
            .filter(|key| {
                let (_, file_collection_id) = self.parse_file_key(key);
                file_collection_id == collection_id
            })
            .cloned()
            .collect();

        for key in keys_to_remove {
            self.file_stats.remove(&key);
        }

        // Remove collection pattern
        self.collection_patterns.remove(collection_id);

        // Remove recent events for this collection
        self.recent_events
            .retain(|event| event.collection_id != collection_id);

        debug!(collection_id, "Cleared access patterns for collection");
    }

    /// Analyze access patterns and generate insights
    pub fn analyze_patterns(&self) -> PatternAnalysis {
        let total_files = self.file_stats.len();
        let total_collections = self.collection_patterns.len();
        let total_events = self.recent_events.len();

        // Find top collections by activity
        let mut active_collections: Vec<_> = self
            .collection_patterns
            .values()
            .map(|pattern| (pattern.collection_id.clone(), pattern.access_velocity))
            .collect();
        active_collections
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Analyze query type distribution
        let mut query_type_counts = HashMap::new();
        for event in &self.recent_events {
            *query_type_counts
                .entry(event.query_type.clone())
                .or_insert(0u64) += 1;
        }

        // Identify trending files
        let trending_files = self.identify_trending_files();

        PatternAnalysis {
            total_files_tracked: total_files,
            total_collections_tracked: total_collections,
            total_events_recorded: total_events,
            active_collections: active_collections.into_iter().take(10).collect(),
            query_type_distribution: query_type_counts,
            trending_files,
            analysis_timestamp: Instant::now(),
        }
    }

    /// Update learning parameters
    pub fn update_learning_parameters(&mut self, params: LearningParameters) {
        self.learning_params = params;
        debug!("Updated learning parameters");
    }

    fn create_file_key(&self, file_path: &str, collection_id: &str) -> String {
        format!("{}:{}", collection_id, file_path)
    }

    fn parse_file_key(&self, key: &str) -> (String, String) {
        if let Some(pos) = key.find(':') {
            let collection_id = key[..pos].to_string();
            let file_path = key[pos + 1..].to_string();
            (file_path, collection_id)
        } else {
            ("".to_string(), key.to_string())
        }
    }

    fn update_file_stats(&mut self, file_key: &str, event: &AccessEvent) {
        let stats = self
            .file_stats
            .entry(file_key.to_string())
            .or_insert_with(|| AccessStats {
                total_accesses: 0,
                last_accessed: event.timestamp,
                first_accessed: event.timestamp,
                access_frequency: 0.0,
                primary_query_type: event.query_type.clone(),
                query_type_distribution: HashMap::new(),
                recent_pattern: VecDeque::new(),
                timing_pattern: TimingPattern::Random,
            });

        stats.total_accesses += 1;
        stats.last_accessed = event.timestamp;

        // Update query type distribution
        *stats
            .query_type_distribution
            .entry(event.query_type.clone())
            .or_insert(0) += 1;

        // Update primary query type
        let max_count = stats.query_type_distribution.values().max().copied();

        if let Some(max_count) = max_count
            && let Some((query_type, _)) = stats
                .query_type_distribution
                .iter()
                .find(|(_, count)| **count == max_count)
        {
            stats.primary_query_type = query_type.clone();
        }

        // Update recent pattern
        stats.recent_pattern.push_back(event.clone());
        if stats.recent_pattern.len() > 10 {
            stats.recent_pattern.pop_front();
        }

        // Calculate access frequency
        let duration = stats.last_accessed.duration_since(stats.first_accessed);
        if duration.as_secs() > 0 {
            stats.access_frequency = stats.total_accesses as f64 / duration.as_secs_f64() * 3600.0;
        }

        // Analyze timing pattern
        let recent_pattern = stats.recent_pattern.clone();
        stats.timing_pattern = Self::analyze_timing_pattern_static(&recent_pattern);
    }

    fn update_collection_patterns(&mut self, event: &AccessEvent) {
        let pattern = self
            .collection_patterns
            .entry(event.collection_id.clone())
            .or_insert_with(|| CollectionAccessPattern {
                collection_id: event.collection_id.clone(),
                files_accessed: 0,
                hot_files: Vec::new(),
                dominant_query_types: Vec::new(),
                access_velocity: 0.0,
                last_activity: event.timestamp,
            });

        pattern.files_accessed += 1;
        pattern.last_activity = event.timestamp;

        // Update access velocity (simple moving average)
        pattern.access_velocity = pattern.access_velocity * 0.9 + 0.1;

        // Update hot files (simplified)
        if !pattern.hot_files.contains(&event.file_path) {
            pattern.hot_files.push(event.file_path.clone());
            if pattern.hot_files.len() > 10 {
                pattern.hot_files.remove(0);
            }
        }
    }

    fn analyze_timing_pattern_static(recent_events: &VecDeque<AccessEvent>) -> TimingPattern {
        if recent_events.len() < 3 {
            return TimingPattern::Random;
        }

        // Calculate intervals between accesses
        let mut intervals = Vec::new();
        for window in recent_events.iter().collect::<Vec<_>>().windows(2) {
            let interval = window[1].timestamp.duration_since(window[0].timestamp);
            intervals.push(interval);
        }

        // Check for periodicity
        if let Some(periodic_interval) = Self::detect_periodicity_static_impl(&intervals) {
            return TimingPattern::Periodic {
                interval: periodic_interval,
                confidence: 0.8,
            };
        }

        // Check for burst pattern
        if Self::is_burst_pattern_static_impl(&intervals) {
            let burst_duration = intervals.iter().take(3).sum::<Duration>() / 3;
            let quiet_duration = Duration::from_secs(3600); // Default 1 hour
            return TimingPattern::Burst {
                burst_duration,
                quiet_duration,
            };
        }

        // Check for trending
        if intervals.len() >= 4 {
            let early_avg = intervals
                .iter()
                .take(intervals.len() / 2)
                .map(|d| d.as_secs_f64())
                .sum::<f64>()
                / (intervals.len() / 2) as f64;

            let late_avg = intervals
                .iter()
                .skip(intervals.len() / 2)
                .map(|d| d.as_secs_f64())
                .sum::<f64>()
                / (intervals.len() - intervals.len() / 2) as f64;

            if late_avg < early_avg * 0.8 {
                return TimingPattern::Trending {
                    growth_rate: (early_avg - late_avg) / early_avg,
                };
            } else if late_avg > early_avg * 1.2 {
                return TimingPattern::Declining {
                    decay_rate: (late_avg - early_avg) / early_avg,
                };
            }
        }

        TimingPattern::Random
    }

    fn detect_periodicity_static(&self, intervals: &[Duration]) -> Option<Duration> {
        Self::detect_periodicity_static_impl(intervals)
    }

    fn detect_periodicity_static_impl(intervals: &[Duration]) -> Option<Duration> {
        if intervals.len() < 3 {
            return None;
        }

        // Simple periodicity detection: check if intervals are similar
        let avg_interval = intervals.iter().sum::<Duration>() / intervals.len() as u32;
        let variance = intervals
            .iter()
            .map(|&interval| {
                let diff = interval.abs_diff(avg_interval);
                diff.as_secs_f64().powi(2)
            })
            .sum::<f64>()
            / intervals.len() as f64;

        let std_dev = variance.sqrt();
        let coefficient_of_variation = std_dev / avg_interval.as_secs_f64();

        // Use a default threshold of 0.2 for periodicity detection
        if coefficient_of_variation < 0.2 {
            Some(avg_interval)
        } else {
            None
        }
    }

    fn is_burst_pattern_static(&self, intervals: &[Duration]) -> bool {
        Self::is_burst_pattern_static_impl(intervals)
    }

    fn is_burst_pattern_static_impl(intervals: &[Duration]) -> bool {
        if intervals.len() < 3 {
            return false;
        }

        // Check if most intervals are very short (indicating burst)
        // Use a default threshold of 100ms for burst detection
        let burst_threshold = Duration::from_millis(100);
        let short_intervals = intervals
            .iter()
            .filter(|&&interval| interval < burst_threshold)
            .count();

        short_intervals as f64 / intervals.len() as f64 > 0.7
    }

    fn make_prediction(
        &self,
        stats: &AccessStats,
        prediction_window: Duration,
    ) -> AccessPrediction {
        if stats.total_accesses < self.learning_params.min_accesses_for_prediction as u64 {
            return AccessPrediction {
                access_probability: 0.3,
                predicted_next_access: None,
                confidence: 0.4,
                predicted_query_type: Some(stats.primary_query_type.clone()),
                prediction_rationale: "Insufficient historical data".to_string(),
            };
        }

        let time_since_last_access = Instant::now().duration_since(stats.last_accessed);

        match &stats.timing_pattern {
            TimingPattern::Periodic {
                interval,
                confidence,
            } => {
                let time_until_next = if time_since_last_access < *interval {
                    *interval - time_since_last_access
                } else {
                    Duration::from_secs(0)
                };

                let probability = if time_until_next < prediction_window {
                    confidence * 0.9
                } else {
                    confidence * 0.3
                };

                AccessPrediction {
                    access_probability: probability,
                    predicted_next_access: Some(time_until_next),
                    confidence: *confidence,
                    predicted_query_type: Some(stats.primary_query_type.clone()),
                    prediction_rationale: format!(
                        "Periodic pattern detected with {:.1}s interval",
                        interval.as_secs_f64()
                    ),
                }
            }

            TimingPattern::Trending { growth_rate } => {
                let probability = (0.8 + growth_rate * 0.2).clamp(0.0, 1.0);
                AccessPrediction {
                    access_probability: probability,
                    predicted_next_access: Some(Duration::from_secs(
                        (3600.0 / stats.access_frequency) as u64,
                    )),
                    confidence: 0.7,
                    predicted_query_type: Some(stats.primary_query_type.clone()),
                    prediction_rationale: format!(
                        "Trending pattern with {:.1}% growth rate",
                        growth_rate * 100.0
                    ),
                }
            }

            TimingPattern::Burst { burst_duration, .. } => {
                let in_burst = time_since_last_access < *burst_duration;
                let probability = if in_burst { 0.8 } else { 0.2 };

                AccessPrediction {
                    access_probability: probability,
                    predicted_next_access: if in_burst {
                        Some(Duration::from_secs(60)) // Soon
                    } else {
                        Some(Duration::from_secs(3600)) // Later
                    },
                    confidence: 0.6,
                    predicted_query_type: Some(stats.primary_query_type.clone()),
                    prediction_rationale: format!(
                        "Burst pattern detected, currently {} burst",
                        if in_burst { "in" } else { "outside" }
                    ),
                }
            }

            _ => {
                // Use frequency-based prediction
                let expected_interval = if stats.access_frequency > 0.0 {
                    Duration::from_secs_f64(3600.0 / stats.access_frequency)
                } else {
                    Duration::from_secs(86400) // 1 day default
                };

                let probability = if time_since_last_access < expected_interval {
                    0.6
                } else {
                    0.3
                };

                AccessPrediction {
                    access_probability: probability,
                    predicted_next_access: Some(expected_interval),
                    confidence: 0.5,
                    predicted_query_type: Some(stats.primary_query_type.clone()),
                    prediction_rationale: format!(
                        "Frequency-based prediction: {:.1} accesses/hour",
                        stats.access_frequency
                    ),
                }
            }
        }
    }

    fn cleanup_old_events(&mut self) {
        let cutoff = Instant::now() - self.analysis_window;
        self.recent_events.retain(|event| event.timestamp > cutoff);
    }

    fn identify_trending_files(&self) -> Vec<(String, String, f64)> {
        self.file_stats
            .iter()
            .filter_map(|(key, stats)| {
                if let TimingPattern::Trending { growth_rate } = &stats.timing_pattern {
                    let (file_path, collection_id) = self.parse_file_key(key);
                    Some((file_path, collection_id, *growth_rate))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Detect periodicity in access intervals
    pub fn detect_periodicity(&self, intervals: &[Duration]) -> Option<Duration> {
        if intervals.len() < 3 {
            return None;
        }

        // Calculate mean and standard deviation
        let sum: u64 = intervals.iter().map(|d| d.as_secs()).sum();
        let mean = sum as f64 / intervals.len() as f64;

        let variance: f64 = intervals
            .iter()
            .map(|d| {
                let diff = d.as_secs() as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / intervals.len() as f64;

        let std_dev = variance.sqrt();

        // If standard deviation is low relative to mean, we have periodicity
        let coefficient_of_variation = std_dev / mean;

        if coefficient_of_variation < self.learning_params.periodicity_threshold {
            Some(Duration::from_secs(mean as u64))
        } else {
            None
        }
    }

    /// Check if intervals represent a burst pattern
    pub fn is_burst_pattern(&self, intervals: &[Duration]) -> bool {
        if intervals.is_empty() {
            return false;
        }

        // Most intervals should be shorter than the burst threshold
        let burst_count = intervals
            .iter()
            .filter(|&interval| interval < &self.learning_params.burst_threshold)
            .count();

        let burst_ratio = burst_count as f64 / intervals.len() as f64;
        burst_ratio > 0.7 // 70% of intervals should be short for burst pattern
    }
}

/// Pattern analysis results
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PatternAnalysis {
    pub total_files_tracked: usize,
    pub total_collections_tracked: usize,
    pub total_events_recorded: usize,
    pub active_collections: Vec<(String, f64)>,
    pub query_type_distribution: HashMap<QueryType, u64>,
    pub trending_files: Vec<(String, String, f64)>,
    pub analysis_timestamp: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_tracker_creation() {
        let tracker = AccessPatternTracker::new(1000, Duration::from_secs(3600));
        assert_eq!(tracker.max_events, 1000);
        assert_eq!(tracker.analysis_window, Duration::from_secs(3600));
    }

    #[test]
    fn test_file_key_creation() {
        let tracker = AccessPatternTracker::new(100, Duration::from_secs(3600));
        let key = tracker.create_file_key("/path/to/file.sst", "collection1");
        assert_eq!(key, "collection1:/path/to/file.sst");

        let (file_path, collection_id) = tracker.parse_file_key(&key);
        assert_eq!(file_path, "/path/to/file.sst");
        assert_eq!(collection_id, "collection1");
    }

    #[test]
    fn test_access_recording() {
        let mut tracker = AccessPatternTracker::new(100, Duration::from_secs(3600));

        let event = AccessEvent {
            file_path: "/test/file.sst".to_string(),
            collection_id: "test_collection".to_string(),
            query_type: QueryType::SimilaritySearch,
            timestamp: Instant::now(),
            result_type: "full_download".to_string(),
        };

        tracker.record_access(event.clone());

        assert_eq!(tracker.recent_events.len(), 1);

        let stats = tracker.get_file_stats(&event.file_path, &event.collection_id);
        assert!(stats.is_some());

        let stats = stats.unwrap();
        assert_eq!(stats.total_accesses, 1);
        assert_eq!(stats.primary_query_type, QueryType::SimilaritySearch);
    }

    #[test]
    fn test_prediction_with_no_data() {
        let tracker = AccessPatternTracker::new(100, Duration::from_secs(3600));

        let prediction = tracker.predict_access(
            "/unknown/file.sst",
            "unknown_collection",
            Duration::from_secs(300),
        );

        assert!(prediction.access_probability < 0.5);
        assert!(prediction.confidence < 0.5);
        assert!(prediction.predicted_query_type.is_none());
    }

    #[test]
    fn test_periodicity_detection() {
        let tracker = AccessPatternTracker::new(100, Duration::from_secs(3600));

        // Create intervals with similar duration (periodic pattern)
        let intervals = vec![
            Duration::from_secs(300),
            Duration::from_secs(310),
            Duration::from_secs(295),
            Duration::from_secs(305),
        ];

        let periodic_interval = tracker.detect_periodicity(&intervals);
        assert!(periodic_interval.is_some());

        let interval = periodic_interval.unwrap();
        assert!(interval.as_secs() >= 300 && interval.as_secs() <= 310);
    }

    #[test]
    fn test_burst_pattern_detection() {
        let tracker = AccessPatternTracker::new(100, Duration::from_secs(3600));

        // Create short intervals (burst pattern)
        let intervals = vec![
            Duration::from_secs(30),
            Duration::from_secs(45),
            Duration::from_secs(20),
            Duration::from_secs(60),
        ];

        let is_burst = tracker.is_burst_pattern(&intervals);
        assert!(is_burst);

        // Create long intervals (not burst)
        let intervals = vec![
            Duration::from_secs(3600),
            Duration::from_secs(7200),
            Duration::from_secs(1800),
        ];

        let is_burst = tracker.is_burst_pattern(&intervals);
        assert!(!is_burst);
    }
}
