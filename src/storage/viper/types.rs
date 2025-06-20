// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Storage Types and Data Structures
//!
//! ⚠️  OBSOLETE: This file is being replaced by the unified vector storage system.
//! 🔄 MIGRATION: Use src/storage/vector/types.rs instead.
//! 📅 DEPRECATION: Phase 1.1 - Core Unification (unified type system)
//!
//! This implementation contains fragmented types that are consolidated in the
//! unified vector storage system. The new system provides:
//! - Single source of truth for all vector types
//! - Consistent interfaces across all storage engines
//! - Reduced type proliferation and duplication
//!
//! Core data types for the VIPER storage layout including vector representations,
//! cluster metadata, and storage format definitions.

use crate::core::{CollectionId, VectorId, VectorRecord};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Cluster ID type for organizing vectors by similarity
pub type ClusterId = u32;

/// Partition ID for physical storage organization
pub type PartitionId = String;

/// Dense vector record for Parquet row storage (ID and metadata columns first)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenseVectorRecord {
    /// Vector ID (first column for efficient lookups)
    pub id: VectorId,

    /// Metadata (second column for efficient filtering)
    pub metadata: serde_json::Value,

    /// Cluster assignment
    pub cluster_id: ClusterId,

    /// Dense vector data
    pub vector: Vec<f32>,

    /// Timestamp
    pub timestamp: DateTime<Utc>,

    /// Expiration timestamp for TTL and MVCC support
    /// None = never expires, Some(timestamp) = expires at timestamp
    pub expires_at: Option<DateTime<Utc>>,
}

/// Sparse vector metadata record for separate Parquet table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseVectorMetadata {
    /// Vector ID (first column for efficient lookups)
    pub id: VectorId,

    /// Metadata (second column for efficient filtering)
    pub metadata: serde_json::Value,

    /// Cluster assignment
    pub cluster_id: ClusterId,

    /// Vector dimension count
    pub dimension_count: u32,

    /// Sparsity ratio
    pub sparsity_ratio: f32,

    /// Timestamp
    pub timestamp: DateTime<Utc>,

    /// Expiration timestamp for TTL and MVCC support
    /// None = never expires, Some(timestamp) = expires at timestamp
    pub expires_at: Option<DateTime<Utc>>,
}

/// Sparse vector key-value pair for KV storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseVectorData {
    /// Vector ID (key for KV storage)
    pub id: VectorId,

    /// Sparse dimensions as (index, value) pairs
    pub dimensions: Vec<(u32, f32)>,
}

/// VIPER vector entry that can be stored in either sparse or dense format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViperVector {
    /// Sparse vector with separate metadata and KV storage
    Sparse {
        metadata: SparseVectorMetadata,
        data: SparseVectorData,
    },

    /// Dense vector stored as a complete row
    Dense { record: DenseVectorRecord },
}

/// Cluster metadata containing information about vector groupings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMetadata {
    /// Unique cluster identifier
    pub cluster_id: ClusterId,

    /// Collection this cluster belongs to
    pub collection_id: CollectionId,

    /// Centroid vector for the cluster
    pub centroid: Vec<f32>,

    /// Number of vectors in this cluster
    pub vector_count: usize,

    /// Cluster quality metrics
    pub quality_metrics: ClusterQualityMetrics,

    /// Partition assignment for this cluster
    pub partition_id: PartitionId,

    /// Last update timestamp
    pub last_updated: DateTime<Utc>,

    /// Cluster statistics for search optimization
    pub statistics: ClusterStatistics,
}

/// Quality metrics for cluster evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterQualityMetrics {
    /// Intra-cluster distance (lower is better)
    pub intra_cluster_distance: f32,

    /// Inter-cluster distance (higher is better)
    pub inter_cluster_distance: f32,

    /// Silhouette score (-1 to 1, higher is better)
    pub silhouette_score: f32,

    /// Cluster density (vectors per unit volume)
    pub density: f32,

    /// Cluster stability over time
    pub stability_score: f32,
}

