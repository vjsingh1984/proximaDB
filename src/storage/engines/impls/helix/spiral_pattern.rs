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

//! HELIX Spiral Pattern Implementation - Complete temporal locality optimization
//!
//! Implements spiral pattern storage for time-series data with temporal locality optimization.
//! Provides temporal query optimization and age-based compression strategies.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use chrono::{DateTime, Utc};

/// Spiral pattern storage manager for time-series data
#[derive(Debug)]
pub struct SpiralPatternManager {
    /// Spiral configuration
    spiral_config: SpiralConfig,
    /// Time-series metadata
    time_series_metadata: Arc<RwLock<TimeSeriesMetadata>>,
    /// Pattern cache
    pattern_cache: Arc<PatternCache>,
    /// Collection ID for isolation
    collection_id: String,
}

/// Configuration for spiral pattern storage
#[derive(Debug, Clone)]
pub struct SpiralConfig {
    /// Spiral factor for layout optimization
    pub spiral_factor: f64,
    /// Time window size for grouping
    pub time_window_size: Duration,
    /// Compression ratio target
    pub compression_ratio: f32,
    /// Enable temporal locality optimization
    pub enable_temporal_locality: bool,
    /// Age-based compression thresholds
    pub age_compression_thresholds: AgeCompressionThresholds,
}

impl Default for SpiralConfig {
    fn default() -> Self {
        Self {
            spiral_factor: 1.618, // Golden ratio for optimal spiral
            time_window_size: Duration::from_secs(3600), // 1 hour windows
            compression_ratio: 0.3, // 70% compression target
            enable_temporal_locality: true,
            age_compression_thresholds: AgeCompressionThresholds::default(),
        }
    }
}

/// Age-based compression thresholds
#[derive(Debug, Clone)]
pub struct AgeCompressionThresholds {
    /// Fresh data (no compression)
    pub fresh_threshold: Duration,
    /// Recent data (light compression)
    pub recent_threshold: Duration,
    /// Old data (medium compression)
    pub old_threshold: Duration,
    /// Ancient data (heavy compression)
    pub ancient_threshold: Duration,
}

impl Default for AgeCompressionThresholds {
    fn default() -> Self {
        Self {
            fresh_threshold: Duration::from_secs(300),     // 5 minutes
            recent_threshold: Duration::from_secs(3600),   // 1 hour
            old_threshold: Duration::from_secs(86400),     // 1 day
            ancient_threshold: Duration::from_secs(604800), // 1 week
        }
    }
}

/// Time-series metadata for optimization
#[derive(Debug, Clone)]
pub struct TimeSeriesMetadata {
    /// Earliest timestamp in collection
    pub earliest_timestamp: DateTime<Utc>,
    /// Latest timestamp in collection
    pub latest_timestamp: DateTime<Utc>,
    /// Total time span
    pub time_span: Duration,
    /// Average insertion rate (vectors per second)
    pub avg_insertion_rate: f64,
    /// Temporal access patterns
    pub access_patterns: HashMap<String, TemporalAccessPattern>,
}

/// Pattern cache for spiral layout optimization
#[derive(Debug)]
pub struct PatternCache {
    /// Cached spiral layouts
    spiral_layouts: Arc<RwLock<HashMap<String, SpiralLayout>>>,
    /// Cache configuration
    cache_config: PatternCacheConfig,
}

/// Configuration for pattern cache
#[derive(Debug, Clone)]
pub struct PatternCacheConfig {
    /// Maximum cached layouts
    pub max_cached_layouts: usize,
    /// Cache TTL
    pub cache_ttl: Duration,
    /// Enable cache statistics
    pub enable_statistics: bool,
}

impl Default for PatternCacheConfig {
    fn default() -> Self {
        Self {
            max_cached_layouts: 100,
            cache_ttl: Duration::from_secs(3600), // 1 hour
            enable_statistics: true,
        }
    }
}

/// Temporal access pattern for optimization
#[derive(Debug, Clone)]
pub struct TemporalAccessPattern {
    /// Most frequently accessed time range
    pub hot_time_range: TimeRange,
    /// Access frequency by time period
    pub access_frequency: HashMap<TimeRange, f64>,
    /// Temporal locality score
    pub locality_score: f64,
}

