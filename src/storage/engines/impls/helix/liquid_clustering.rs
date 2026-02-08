//! Liquid clustering implementation for adaptive data reorganization
//!
//! This module implements liquid clustering that adapts data organization
//! based on query patterns to optimize for frequently accessed regions.

use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use super::clustering::{HilbertKey, LiquidClusteringConfig, QueryPatternTracker};
use crate::proto::proximadb_v1::VectorRecord;

/// Liquid clustering coordinator
pub struct LiquidClusteringCoordinator {
    config: LiquidClusteringConfig,
    query_tracker: Arc<RwLock<QueryPatternTracker>>,
    cluster_stats: Arc<RwLock<ClusterStatistics>>,
}

/// Statistics for cluster optimization
#[derive(Debug, Default)]
pub struct ClusterStatistics {
    /// Hilbert range -> access count
    pub range_access_counts: BTreeMap<(HilbertKey, HilbertKey), usize>,
    /// Vector ID -> cluster assignment
    pub vector_clusters: HashMap<String, ClusterInfo>,
    /// Cluster quality score (0.0 - 1.0)
    pub clustering_quality: f32,
    /// Number of re-clustering operations
    pub recluster_count: u64,
    /// Last re-clustering time
    pub last_recluster: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone)]
pub struct ClusterInfo {
    pub cluster_id: u32,
    pub hilbert_key: HilbertKey,
    pub access_count: usize,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
}

impl LiquidClusteringCoordinator {
    pub fn new(
        config: LiquidClusteringConfig,
        query_tracker: Arc<RwLock<QueryPatternTracker>>,
    ) -> Self {
        Self {
            config,
            query_tracker,
            cluster_stats: Arc::new(RwLock::new(ClusterStatistics::default())),
        }
    }

    /// Apply liquid clustering to reorganize data based on access patterns
    pub async fn apply_liquid_clustering(
        &self,
        records: Vec<VectorRecord>,
        hilbert_keys: &[HilbertKey],
    ) -> Result<(Vec<VectorRecord>, Vec<HilbertKey>)> {
        if !self.config.enabled || records.is_empty() {
            return Ok((records, hilbert_keys.to_vec()));
        }

        let tracker = self.query_tracker.read().await;
        let stats = self.cluster_stats.read().await;

        // Get access patterns for these records
        let vector_ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
        let clustering_hints = tracker.get_clustering_hints(&vector_ids, &self.config);

        // Identify hot regions in Hilbert space
        let hot_regions = tracker.identify_hot_regions(0.01); // 1% threshold

        info!(
            "Liquid clustering: {} hot regions identified from {} queries",
            hot_regions.len(),
            tracker.total_queries
        );

        // Create cluster assignments based on access patterns
        let assignments = self.compute_cluster_assignments(
            &records,
            hilbert_keys,
            &clustering_hints,
            &hot_regions,
        );

        // Reorganize records based on assignments
        let (reorganized_records, new_keys) =
            self.reorganize_by_clusters(records, hilbert_keys, assignments);

        // Update statistics
        drop(stats);
        let mut stats = self.cluster_stats.write().await;
        stats.recluster_count += 1;
        stats.last_recluster = Some(chrono::Utc::now());

        Ok((reorganized_records, new_keys))
    }

    /// Compute cluster assignments based on access patterns
    fn compute_cluster_assignments(
        &self,
        records: &[VectorRecord],
        hilbert_keys: &[HilbertKey],
        access_scores: &HashMap<String, f32>,
        hot_regions: &[(HilbertKey, HilbertKey)],
    ) -> Vec<ClusterAssignment> {
        let mut assignments = Vec::new();

        for (i, record) in records.iter().enumerate() {
            let hilbert_key = hilbert_keys[i];
            let access_score = access_scores.get(&record.id).copied().unwrap_or(0.0);

            // Determine cluster based on access patterns and Hilbert location
            let cluster_id = self.determine_cluster(hilbert_key, access_score, hot_regions);

            // Calculate priority within cluster
            let priority = self.calculate_priority(access_score, hilbert_key);

            assignments.push(ClusterAssignment {
                record_index: i,
                cluster_id,
                priority,
                hilbert_key,
            });
        }

        assignments
    }