/// Statistical information about a cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatistics {
    /// Minimum value per dimension
    pub min_values: Vec<f32>,

    /// Maximum value per dimension
    pub max_values: Vec<f32>,

    /// Mean value per dimension
    pub mean_values: Vec<f32>,

    /// Standard deviation per dimension
    pub std_dev_values: Vec<f32>,

    /// Sparsity ratio (percentage of zero values)
    pub sparsity_ratio: f32,

    /// Most frequent metadata values
    pub frequent_metadata: HashMap<String, serde_json::Value>,
}

/// Partition information for physical storage organization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionMetadata {
    /// Unique partition identifier
    pub partition_id: PartitionId,

    /// Collection this partition belongs to
    pub collection_id: CollectionId,

    /// Clusters contained in this partition
    pub cluster_ids: Vec<ClusterId>,

    /// Partition statistics
    pub statistics: PartitionStatistics,

    /// Storage filesystem URL (s3://, file://, adls://, gcs://)
    pub storage_url: String,

    /// Parquet file information
    pub parquet_files: Vec<ParquetFileInfo>,

    /// Bloom filter data for fast cluster lookup
    pub bloom_filter: Option<Vec<u8>>,

    /// Creation and modification timestamps
    pub created_at: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,
}

/// Statistics for a storage partition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionStatistics {
    /// Total number of vectors in partition
    pub total_vectors: usize,

    /// Total storage size in bytes
    pub storage_size_bytes: u64,

    /// Compressed storage size
    pub compressed_size_bytes: u64,

    /// Compression ratio achieved
    pub compression_ratio: f32,

    /// Average access frequency
    pub access_frequency: f32,

    /// Last access timestamp
    pub last_accessed: DateTime<Utc>,
}

/// Information about Parquet files in a partition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParquetFileInfo {
    /// File path relative to partition directory
    pub file_path: String,

    /// Storage URL for filesystem access (s3://, file://, etc.)
    pub storage_url: String,

    /// File size in bytes
    pub file_size_bytes: u64,

    /// Number of row groups in the file
    pub row_group_count: usize,

    /// Row count in the file
    pub row_count: usize,

    /// Cluster IDs covered by this file
    pub cluster_ids: Vec<ClusterId>,

    /// Schema version for compatibility
    pub schema_version: u32,

    /// Checksum for integrity verification
    pub checksum: String,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

/// Search result from VIPER storage
#[derive(Debug, Clone)]
pub struct ViperSearchResult {
    /// Vector ID
    pub vector_id: VectorId,

    /// Similarity score
    pub score: f32,

    /// Vector data (loaded on demand)
    pub vector: Option<Vec<f32>>,

    /// Metadata
    pub metadata: serde_json::Value,

    /// Cluster ID this vector belongs to
    pub cluster_id: ClusterId,

    /// Partition where this vector is stored
    pub partition_id: PartitionId,

    /// Storage filesystem URL where vector was found (s3://, file://, adls://, gcs://)
    pub storage_url: String,
}

/// Query context for VIPER search operations
#[derive(Debug, Clone)]
pub struct ViperSearchContext {
    /// Target collection
    pub collection_id: CollectionId,

    /// Query vector
    pub query_vector: Vec<f32>,

    /// Number of results requested
    pub k: usize,

    /// Similarity threshold
    pub threshold: Option<f32>,

    /// Metadata filters
    pub filters: Option<HashMap<String, serde_json::Value>>,

    /// Cluster hints for pruning (predicted by ML model)
    pub cluster_hints: Option<Vec<ClusterId>>,

    /// Search strategy preferences
    pub search_strategy: SearchStrategy,

    /// Maximum storage locations to search (for performance control)
    pub max_storage_locations: Option<usize>,
}

/// Search strategy for VIPER operations
#[derive(Debug, Clone)]
pub enum SearchStrategy {
    /// Search all relevant clusters exhaustively
    Exhaustive,

    /// Use ML-predicted cluster pruning for speed
    ClusterPruned {
        max_clusters: usize,
        confidence_threshold: f32,
    },

    /// Progressive search across tiers (stop early if enough results)
    Progressive { tier_result_thresholds: Vec<usize> },

    /// Adaptive strategy based on query characteristics
    Adaptive {
        query_complexity_score: f32,
        time_budget_ms: u64,
    },
}