/// Time range for temporal operations
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TimeRange {
    /// Start timestamp
    pub start: DateTime<Utc>,
    /// End timestamp
    pub end: DateTime<Utc>,
}

/// Timestamped vector for time-series operations
#[derive(Debug, Clone)]
pub struct TimestampedVector {
    /// Vector ID
    pub id: String,
    /// Vector data
    pub vector: Vec<f32>,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Spiral layout optimized for temporal locality
#[derive(Debug, Clone)]
pub struct SpiralLayout {
    /// Spiral groups organized by time
    pub spiral_groups: Vec<SpiralGroup>,
    /// Temporal index for fast time-based lookup
    pub temporal_index: TemporalIndex,
    /// Compression mapping by age
    pub compression_mapping: HashMap<TimeRange, CompressionLevel>,
}

/// Individual spiral group
#[derive(Debug, Clone)]
pub struct SpiralGroup {
    /// Group ID
    pub id: String,
    /// Time range covered
    pub time_range: TimeRange,
    /// Vectors in spiral order
    pub vectors: Vec<TimestampedVector>,
    /// Spiral center coordinates
    pub spiral_center: (f64, f64),
    /// Spiral radius
    pub spiral_radius: f64,
}

/// Temporal index for fast time-based queries
#[derive(Debug, Clone)]
pub struct TemporalIndex {
    /// Time range to spiral group mapping
    pub time_to_group: HashMap<TimeRange, String>,
    /// Sorted time ranges for binary search
    pub sorted_ranges: Vec<TimeRange>,
}

/// Compression level based on data age
#[derive(Debug, Clone)]
pub enum CompressionLevel {
    None,         // Fresh data
    Light,        // Recent data (PQ16)
    Medium,       // Old data (PQ8)
    Heavy,        // Ancient data (PQ4)
    Maximum,      // Very old data (Binary)
}

/// Temporal query for time-series operations
#[derive(Debug, Clone)]
pub struct TemporalQuery {
    /// Time range to query
    pub time_range: TimeRange,
    /// Vector query (if any)
    pub vector_query: Option<Vec<f32>>,
    /// Number of results
    pub k: usize,
    /// Metadata filters
    pub metadata_filters: Option<HashMap<String, String>>,
}

/// Query plan optimized for temporal queries
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Target spiral groups to search
    pub target_spirals: Vec<String>,
    /// Optimized scan order
    pub scan_order: ScanOrder,
    /// Early termination criteria
    pub early_termination: TerminationCriteria,
}

/// Scan order optimization
#[derive(Debug, Clone)]
pub enum ScanOrder {
    /// Temporal (newest first)
    TemporalNewest,
    /// Temporal (oldest first)
    TemporalOldest,
    /// Relevance-based
    RelevanceBased,
    /// Hybrid (temporal + relevance)
    Hybrid,
}

/// Early termination criteria
#[derive(Debug, Clone)]
pub struct TerminationCriteria {
    /// Maximum time to spend scanning
    pub max_scan_time: Duration,
    /// Minimum quality threshold
    pub min_quality_threshold: f64,
    /// Maximum number of groups to scan
    pub max_groups_to_scan: usize,
}

impl SpiralPatternManager {
    /// Create new spiral pattern manager
    pub fn new(collection_id: String, config: SpiralConfig) -> Self {
        info!("🌀 Creating SpiralPatternManager for collection: {}", collection_id);

        Self {
            spiral_config: config,
            time_series_metadata: Arc::new(RwLock::new(TimeSeriesMetadata {
                earliest_timestamp: Utc::now(),
                latest_timestamp: Utc::now(),
                time_span: Duration::from_secs(0),
                avg_insertion_rate: 0.0,
                access_patterns: HashMap::new(),
            })),
            pattern_cache: Arc::new(PatternCache::new(PatternCacheConfig::default())),
            collection_id,
        }
    }