    /// Determine which cluster a record belongs to
    fn determine_cluster(
        &self,
        hilbert_key: HilbertKey,
        access_score: f32,
        hot_regions: &[(HilbertKey, HilbertKey)],
    ) -> u32 {
        // Check if in hot region
        for (idx, &(min_key, max_key)) in hot_regions.iter().enumerate() {
            if hilbert_key >= min_key && hilbert_key <= max_key {
                return idx as u32; // Hot cluster
            }
        }

        // Assign to cold cluster based on access score
        if access_score < 0.1 {
            return u32::MAX; // Cold cluster
        }

        // Medium access cluster
        (hot_regions.len() + 1) as u32
    }

    /// Calculate priority for ordering within cluster
    fn calculate_priority(&self, access_score: f32, hilbert_key: HilbertKey) -> f64 {
        // Combine access score with Hilbert key for stable ordering
        (access_score as f64 * 1e9) + (hilbert_key as f64 / u64::MAX as f64)
    }

    /// Reorganize records based on cluster assignments
    fn reorganize_by_clusters(
        &self,
        records: Vec<VectorRecord>,
        hilbert_keys: &[HilbertKey],
        mut assignments: Vec<ClusterAssignment>,
    ) -> (Vec<VectorRecord>, Vec<HilbertKey>) {
        // Sort assignments by cluster and priority
        assignments.sort_by(|a, b| {
            a.cluster_id
                .cmp(&b.cluster_id)
                .then(b.priority.partial_cmp(&a.priority).unwrap())
        });

        // Reorder records and keys based on assignments
        let mut new_records = Vec::with_capacity(records.len());
        let mut new_keys = Vec::with_capacity(hilbert_keys.len());

        for assignment in assignments {
            new_records.push(records[assignment.record_index].clone());
            new_keys.push(assignment.hilbert_key);
        }

        (new_records, new_keys)
    }

    /// Check if re-clustering is needed based on access patterns
    pub async fn should_recluster(&self) -> bool {
        let tracker = self.query_tracker.read().await;
        let stats = self.cluster_stats.read().await;

        // Check if enough queries have been processed
        if tracker.total_queries < self.config.recluster_threshold as usize {
            return false;
        }

        // Check time since last re-clustering
        if let Some(last) = stats.last_recluster {
            let elapsed = chrono::Utc::now() - last;
            if elapsed.num_hours() < 1 {
                return false; // Don't re-cluster too frequently
            }
        }

        // Check clustering quality degradation
        if stats.clustering_quality < 0.5 {
            info!(
                "Clustering quality degraded to {:.2}",
                stats.clustering_quality
            );
            return true;
        }

        false
    }

    /// Calculate clustering quality score
    pub async fn calculate_clustering_quality(
        &self,
        records: &[VectorRecord],
        hilbert_keys: &[HilbertKey],
    ) -> f32 {
        if records.is_empty() {
            return 1.0;
        }

        let tracker = self.query_tracker.read().await;

        // Calculate locality score (nearby vectors should have similar access patterns)
        let mut locality_score = 0.0;
        let window_size = 10;

        for i in 0..records.len().saturating_sub(window_size) {
            let window = &records[i..i + window_size];
            let _window_keys = &hilbert_keys[i..i + window_size];

            // Check if vectors in window have similar access counts
            let access_counts: Vec<usize> = window
                .iter()
                .map(|r| tracker.access_counts.get(&r.id).copied().unwrap_or(0))
                .collect();

            let mean_access = access_counts.iter().sum::<usize>() as f32 / window_size as f32;
            let variance: f32 = access_counts
                .iter()
                .map(|&c| (c as f32 - mean_access).powi(2))
                .sum::<f32>()
                / window_size as f32;

            // Lower variance within windows = better clustering
            locality_score += 1.0 / (1.0 + variance);
        }

        let num_windows = records.len().saturating_sub(window_size).max(1);
        locality_score / num_windows as f32
    }