/// ML model prediction for cluster assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterPrediction {
    /// Predicted cluster ID
    pub cluster_id: ClusterId,

    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,

    /// Alternative cluster suggestions
    pub alternatives: Vec<(ClusterId, f32)>,

    /// Prediction metadata
    pub prediction_metadata: PredictionMetadata,
}

/// Metadata about ML model predictions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionMetadata {
    /// Model version used for prediction
    pub model_version: String,

    /// Features used in prediction
    pub features_used: Vec<String>,

    /// Prediction timestamp
    pub predicted_at: DateTime<Utc>,

    /// Model accuracy metrics at prediction time
    pub model_accuracy: f32,

    /// Computational cost of prediction
    pub prediction_cost_ms: u64,
}

/// Vector storage format decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorStorageFormat {
    /// Store as sparse key-value pairs
    Sparse,

    /// Store as dense row
    Dense,

    /// Automatically decide based on sparsity
    Auto,
}

/// Batch operation for efficient VIPER operations
#[derive(Debug, Clone)]
pub struct ViperBatchOperation {
    /// Operation type
    pub operation_type: BatchOperationType,

    /// Vectors to process
    pub vectors: Vec<VectorRecord>,

    /// Cluster predictions for the batch
    pub cluster_predictions: Option<Vec<ClusterPrediction>>,

    /// Batch metadata
    pub batch_metadata: BatchMetadata,
}

/// Types of batch operations
#[derive(Debug, Clone)]
pub enum BatchOperationType {
    /// Insert new vectors
    Insert,

    /// Update existing vectors
    Update,

    /// Delete vectors
    Delete,

    /// Recluster existing vectors
    Recluster,
}

/// Metadata for batch operations
#[derive(Debug, Clone)]
pub struct BatchMetadata {
    /// Batch size
    pub batch_size: usize,

    /// Expected processing time
    pub estimated_duration_ms: u64,

    /// Priority level
    pub priority: BatchPriority,

    /// Source of the batch
    pub source: String,

    /// Batch creation timestamp
    pub created_at: DateTime<Utc>,
}

/// Priority levels for batch processing
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BatchPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl ViperVector {
    /// Get the vector ID regardless of storage format
    pub fn id(&self) -> &VectorId {
        match self {
            ViperVector::Sparse { metadata, .. } => &metadata.id,
            ViperVector::Dense { record, .. } => &record.id,
        }
    }

    /// Get the cluster ID
    pub fn cluster_id(&self) -> ClusterId {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.cluster_id,
            ViperVector::Dense { record, .. } => record.cluster_id,
        }
    }

    /// Check if vector is currently active (not expired)
    pub fn is_active(&self) -> bool {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.is_active(),
            ViperVector::Dense { record, .. } => record.is_active(),
        }
    }

    /// Check if vector is a tombstone
    pub fn is_tombstone(&self) -> bool {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.is_tombstone(),
            ViperVector::Dense { record, .. } => record.is_tombstone(),
        }
    }

    /// Get expires_at timestamp
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.expires_at,
            ViperVector::Dense { record, .. } => record.expires_at,
        }
    }

    /// Set TTL on vector
    pub fn set_ttl(&mut self, ttl_seconds: i64) {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.set_ttl(ttl_seconds),
            ViperVector::Dense { record, .. } => record.set_ttl(ttl_seconds),
        }
    }

    /// Remove TTL from vector
    pub fn remove_ttl(&mut self) {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.remove_ttl(),
            ViperVector::Dense { record, .. } => record.remove_ttl(),
        }
    }

    /// Get metadata
    pub fn metadata(&self) -> &serde_json::Value {
        match self {
            ViperVector::Sparse { metadata, .. } => &metadata.metadata,
            ViperVector::Dense { record, .. } => &record.metadata,
        }
    }

    /// Get timestamp
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.timestamp,
            ViperVector::Dense { record, .. } => record.timestamp,
        }
    }

    /// Convert to dense vector representation (materializes sparse vectors)
    pub fn to_dense_vector(&self, dimension: usize) -> Vec<f32> {
        match self {
            ViperVector::Dense { record, .. } => record.vector.clone(),
            ViperVector::Sparse { data, .. } => {
                let mut dense = vec![0.0; dimension];
                for &(dim_idx, value) in &data.dimensions {
                    if (dim_idx as usize) < dimension {
                        dense[dim_idx as usize] = value;
                    }
                }
                dense
            }
        }
    }

    /// Calculate sparsity ratio (percentage of zero values)
    pub fn sparsity_ratio(&self, _dimension: usize) -> f32 {
        match self {
            ViperVector::Sparse { metadata, .. } => metadata.sparsity_ratio,
            ViperVector::Dense { record, .. } => {
                let zero_count = record.vector.iter().filter(|&&x| x == 0.0).count();
                zero_count as f32 / record.vector.len() as f32
            }
        }
    }

    /// Determine optimal storage format based on sparsity
    pub fn recommended_storage_format(
        &self,
        dimension: usize,
        sparsity_threshold: f32,
    ) -> VectorStorageFormat {
        let sparsity = self.sparsity_ratio(dimension);
        if sparsity > sparsity_threshold {
            VectorStorageFormat::Sparse
        } else {
            VectorStorageFormat::Dense
        }
    }
}

