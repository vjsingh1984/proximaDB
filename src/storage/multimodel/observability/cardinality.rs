//! # High-Cardinality Label Optimization
//!
//! Detects and limits high-cardinality labels to prevent
//! index explosion and memory issues.

use std::collections::{HashMap, HashSet};
use anyhow::Result;
use tokio::sync::RwLock;
use tracing::warn;

/// Configuration for cardinality limiting
#[derive(Debug, Clone)]
pub struct CardinalityConfig {
    /// Maximum unique values per label
    pub max_cardinality_per_label: usize,
    /// Maximum total cardinality across all labels
    pub max_total_cardinality: usize,
    /// Labels exempt from limiting (e.g., "namespace", "cluster")
    pub exempt_labels: HashSet<String>,
    /// Enable automatic high-cardinality detection
    pub auto_detect: bool,
    /// High-cardinality threshold (percentage of total samples)
    pub high_cardinality_threshold: f64,
    /// Action when limit exceeded
    pub on_limit_exceeded: LimitAction,
}

impl Default for CardinalityConfig {
    fn default() -> Self {
        let mut exempt = HashSet::new();
        exempt.insert("namespace".to_string());
        exempt.insert("cluster".to_string());
        exempt.insert("datacenter".to_string());
        exempt.insert("env".to_string());
        exempt.insert("environment".to_string());

        Self {
            max_cardinality_per_label: 10_000,
            max_total_cardinality: 100_000,
            exempt_labels: exempt,
            auto_detect: true,
            high_cardinality_threshold: 0.5, // 50% of samples have unique value
            on_limit_exceeded: LimitAction::Truncate,
        }
    }
}

/// Action to take when cardinality limit is exceeded
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitAction {
    /// Drop the high-cardinality label entirely
    Drop,
    /// Truncate value to a hash
    Truncate,
    /// Replace with "__overflow__"
    Replace,
    /// Reject the sample
    Reject,
    /// Log warning and continue
    Warn,
}

/// Statistics for a single label
#[derive(Debug, Clone, Default)]
pub struct LabelStats {
    /// Label name
    pub name: String,
    /// Number of unique values
    pub unique_values: usize,
    /// Total samples with this label
    pub total_samples: u64,
    /// Is this label considered high-cardinality
    pub is_high_cardinality: bool,
    /// Number of values that were dropped/truncated
    pub overflow_count: u64,
    /// Most common values (top 10)
    pub top_values: Vec<(String, u64)>,
}

impl LabelStats {
    /// Create new label stats
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Calculate cardinality ratio
    pub fn cardinality_ratio(&self) -> f64 {
        if self.total_samples == 0 {
            0.0
        } else {
            self.unique_values as f64 / self.total_samples as f64
        }
    }
}

/// Cardinality limiter tracks and limits label cardinality
pub struct CardinalityLimiter {
    /// Configuration
    config: CardinalityConfig,

    /// Label value sets (label_name -> set of values)
    label_values: RwLock<HashMap<String, HashSet<String>>>,

    /// Label statistics
    label_stats: RwLock<HashMap<String, LabelStats>>,

    /// High-cardinality labels detected
    high_cardinality_labels: RwLock<HashSet<String>>,

    /// Total samples processed
    total_samples: std::sync::atomic::AtomicU64,

    /// Samples dropped due to limits
    dropped_samples: std::sync::atomic::AtomicU64,
}