    /// Get optimization suggestions based on current patterns
    pub async fn get_optimization_suggestions(&self) -> Vec<OptimizationSuggestion> {
        let mut suggestions = Vec::new();
        let tracker = self.query_tracker.read().await;
        let stats = self.cluster_stats.read().await;

        // Suggest PCA retraining if access patterns have shifted
        let hot_regions = tracker.identify_hot_regions(0.01);
        if hot_regions.len() > 10 {
            suggestions.push(OptimizationSuggestion {
                suggestion_type: SuggestionType::PcaRetrain,
                reason: format!(
                    "High fragmentation: {} hot regions detected",
                    hot_regions.len()
                ),
                estimated_improvement: 0.3,
            });
        }

        // Suggest block size adjustment based on access patterns
        let avg_access_per_vector =
            tracker.total_queries as f32 / tracker.access_counts.len().max(1) as f32;

        if avg_access_per_vector > 100.0 {
            suggestions.push(OptimizationSuggestion {
                suggestion_type: SuggestionType::BlockSizeDecrease,
                reason: "High access rate suggests smaller blocks for better cache utilization"
                    .to_string(),
                estimated_improvement: 0.2,
            });
        }

        // Suggest compaction if clustering quality is low
        if stats.clustering_quality < 0.6 {
            suggestions.push(OptimizationSuggestion {
                suggestion_type: SuggestionType::ForceCompaction,
                reason: format!(
                    "Clustering quality {:.2} below threshold",
                    stats.clustering_quality
                ),
                estimated_improvement: 0.4,
            });
        }

        suggestions
    }
}

#[derive(Debug)]
struct ClusterAssignment {
    record_index: usize,
    cluster_id: u32,
    priority: f64,
    hilbert_key: HilbertKey,
}

#[derive(Debug, Clone)]
pub struct OptimizationSuggestion {
    pub suggestion_type: SuggestionType,
    pub reason: String,
    pub estimated_improvement: f32,
}

#[derive(Debug, Clone)]
pub enum SuggestionType {
    PcaRetrain,
    BlockSizeIncrease,
    BlockSizeDecrease,
    ForceCompaction,
    EnableQuantization,
}

/// Liquid clustering metrics for monitoring
#[derive(Debug, Default)]
pub struct LiquidClusteringMetrics {
    pub total_reorganizations: u64,
    pub vectors_moved: u64,
    pub hot_regions_tracked: usize,
    pub clustering_quality: f32,
    pub avg_access_locality: f32,
}

impl LiquidClusteringMetrics {
    pub fn update(&mut self, quality: f32, hot_regions: usize) {
        self.clustering_quality = quality;
        self.hot_regions_tracked = hot_regions;
    }

    pub fn record_reorganization(&mut self, vectors_moved: usize) {
        self.total_reorganizations += 1;
        self.vectors_moved += vectors_moved as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_liquid_clustering() {
        let config = LiquidClusteringConfig::default();
        let query_tracker = Arc::new(RwLock::new(QueryPatternTracker::default()));

        // Record some access patterns
        {
            let mut tracker = query_tracker.write().await;
            tracker.record_access("vec1", 100);
            tracker.record_access("vec1", 100);
            tracker.record_access("vec2", 200);
            tracker.record_access("vec3", 300);
        }

        let coordinator = LiquidClusteringCoordinator::new(config, query_tracker);

        // Create test records
        let records = vec![
            VectorRecord {
                id: "vec1".to_string(),
                vector: vec![1.0, 2.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "vec2".to_string(),
                vector: vec![3.0, 4.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "vec3".to_string(),
                vector: vec![5.0, 6.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
        ];

        let hilbert_keys = vec![100, 200, 300];

        // Apply liquid clustering
        let (reorganized, new_keys) = coordinator
            .apply_liquid_clustering(records.clone(), &hilbert_keys)
            .await
            .unwrap();

        assert_eq!(reorganized.len(), records.len());
        assert_eq!(new_keys.len(), hilbert_keys.len());
    }

    #[tokio::test]
    async fn test_clustering_quality() {
        let config = LiquidClusteringConfig::default();
        let query_tracker = Arc::new(RwLock::new(QueryPatternTracker::default()));
        let coordinator = LiquidClusteringCoordinator::new(config, query_tracker);

        let records = vec![VectorRecord {
            id: "vec1".to_string(),
            vector: vec![1.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0i64),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        }];

        let hilbert_keys = vec![100];

        let quality = coordinator
            .calculate_clustering_quality(&records, &hilbert_keys)
            .await;

        assert!(quality >= 0.0 && quality <= 1.0);
    }
}