impl DenseVectorRecord {
    /// Create a new dense vector record
    pub fn new(
        id: VectorId,
        vector: Vec<f32>,
        metadata: serde_json::Value,
        cluster_id: ClusterId,
    ) -> Self {
        Self {
            id,
            metadata,
            cluster_id,
            vector,
            timestamp: Utc::now(),
            expires_at: None, // Default: never expires
        }
    }

    /// Create a record with TTL (expires after duration)
    pub fn with_ttl(
        id: VectorId,
        vector: Vec<f32>,
        metadata: serde_json::Value,
        cluster_id: ClusterId,
        ttl_seconds: i64,
    ) -> Self {
        Self {
            id,
            metadata,
            cluster_id,
            vector,
            timestamp: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::seconds(ttl_seconds)),
        }
    }

    /// Create a tombstone record for soft delete
    pub fn create_tombstone(id: VectorId, cluster_id: ClusterId) -> Self {
        Self {
            id,
            metadata: serde_json::Value::Null,
            cluster_id,
            vector: Vec::new(), // Empty vector for tombstone
            timestamp: Utc::now(),
            expires_at: Some(Utc::now()), // Immediately expired = soft deleted
        }
    }

    /// Check if record is currently active (not expired)
    pub fn is_active(&self) -> bool {
        match self.expires_at {
            None => true, // Never expires
            Some(expires_at) => Utc::now() < expires_at,
        }
    }

    /// Check if record is a tombstone
    pub fn is_tombstone(&self) -> bool {
        self.vector.is_empty() && self.expires_at.is_some()
    }

    /// Set TTL on existing record
    pub fn set_ttl(&mut self, ttl_seconds: i64) {
        self.expires_at = Some(Utc::now() + chrono::Duration::seconds(ttl_seconds));
    }

    /// Remove TTL (make record permanent)
    pub fn remove_ttl(&mut self) {
        self.expires_at = None;
    }

    /// Calculate sparsity ratio
    pub fn sparsity_ratio(&self) -> f32 {
        let zero_count = self.vector.iter().filter(|&&x| x == 0.0).count();
        zero_count as f32 / self.vector.len() as f32
    }
}

impl SparseVectorMetadata {
    /// Create new sparse vector metadata
    pub fn new(
        id: VectorId,
        metadata: serde_json::Value,
        cluster_id: ClusterId,
        dimension_count: u32,
        sparsity_ratio: f32,
    ) -> Self {
        Self {
            id,
            metadata,
            cluster_id,
            dimension_count,
            sparsity_ratio,
            timestamp: Utc::now(),
            expires_at: None, // Default: never expires
        }
    }