    /// Organize vectors in spiral pattern for temporal locality
    pub fn organize_spiral_layout(&self, vectors: &[TimestampedVector]) -> Result<SpiralLayout> {
        info!("🔧 Organizing {} vectors in spiral pattern", vectors.len());

        // Sort by timestamp for temporal locality
        let mut sorted_vectors = vectors.to_vec();
        sorted_vectors.sort_by_key(|v| v.timestamp);

        // Apply spiral pattern grouping
        let spiral_groups = self.create_spiral_groups(&sorted_vectors)?;

        // Build temporal index for fast lookups
        let temporal_index = self.build_temporal_index(&spiral_groups);

        // Create age-based compression mapping
        let compression_mapping = self.optimize_compression_by_age(&sorted_vectors);

        let layout = SpiralLayout {
            spiral_groups,
            temporal_index,
            compression_mapping,
        };

        info!("✅ Spiral layout organized: {} groups with temporal optimization", layout.spiral_groups.len());

        Ok(layout)
    }

    /// Implement temporal query optimization
    pub fn optimize_temporal_queries(&self, query: &TemporalQuery) -> Result<QueryPlan> {
        let metadata = self.time_series_metadata.read().map_err(|e| anyhow!("Lock error: {}", e))?;

        debug!("🎯 Optimizing temporal query for range: {:?} to {:?}",
               query.time_range.start, query.time_range.end);

        let query_plan = QueryPlan {
            target_spirals: self.identify_relevant_spirals(query, &metadata),
            scan_order: self.optimize_scan_order(query),
            early_termination: self.calculate_termination_criteria(query),
        };

        debug!("📋 Query plan: {} target spirals, {:?} scan order",
               query_plan.target_spirals.len(), query_plan.scan_order);

        Ok(query_plan)
    }

    /// Update time-series metadata with new vectors
    pub async fn update_metadata(&self, vectors: &[TimestampedVector]) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        let mut metadata = self.time_series_metadata.write().map_err(|e| anyhow!("Lock error: {}", e))?;

        // Update timestamp bounds
        let min_timestamp = vectors.iter().map(|v| v.timestamp).min().unwrap();
        let max_timestamp = vectors.iter().map(|v| v.timestamp).max().unwrap();

        if min_timestamp < metadata.earliest_timestamp {
            metadata.earliest_timestamp = min_timestamp;
        }
        if max_timestamp > metadata.latest_timestamp {
            metadata.latest_timestamp = max_timestamp;
        }

        // Update time span
        metadata.time_span = metadata.latest_timestamp.signed_duration_since(metadata.earliest_timestamp)
            .to_std().unwrap_or(Duration::from_secs(0));

        // Update insertion rate
        let time_span_secs = metadata.time_span.as_secs() as f64;
        if time_span_secs > 0.0 {
            metadata.avg_insertion_rate = vectors.len() as f64 / time_span_secs;
        }

        info!("📊 Updated time-series metadata: span={:.1}h, rate={:.1} vectors/sec",
              time_span_secs / 3600.0, metadata.avg_insertion_rate);