impl CardinalityLimiter {
    /// Create a new cardinality limiter
    pub fn new(config: CardinalityConfig) -> Self {
        Self {
            config,
            label_values: RwLock::new(HashMap::new()),
            label_stats: RwLock::new(HashMap::new()),
            high_cardinality_labels: RwLock::new(HashSet::new()),
            total_samples: std::sync::atomic::AtomicU64::new(0),
            dropped_samples: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Check and potentially modify labels to enforce cardinality limits
    pub async fn check_labels(
        &self,
        labels: &mut HashMap<String, String>,
    ) -> Result<CheckResult> {
        self.total_samples.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut modified_labels = Vec::new();
        let mut warnings = Vec::new();

        for (label_name, label_value) in labels.iter_mut() {
            // Skip exempt labels
            if self.config.exempt_labels.contains(label_name) {
                continue;
            }

            // Check if already known high-cardinality
            {
                let high_card = self.high_cardinality_labels.read().await;
                if high_card.contains(label_name) {
                    // Apply limit action
                    match self.config.on_limit_exceeded {
                        LimitAction::Drop => {
                            modified_labels.push(label_name.clone());
                            continue;
                        }
                        LimitAction::Truncate => {
                            *label_value = self.truncate_value(label_value);
                            modified_labels.push(label_name.clone());
                        }
                        LimitAction::Replace => {
                            *label_value = "__overflow__".to_string();
                            modified_labels.push(label_name.clone());
                        }
                        LimitAction::Reject => {
                            return Ok(CheckResult::Rejected(format!(
                                "High-cardinality label: {}",
                                label_name
                            )));
                        }
                        LimitAction::Warn => {
                            warnings.push(format!(
                                "High-cardinality label: {}",
                                label_name
                            ));
                        }
                    }
                    continue;
                }
            }

            // Track this value
            let is_new = {
                let mut values = self.label_values.write().await;
                let label_set = values.entry(label_name.clone()).or_default();

                if label_set.contains(label_value) {
                    false
                } else if label_set.len() < self.config.max_cardinality_per_label {
                    label_set.insert(label_value.clone());
                    true
                } else {
                    // Limit exceeded - mark as high-cardinality
                    drop(values);

                    let mut high_card = self.high_cardinality_labels.write().await;
                    high_card.insert(label_name.clone());

                    warn!(
                        "Label '{}' exceeded cardinality limit ({}), marking as high-cardinality",
                        label_name, self.config.max_cardinality_per_label
                    );

                    // Apply action
                    match self.config.on_limit_exceeded {
                        LimitAction::Truncate => {
                            *label_value = self.truncate_value(label_value);
                        }
                        LimitAction::Replace => {
                            *label_value = "__overflow__".to_string();
                        }
                        _ => {}
                    }

                    modified_labels.push(label_name.clone());
                    false
                }
            };

            // Update stats
            {
                let mut stats = self.label_stats.write().await;
                let label_stat = stats.entry(label_name.clone()).or_insert_with(|| {
                    LabelStats::new(label_name.clone())
                });
                label_stat.total_samples += 1;
                if is_new {
                    label_stat.unique_values += 1;
                }

                // Check for high cardinality pattern
                if self.config.auto_detect && label_stat.total_samples > 100 {
                    let ratio = label_stat.cardinality_ratio();
                    if ratio > self.config.high_cardinality_threshold {
                        let mut high_card = self.high_cardinality_labels.write().await;
                        if !high_card.contains(label_name) {
                            high_card.insert(label_name.clone());
                            label_stat.is_high_cardinality = true;
                            warn!(
                                "Auto-detected high-cardinality label '{}' (ratio: {:.2})",
                                label_name, ratio
                            );
                        }
                    }
                }
            }
        }

        // Remove dropped labels
        for label in &modified_labels {
            if self.config.on_limit_exceeded == LimitAction::Drop {
                labels.remove(label);
            }
        }

        if !warnings.is_empty() {
            Ok(CheckResult::Warning(warnings))
        } else if !modified_labels.is_empty() {
            Ok(CheckResult::Modified(modified_labels))
        } else {
            Ok(CheckResult::Passed)
        }
    }

    /// Truncate a value using hash
    fn truncate_value(&self, value: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let hash = hasher.finish();

        format!("__hash_{:016x}__", hash)
    }

    /// Get statistics for a label
    pub async fn get_label_stats(&self, label_name: &str) -> Option<LabelStats> {
        let stats = self.label_stats.read().await;
        stats.get(label_name).cloned()
    }

    /// Get all label statistics
    pub async fn get_all_stats(&self) -> Vec<LabelStats> {
        let stats = self.label_stats.read().await;
        stats.values().cloned().collect()
    }

    /// Get high-cardinality labels
    pub async fn get_high_cardinality_labels(&self) -> Vec<String> {
        let high_card = self.high_cardinality_labels.read().await;
        high_card.iter().cloned().collect()
    }

    /// Get total samples processed
    pub fn total_samples(&self) -> u64 {
        self.total_samples.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get dropped samples count
    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get configuration
    pub fn config(&self) -> &CardinalityConfig {
        &self.config
    }

    /// Reset all tracking (for testing)
    #[cfg(test)]
    pub async fn reset(&self) {
        self.label_values.write().await.clear();
        self.label_stats.write().await.clear();
        self.high_cardinality_labels.write().await.clear();
        self.total_samples.store(0, std::sync::atomic::Ordering::Relaxed);
        self.dropped_samples.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Result of cardinality check
#[derive(Debug, Clone)]
pub enum CheckResult {
    /// Labels passed without modification
    Passed,
    /// Labels were modified
    Modified(Vec<String>),
    /// Warning issued but sample accepted
    Warning(Vec<String>),
    /// Sample rejected
    Rejected(String),
}

impl CheckResult {
    /// Check if result is success (not rejected)
    pub fn is_success(&self) -> bool {
        !matches!(self, CheckResult::Rejected(_))
    }
}

impl Default for CardinalityLimiter {
    fn default() -> Self {
        Self::new(CardinalityConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cardinality_config_default() {
        let config = CardinalityConfig::default();
        assert_eq!(config.max_cardinality_per_label, 10_000);
        assert!(config.exempt_labels.contains("namespace"));
    }

    #[tokio::test]
    async fn test_check_labels_pass() {
        let limiter = CardinalityLimiter::new(CardinalityConfig::default());

        let mut labels = HashMap::from([
            ("host".to_string(), "server1".to_string()),
            ("env".to_string(), "prod".to_string()),
        ]);

        let result = limiter.check_labels(&mut labels).await.unwrap();
        assert!(result.is_success());
        assert!(matches!(result, CheckResult::Passed));
    }

    #[tokio::test]
    async fn test_high_cardinality_detection() {
        let config = CardinalityConfig {
            max_cardinality_per_label: 5,
            on_limit_exceeded: LimitAction::Replace,
            ..Default::default()
        };

        let limiter = CardinalityLimiter::new(config);

        // Add values up to limit
        for i in 0..10 {
            let mut labels = HashMap::from([
                ("user_id".to_string(), format!("user_{}", i)),
            ]);
            limiter.check_labels(&mut labels).await.unwrap();
        }

        // Check that label was marked high-cardinality
        let high_card = limiter.get_high_cardinality_labels().await;
        assert!(high_card.contains(&"user_id".to_string()));
    }

    #[tokio::test]
    async fn test_exempt_labels() {
        let config = CardinalityConfig {
            max_cardinality_per_label: 2,
            ..Default::default()
        };

        let limiter = CardinalityLimiter::new(config);

        // "namespace" is exempt, should not trigger limit
        for i in 0..10 {
            let mut labels = HashMap::from([
                ("namespace".to_string(), format!("ns_{}", i)),
            ]);
            limiter.check_labels(&mut labels).await.unwrap();
        }

        let high_card = limiter.get_high_cardinality_labels().await;
        assert!(!high_card.contains(&"namespace".to_string()));
    }

    #[tokio::test]
    async fn test_label_stats() {
        let limiter = CardinalityLimiter::new(CardinalityConfig::default());

        // Add some samples
        for _ in 0..5 {
            let mut labels = HashMap::from([
                ("host".to_string(), "server1".to_string()),
            ]);
            limiter.check_labels(&mut labels).await.unwrap();
        }

        for _ in 0..3 {
            let mut labels = HashMap::from([
                ("host".to_string(), "server2".to_string()),
            ]);
            limiter.check_labels(&mut labels).await.unwrap();
        }

        let stats = limiter.get_label_stats("host").await.unwrap();
        assert_eq!(stats.total_samples, 8);
        assert_eq!(stats.unique_values, 2);
    }

    #[test]
    fn test_check_result_is_success() {
        assert!(CheckResult::Passed.is_success());
        assert!(CheckResult::Modified(vec![]).is_success());
        assert!(CheckResult::Warning(vec![]).is_success());
        assert!(!CheckResult::Rejected("reason".to_string()).is_success());
    }
}