    /// Create metadata with TTL
    pub fn with_ttl(
        id: VectorId,
        metadata: serde_json::Value,
        cluster_id: ClusterId,
        dimension_count: u32,
        sparsity_ratio: f32,
        ttl_seconds: i64,
    ) -> Self {
        Self {
            id,
            metadata,
            cluster_id,
            dimension_count,
            sparsity_ratio,
            timestamp: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::seconds(ttl_seconds)),
        }
    }

    /// Create tombstone metadata for soft delete
    pub fn create_tombstone(id: VectorId, cluster_id: ClusterId) -> Self {
        Self {
            id,
            metadata: serde_json::Value::Null,
            cluster_id,
            dimension_count: 0,
            sparsity_ratio: 1.0, // Fully sparse (deleted)
            timestamp: Utc::now(),
            expires_at: Some(Utc::now()), // Immediately expired = soft deleted
        }
    }

    /// Check if metadata is currently active (not expired)
    pub fn is_active(&self) -> bool {
        match self.expires_at {
            None => true, // Never expires
            Some(expires_at) => Utc::now() < expires_at,
        }
    }

    /// Check if metadata represents a tombstone
    pub fn is_tombstone(&self) -> bool {
        self.dimension_count == 0 && self.expires_at.is_some()
    }

    /// Set TTL on existing metadata
    pub fn set_ttl(&mut self, ttl_seconds: i64) {
        self.expires_at = Some(Utc::now() + chrono::Duration::seconds(ttl_seconds));
    }

    /// Remove TTL (make metadata permanent)
    pub fn remove_ttl(&mut self) {
        self.expires_at = None;
    }
}

impl SparseVectorData {
    /// Create new sparse vector data
    pub fn new(id: VectorId, dimensions: Vec<(u32, f32)>) -> Self {
        Self { id, dimensions }
    }

    /// Create from HashMap
    pub fn from_hashmap(id: VectorId, dimensions: HashMap<u32, f32>) -> Self {
        let mut pairs: Vec<(u32, f32)> = dimensions.into_iter().collect();
        pairs.sort_by_key(|&(dim, _)| dim);
        Self::new(id, pairs)
    }

    /// Convert to HashMap
    pub fn to_hashmap(&self) -> HashMap<u32, f32> {
        self.dimensions.iter().cloned().collect()
    }

    /// Get dimension count
    pub fn dimension_count(&self) -> usize {
        self.dimensions.len()
    }
}

impl ClusterMetadata {
    /// Calculate cluster quality score (0.0 to 1.0, higher is better)
    pub fn quality_score(&self) -> f32 {
        let silhouette_normalized = (self.quality_metrics.silhouette_score + 1.0) / 2.0;
        let density_score = self.quality_metrics.density.min(1.0);
        let stability_score = self.quality_metrics.stability_score;

        // Weighted average of quality metrics
        (silhouette_normalized * 0.4 + density_score * 0.3 + stability_score * 0.3)
            .max(0.0)
            .min(1.0)
    }

    /// Check if cluster needs reclustering based on quality threshold
    pub fn needs_reclustering(&self, quality_threshold: f32) -> bool {
        self.quality_score() < quality_threshold
    }
}

impl PartitionMetadata {
    /// Calculate partition efficiency score
    pub fn efficiency_score(&self) -> f32 {
        let compression_score = self.statistics.compression_ratio.min(10.0) / 10.0;
        let access_score = self.statistics.access_frequency.min(1.0);
        let size_score =
            1.0 - (self.statistics.storage_size_bytes as f32 / (1024.0 * 1024.0 * 1024.0)).min(1.0); // Prefer smaller partitions

        (compression_score * 0.4 + access_score * 0.4 + size_score * 0.2)
            .max(0.0)
            .min(1.0)
    }

    /// Check if partition should be migrated to a different tier
    pub fn should_migrate(&self, current_time: DateTime<Utc>) -> bool {
        let days_since_access = (current_time - self.statistics.last_accessed).num_days();
        let low_access_frequency = self.statistics.access_frequency < 0.1;

        days_since_access > 30 && low_access_frequency
    }
}

impl SearchStrategy {
    /// Get the estimated computational cost for this strategy (relative scale)
    pub fn computational_cost(&self) -> f32 {
        match self {
            SearchStrategy::Exhaustive => 1.0,
            SearchStrategy::ClusterPruned { max_clusters, .. } => {
                (*max_clusters as f32 / 100.0).min(1.0)
            }
            SearchStrategy::Progressive { .. } => 0.6,
            SearchStrategy::Adaptive {
                query_complexity_score,
                ..
            } => *query_complexity_score,
        }
    }
}

// Re-export SIMD capabilities from hardware detection
pub use crate::compute::hardware_detection::SimdLevel as SimdInstructionSet;