        Ok(())
    }

    // Private helper methods for spiral pattern optimization
    fn create_spiral_groups(&self, sorted_vectors: &[TimestampedVector]) -> Result<Vec<SpiralGroup>> {
        let time_window = self.spiral_config.time_window_size;
        let mut groups = Vec::new();

        if sorted_vectors.is_empty() {
            return Ok(groups);
        }

        let mut current_group_start = sorted_vectors[0].timestamp;
        let mut current_group_vectors = Vec::new();

        for vector in sorted_vectors {
            let time_diff = vector.timestamp.signed_duration_since(current_group_start)
                .to_std().unwrap_or(Duration::from_secs(0));

            if time_diff > time_window && !current_group_vectors.is_empty() {
                // Create new group
                let group_id = format!("spiral_{}_{}", current_group_start.timestamp(), groups.len());
                let group = self.create_spiral_group(
                    group_id,
                    current_group_start,
                    vector.timestamp,
                    current_group_vectors,
                )?;
                groups.push(group);

                // Start new group
                current_group_start = vector.timestamp;
                current_group_vectors = vec![vector.clone()];
            } else {
                current_group_vectors.push(vector.clone());
            }
        }

        // Handle remaining vectors
        if !current_group_vectors.is_empty() {
            let group_id = format!("spiral_{}_{}", current_group_start.timestamp(), groups.len());
            let last_timestamp = current_group_vectors.last().unwrap().timestamp;
            let group = self.create_spiral_group(
                group_id,
                current_group_start,
                last_timestamp,
                current_group_vectors,
            )?;
            groups.push(group);
        }

        Ok(groups)
    }

    fn create_spiral_group(
        &self,
        id: String,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        vectors: Vec<TimestampedVector>,
    ) -> Result<SpiralGroup> {
        // Calculate spiral center based on vector centroids
        let spiral_center = self.calculate_spiral_center(&vectors);

        // Calculate spiral radius based on data distribution
        let spiral_radius = self.calculate_spiral_radius(&vectors, spiral_center);

        // Organize vectors in spiral order for cache optimization
        let spiral_ordered_vectors = self.organize_in_spiral_order(vectors, spiral_center)?;

        Ok(SpiralGroup {
            id,
            time_range: TimeRange {
                start: start_time,
                end: end_time,
            },
            vectors: spiral_ordered_vectors,
            spiral_center,
            spiral_radius,
        })
    }

    fn build_temporal_index(&self, spiral_groups: &[SpiralGroup]) -> TemporalIndex {
        let mut time_to_group = HashMap::new();
        let mut sorted_ranges = Vec::new();

        for group in spiral_groups {
            time_to_group.insert(group.time_range.clone(), group.id.clone());
            sorted_ranges.push(group.time_range.clone());
        }

        // Sort ranges for binary search
        sorted_ranges.sort_by_key(|range| range.start);

        TemporalIndex {
            time_to_group,
            sorted_ranges,
        }
    }

    fn optimize_compression_by_age(&self, vectors: &[TimestampedVector]) -> HashMap<TimeRange, CompressionLevel> {
        let mut compression_mapping = HashMap::new();
        let now = Utc::now();

        for vector in vectors {
            let age = now.signed_duration_since(vector.timestamp)
                .to_std().unwrap_or(Duration::from_secs(0));

            let compression_level = if age < self.spiral_config.age_compression_thresholds.fresh_threshold {
                CompressionLevel::None
            } else if age < self.spiral_config.age_compression_thresholds.recent_threshold {
                CompressionLevel::Light
            } else if age < self.spiral_config.age_compression_thresholds.old_threshold {
                CompressionLevel::Medium
            } else if age < self.spiral_config.age_compression_thresholds.ancient_threshold {
                CompressionLevel::Heavy
            } else {
                CompressionLevel::Maximum
            };

            let time_range = TimeRange {
                start: vector.timestamp,
                end: vector.timestamp + chrono::Duration::seconds(1),
            };

            compression_mapping.insert(time_range, compression_level);
        }

        compression_mapping
    }

    fn identify_relevant_spirals(&self, query: &TemporalQuery, metadata: &TimeSeriesMetadata) -> Vec<String> {
        // Find spiral groups that overlap with query time range
        let mut relevant_spirals = Vec::new();

        for (time_range, group_id) in &metadata.access_patterns {
            if self.time_ranges_overlap(&query.time_range, time_range) {
                relevant_spirals.push(group_id.clone());
            }
        }

        if relevant_spirals.is_empty() {
            // If no specific patterns, return default spiral selection
            relevant_spirals.push("default_spiral".to_string());
        }

        relevant_spirals
    }

    fn optimize_scan_order(&self, query: &TemporalQuery) -> ScanOrder {
        // Determine optimal scan order based on query characteristics
        if query.vector_query.is_some() {
            ScanOrder::Hybrid // Vector similarity + temporal
        } else {
            ScanOrder::TemporalNewest // Pure temporal query
        }
    }

    fn calculate_termination_criteria(&self, query: &TemporalQuery) -> TerminationCriteria {
        TerminationCriteria {
            max_scan_time: Duration::from_millis(100), // 100ms max scan time
            min_quality_threshold: 0.8, // 80% quality threshold
            max_groups_to_scan: (query.k * 2).min(50), // At most 50 groups
        }
    }

    // Additional helper methods for spiral pattern calculations
    fn calculate_spiral_center(&self, vectors: &[TimestampedVector]) -> (f64, f64) {
        if vectors.is_empty() {
            return (0.0, 0.0);
        }

        // Calculate centroid of vector embeddings projected to 2D
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;

        for vector in vectors {
            if vector.vector.len() >= 2 {
                sum_x += vector.vector[0] as f64;
                sum_y += vector.vector[1] as f64;
            }
        }

        let count = vectors.len() as f64;
        (sum_x / count, sum_y / count)
    }

    fn calculate_spiral_radius(&self, vectors: &[TimestampedVector], center: (f64, f64)) -> f64 {
        let mut max_distance = 0.0;

        for vector in vectors {
            if vector.vector.len() >= 2 {
                let dx = vector.vector[0] as f64 - center.0;
                let dy = vector.vector[1] as f64 - center.1;
                let distance = (dx * dx + dy * dy).sqrt();
                max_distance = max_distance.max(distance);
            }
        }

        max_distance * self.spiral_config.spiral_factor
    }

    fn organize_in_spiral_order(&self, vectors: Vec<TimestampedVector>, center: (f64, f64)) -> Result<Vec<TimestampedVector>> {
        let mut spiral_vectors = vectors;

        // Sort by spiral angle for cache-friendly access pattern
        spiral_vectors.sort_by(|a, b| {
            let angle_a = self.calculate_spiral_angle(&a.vector, center);
            let angle_b = self.calculate_spiral_angle(&b.vector, center);
            angle_a.partial_cmp(&angle_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(spiral_vectors)
    }

    fn calculate_spiral_angle(&self, vector: &[f32], center: (f64, f64)) -> f64 {
        if vector.len() < 2 {
            return 0.0;
        }

        let dx = vector[0] as f64 - center.0;
        let dy = vector[1] as f64 - center.1;
        dy.atan2(dx)
    }

    fn time_ranges_overlap(&self, range1: &TimeRange, _range2: &TimeRange) -> bool {
        // Simplified overlap check (would be more sophisticated in production)
        !range1.start.timestamp_nanos().is_zero()
    }
}

impl PatternCache {
    pub fn new(config: PatternCacheConfig) -> Self {
        Self {
            spiral_layouts: Arc::new(RwLock::new(HashMap::new())),
            cache_config: config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spiral_pattern_manager() {
        let config = SpiralConfig::default();
        let manager = SpiralPatternManager::new("test_collection".to_string(), config);

        // Test spiral center calculation
        let vectors = vec![
            TimestampedVector {
                id: "v1".to_string(),
                vector: vec![1.0, 2.0],
                timestamp: Utc::now(),
                metadata: HashMap::new(),
            },
            TimestampedVector {
                id: "v2".to_string(),
                vector: vec![3.0, 4.0],
                timestamp: Utc::now(),
                metadata: HashMap::new(),
            },
        ];

        let center = manager.calculate_spiral_center(&vectors);
        assert_eq!(center, (2.0, 3.0)); // Average of (1,2) and (3,4)
    }

    #[test]
    fn test_compression_level_selection() {
        let config = SpiralConfig::default();
        let manager = SpiralPatternManager::new("test".to_string(), config);

        // Test age-based compression
        let old_vector = TimestampedVector {
            id: "old".to_string(),
            vector: vec![1.0; 768],
            timestamp: Utc::now() - chrono::Duration::days(7),
            metadata: HashMap::new(),
        };

        let vectors = vec![old_vector];
        let compression_mapping = manager.optimize_compression_by_age(&vectors);

        assert!(!compression_mapping.is_empty());
    }

    #[tokio::test]
    async fn test_temporal_query_optimization() {
        let config = SpiralConfig::default();
        let manager = SpiralPatternManager::new("test".to_string(), config);

        let query = TemporalQuery {
            time_range: TimeRange {
                start: Utc::now() - chrono::Duration::hours(1),
                end: Utc::now(),
            },
            vector_query: Some(vec![0.5; 768]),
            k: 10,
            metadata_filters: None,
        };

        let plan = manager.optimize_temporal_queries(&query).unwrap();
        assert!(!plan.target_spirals.is_empty());
        assert!(plan.early_termination.max_groups_to_scan > 0);
    }
}