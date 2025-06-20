// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Vector Storage Types - Single Source of Truth
//!
//! This module consolidates all vector storage types from across the system,
//! eliminating duplication and providing a clean, unified interface for all
//! vector operations. This replaces the fragmented types from:
//! - storage/viper/types.rs (ViperSearchResult, ViperSearchContext)
//! - storage/search/mod.rs (SearchResult, SearchOperation)
//! - compute/indexing.rs (fragmented index types)

use crate::core::{CollectionId, VectorId, VectorRecord};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

/// Unified search context for all vector operations
/// Replaces: ViperSearchContext, SearchOperation variants
#[derive(Debug, Clone, PartialEq)]
pub struct SearchContext {
    /// Target collection
    pub collection_id: CollectionId,
    
    /// Query vector for similarity search
    pub query_vector: Vec<f32>,
    
    /// Number of results requested
    pub k: usize,
    
    /// Optional metadata filters
    pub filters: Option<MetadataFilter>,
    
    /// Search strategy to use
    pub strategy: SearchStrategy,
    
    /// Algorithm-specific hints and parameters
    pub algorithm_hints: HashMap<String, Value>,
    
    /// Optional similarity threshold
    pub threshold: Option<f32>,
    
    /// Maximum execution time budget
    pub timeout_ms: Option<u64>,
    
    /// Debug information requests
    pub include_debug_info: bool,
    
    /// Vector data inclusion preference
    pub include_vectors: bool,
}

/// Unified search result type
/// Replaces: ViperSearchResult, SearchResult, IndexSearchResult
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Vector identifier
    pub vector_id: VectorId,
    
    /// Similarity/distance score
    pub score: f32,
    
    /// Optional vector data (loaded on demand)
    pub vector: Option<Vec<f32>>,
    
    /// Metadata associated with vector
    pub metadata: Value,
    
    /// Optional debug information
    pub debug_info: Option<SearchDebugInfo>,
    
    /// Storage location information
    pub storage_info: Option<StorageLocationInfo>,
    
    /// Ranking information
    pub rank: Option<usize>,
    
    /// Distance value (if different from score)
    pub distance: Option<f32>,
}

/// Search strategy unified across all engines
/// Consolidates: SearchStrategy from VIPER, HybridSearchStrategy from search/mod.rs
#[derive(Debug, Clone, PartialEq)]
pub enum SearchStrategy {
    /// Exact brute-force search - highest accuracy, highest cost
    Exact,
    
    /// Approximate Nearest Neighbor with recall target
    Approximate { 
        target_recall: f32,
        max_candidates: Option<usize>,
    },
    
    /// Hybrid strategy: exact below threshold, ANN above
    Hybrid { 
        exact_threshold: f32, 
        ann_backup: bool,
        fallback_strategy: Box<SearchStrategy>,
    },
    
    /// Progressive search across storage tiers
    Progressive { 
        tier_limits: Vec<usize>,
        early_termination_threshold: Option<f32>,
    },
    
    /// Adaptive strategy based on query characteristics
    Adaptive {
        query_complexity_score: f32,
        time_budget_ms: u64,
        accuracy_preference: f32, // 0.0 = speed, 1.0 = accuracy
    },
}

/// Unified metadata filter system
/// Consolidates: FilterCondition from search/mod.rs, VIPER filters
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataFilter {
    /// Single field condition
    Field {
        field: String,
        condition: FieldCondition,
    },
    
    /// Logical AND of multiple filters
    And(Vec<MetadataFilter>),
    
    /// Logical OR of multiple filters
    Or(Vec<MetadataFilter>),
    
    /// Logical NOT of a filter
    Not(Box<MetadataFilter>),
}

/// Field-level conditions
#[derive(Debug, Clone, PartialEq)]
pub enum FieldCondition {
    Equals(Value),
    NotEquals(Value),
    GreaterThan(Value),
    GreaterThanOrEqual(Value),
    LessThan(Value),
    LessThanOrEqual(Value),
    In(Vec<Value>),
    NotIn(Vec<Value>),
    Contains(String),
    StartsWith(String),
    EndsWith(String),
    Regex(String),
    IsNull,
    IsNotNull,
    Range { min: Value, max: Value, inclusive: bool },
}

/// Debug information for search operations
#[derive(Debug, Clone)]
pub struct SearchDebugInfo {
    /// Execution time breakdown
    pub timing: SearchTiming,
    
    /// Algorithm used
    pub algorithm_used: String,
    
    /// Index information
    pub index_usage: Vec<IndexUsageInfo>,
    
    /// Search path taken
    pub search_path: Vec<SearchStep>,
    
    /// Performance metrics
    pub metrics: SearchMetrics,
    
    /// Cost breakdown
    pub cost_breakdown: CostBreakdown,
}

/// Search timing information
#[derive(Debug, Clone)]
pub struct SearchTiming {
    pub total_ms: f64,
    pub planning_ms: f64,
    pub execution_ms: f64,
    pub post_processing_ms: f64,
    pub io_wait_ms: f64,
    pub cpu_time_ms: f64,
}

/// Index usage information
#[derive(Debug, Clone)]
pub struct IndexUsageInfo {
    pub index_name: String,
    pub index_type: IndexType,
    pub selectivity: f64,
    pub entries_scanned: usize,
    pub cache_hit_ratio: f64,
}

/// Search execution step
#[derive(Debug, Clone)]
pub struct SearchStep {
    pub step_name: String,
    pub step_type: SearchStepType,
    pub duration_ms: f64,
    pub items_processed: usize,
    pub items_filtered: usize,
}

/// Types of search steps
#[derive(Debug, Clone)]
pub enum SearchStepType {
    Planning,
    IndexLookup,
    VectorComparison,
    MetadataFilter,
    ResultMerging,
    PostProcessing,
}

/// Performance metrics
#[derive(Debug, Clone)]
pub struct SearchMetrics {
    pub vectors_compared: usize,
    pub distance_calculations: usize,
    pub index_traversals: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub disk_reads: usize,
    pub network_requests: usize,
    pub memory_peak_mb: f64,
}

/// Cost breakdown for search operations
#[derive(Debug, Clone)]
pub struct CostBreakdown {
    pub cpu_cost: f64,
    pub io_cost: f64,
    pub memory_cost: f64,
    pub network_cost: f64,
    pub total_cost: f64,
    pub cost_efficiency: f64, // results per unit cost
}

/// Storage location information
#[derive(Debug, Clone)]
pub struct StorageLocationInfo {
    pub storage_url: String,
    pub tier: StorageTier,
    pub partition_id: Option<String>,
    pub cluster_id: Option<u32>,
    pub file_path: Option<String>,
    pub row_group: Option<usize>,
}

/// Storage tier enumeration
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StorageTier {
    UltraHot,  // MMAP + OS cache
    Hot,       // Local SSD
    Warm,      // Local HDD/Network storage
    Cold,      // Cloud storage (S3/Azure/GCS)
}

/// Unified index type enumeration
/// Consolidates: Various index types scattered across system
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexType {
    /// Hierarchical Navigable Small World
    HNSW {
        m: usize,              // max connections per layer
        ef_construction: usize, // size of dynamic candidate list
        ef_search: usize,      // size of search candidate list
    },
    
    /// Inverted File Index
    IVF {
        n_lists: usize,        // number of inverted lists
        n_probes: usize,       // number of lists to search
    },
    
    /// Product Quantization
    PQ {
        m: usize,              // number of subquantizers
        n_bits: usize,         // bits per subquantizer
    },
    
    /// Locality Sensitive Hashing
    LSH {
        n_tables: usize,       // number of hash tables
        n_bits: usize,         // bits per hash
    },
    
    /// Flat/Brute Force
    Flat,
    
    /// B-tree for metadata
    BTree,
    
    /// Hash index for exact lookups
    Hash,
    
    /// Custom algorithm
    Custom(String),
}

/// Vector operation types
#[derive(Debug, Clone, PartialEq)]
pub enum VectorOperation {
    /// Insert new vector
    Insert {
        record: VectorRecord,
        index_immediately: bool,
    },
    
    /// Update existing vector
    Update {
        vector_id: VectorId,
        new_vector: Option<Vec<f32>>,
        new_metadata: Option<Value>,
    },
    
    /// Delete vector (soft or hard)
    Delete {
        vector_id: VectorId,
        soft_delete: bool,
    },
    
    /// Batch operations
    Batch {
        operations: Vec<VectorOperation>,
        transactional: bool,
    },
    
    /// Search operation
    Search(SearchContext),
    
    /// Get vector by ID
    Get {
        vector_id: VectorId,
        include_vector: bool,
    },
}

/// Operation result
#[derive(Debug, Clone)]
pub enum OperationResult {
    /// Insert result
    Inserted { vector_id: VectorId },
    
    /// Update result  
    Updated { vector_id: VectorId, changes: usize },
    
    /// Delete result
    Deleted { vector_id: VectorId },
    
    /// Search results
    SearchResults(Vec<SearchResult>),
    
    /// Get result
    VectorData {
        vector_id: VectorId,
        vector: Option<Vec<f32>>,
        metadata: Value,
    },
    
    /// Batch results
    BatchResults(Vec<OperationResult>),
    
    /// Error result
    Error { 
        operation: String,
        error: String,
        recoverable: bool,
    },
}

/// Vector storage traits - unified interface for all storage engines

/// Main vector storage trait
#[async_trait::async_trait]
pub trait VectorStorage: Send + Sync {
    /// Storage engine name
    fn engine_name(&self) -> &'static str;
    
    /// Storage capabilities
    fn capabilities(&self) -> StorageCapabilities;
    
    /// Execute vector operation
    async fn execute_operation(
        &self,
        operation: VectorOperation,
    ) -> anyhow::Result<OperationResult>;
    
    /// Get storage statistics
    async fn get_statistics(&self) -> anyhow::Result<StorageStatistics>;
    
    /// Health check
    async fn health_check(&self) -> anyhow::Result<HealthStatus>;
}

/// Search algorithm trait
#[async_trait::async_trait]
pub trait SearchAlgorithm: Send + Sync {
    /// Algorithm name
    fn algorithm_name(&self) -> &'static str;
    
    /// Algorithm type
    fn algorithm_type(&self) -> IndexType;
    
    /// Execute search
    async fn search(
        &self,
        context: &SearchContext,
    ) -> anyhow::Result<Vec<SearchResult>>;
    
    /// Estimate search cost
    async fn estimate_cost(
        &self,
        context: &SearchContext,
    ) -> anyhow::Result<CostBreakdown>;
    
    /// Algorithm configuration
    fn configuration(&self) -> HashMap<String, Value>;
}

/// Vector index trait
#[async_trait::async_trait]
pub trait VectorIndex: Send + Sync {
    /// Index name
    fn index_name(&self) -> &str;
    
    /// Index type
    fn index_type(&self) -> IndexType;
    
    /// Add vector to index
    async fn add_vector(
        &mut self,
        vector_id: VectorId,
        vector: &[f32],
        metadata: &Value,
    ) -> anyhow::Result<()>;
    
    /// Remove vector from index
    async fn remove_vector(&mut self, vector_id: VectorId) -> anyhow::Result<()>;
    
    /// Search in index
    async fn search(
        &self,
        query: &[f32],
        k: usize,
        filters: Option<&MetadataFilter>,
    ) -> anyhow::Result<Vec<SearchResult>>;
    
    /// Get index statistics
    async fn statistics(&self) -> anyhow::Result<IndexStatistics>;
    
    /// Optimize index
    async fn optimize(&mut self) -> anyhow::Result<()>;
}

/// Storage capabilities
#[derive(Debug, Clone)]
pub struct StorageCapabilities {
    pub supports_transactions: bool,
    pub supports_streaming: bool,
    pub supports_compression: bool,
    pub supports_encryption: bool,
    pub max_vector_dimension: usize,
    pub max_batch_size: usize,
    pub supported_distances: Vec<DistanceMetric>,
    pub supported_indexes: Vec<IndexType>,
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStatistics {
    pub total_vectors: usize,
    pub total_collections: usize,
    pub storage_size_bytes: u64,
    pub index_size_bytes: u64,
    pub cache_hit_ratio: f64,
    pub avg_search_latency_ms: f64,
    pub operations_per_second: f64,
    pub last_compaction: Option<DateTime<Utc>>,
}

/// Index statistics
#[derive(Debug, Clone)]
pub struct IndexStatistics {
    pub vector_count: usize,
    pub index_size_bytes: u64,
    pub build_time_ms: u64,
    pub last_optimized: DateTime<Utc>,
    pub search_accuracy: f64,
    pub avg_search_time_ms: f64,
}

/// Health status
#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub healthy: bool,
    pub status: String,
    pub last_check: DateTime<Utc>,
    pub response_time_ms: f64,
    pub error_count: usize,
    pub warnings: Vec<String>,
}

/// Distance metric enumeration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistanceMetric {
    Euclidean,
    Cosine,
    DotProduct,
    Manhattan,
    Hamming,
    Jaccard,
    Custom(String),
}

// Implementations

impl Default for SearchContext {
    fn default() -> Self {
        Self {
            collection_id: String::new(),
            query_vector: Vec::new(),
            k: 10,
            filters: None,
            strategy: SearchStrategy::Approximate { 
                target_recall: 0.9, 
                max_candidates: None 
            },
            algorithm_hints: HashMap::new(),
            threshold: None,
            timeout_ms: Some(10000), // 10 second default
            include_debug_info: false,
            include_vectors: false,
        }
    }
}

impl SearchContext {
    /// Create a simple similarity search context
    pub fn similarity_search(
        collection_id: impl Into<String>,
        query_vector: Vec<f32>,
        k: usize,
    ) -> Self {
        Self {
            collection_id: collection_id.into(),
            query_vector,
            k,
            ..Default::default()
        }
    }
    
    /// Add metadata filter
    pub fn with_filter(mut self, filter: MetadataFilter) -> Self {
        self.filters = Some(filter);
        self
    }
    
    /// Set search strategy
    pub fn with_strategy(mut self, strategy: SearchStrategy) -> Self {
        self.strategy = strategy;
        self
    }
    
    /// Enable debug information
    pub fn with_debug(mut self) -> Self {
        self.include_debug_info = true;
        self
    }
    
    /// Include vector data in results
    pub fn with_vectors(mut self) -> Self {
        self.include_vectors = true;
        self
    }
}

impl fmt::Display for SearchStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchStrategy::Exact => write!(f, "Exact"),
            SearchStrategy::Approximate { target_recall, .. } => {
                write!(f, "Approximate(recall={})", target_recall)
            }
            SearchStrategy::Hybrid { exact_threshold, .. } => {
                write!(f, "Hybrid(threshold={})", exact_threshold)
            }
            SearchStrategy::Progressive { tier_limits, .. } => {
                write!(f, "Progressive({} tiers)", tier_limits.len())
            }
            SearchStrategy::Adaptive { accuracy_preference, .. } => {
                write!(f, "Adaptive(accuracy={})", accuracy_preference)
            }
        }
    }
}

impl fmt::Display for IndexType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexType::HNSW { m, ef_construction, .. } => {
                write!(f, "HNSW(M={}, ef_construction={})", m, ef_construction)
            }
            IndexType::IVF { n_lists, n_probes } => {
                write!(f, "IVF(lists={}, probes={})", n_lists, n_probes)
            }
            IndexType::PQ { m, n_bits } => {
                write!(f, "PQ(m={}, bits={})", m, n_bits)
            }
            IndexType::LSH { n_tables, n_bits } => {
                write!(f, "LSH(tables={}, bits={})", n_tables, n_bits)
            }
            IndexType::Flat => write!(f, "Flat"),
            IndexType::BTree => write!(f, "BTree"),
            IndexType::Hash => write!(f, "Hash"),
            IndexType::Custom(name) => write!(f, "Custom({})", name),
        }
    }
}

impl StorageTier {
    /// Get tier priority (lower number = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            StorageTier::UltraHot => 0,
            StorageTier::Hot => 1,
            StorageTier::Warm => 2,
            StorageTier::Cold => 3,
        }
    }
    
    /// Get expected latency range
    pub fn expected_latency_ms(&self) -> (f64, f64) {
        match self {
            StorageTier::UltraHot => (0.1, 1.0),
            StorageTier::Hot => (1.0, 10.0),
            StorageTier::Warm => (10.0, 100.0),
            StorageTier::Cold => (100.0, 1000.0),
        }
    }
}

impl SearchStrategy {
    /// Get computational cost estimate (0.0 to 1.0)
    pub fn computational_cost(&self) -> f64 {
        match self {
            SearchStrategy::Exact => 1.0,
            SearchStrategy::Approximate { target_recall, .. } => {
                (1.0 - *target_recall as f64).max(0.1)
            }
            SearchStrategy::Hybrid { exact_threshold, .. } => {
                0.3 + (exact_threshold * 0.7) as f64
            }
            SearchStrategy::Progressive { .. } => 0.5,
            SearchStrategy::Adaptive { accuracy_preference, .. } => {
                *accuracy_preference as f64
            }
        }
    }
}

/// Index specification for creating new indexes
#[derive(Debug, Clone)]
pub struct IndexSpec {
    /// Name of the index
    pub name: String,
    
    /// Type of index to create
    pub index_type: IndexType,
    
    /// Columns/fields to index
    pub columns: Vec<String>,
    
    /// Whether the index enforces uniqueness
    pub unique: bool,
    
    /// Index-specific parameters
    pub parameters: serde_json::Value,
}

impl IndexSpec {
    /// Create a new vector similarity index specification
    pub fn vector_similarity(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            index_type: IndexType::HNSW { 
                m: 16, 
                ef_construction: 200, 
                ef_search: 50 
            },
            columns: vec!["vector".to_string()],
            unique: false,
            parameters: serde_json::json!({
                "distance_metric": "cosine",
                "enable_simd": true
            }),
        }
    }
    
    /// Create a new flat index specification for exact search
    pub fn flat_index(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            index_type: IndexType::Flat,
            columns: vec!["vector".to_string()],
            unique: false,
            parameters: serde_json::json!({
                "distance_metric": "cosine"
            }),
        }
    }
    
    /// Create a new IVF index specification for large datasets
    pub fn ivf_index(name: impl Into<String>, n_lists: usize, n_probes: usize) -> Self {
        Self {
            name: name.into(),
            index_type: IndexType::IVF { n_lists, n_probes },
            columns: vec!["vector".to_string()],
            unique: false,
            parameters: serde_json::json!({
                "distance_metric": "cosine",
                "n_lists": n_lists,
                "n_probes": n_probes,
                "train_size": 10000
            }),
        }
    }
}

// Helper functions for common operations

/// Create metadata filter for simple equality
pub fn eq_filter(field: impl Into<String>, value: Value) -> MetadataFilter {
    MetadataFilter::Field {
        field: field.into(),
        condition: FieldCondition::Equals(value),
    }
}

/// Create metadata filter for range queries
pub fn range_filter(
    field: impl Into<String>, 
    min: Value, 
    max: Value,
    inclusive: bool
) -> MetadataFilter {
    MetadataFilter::Field {
        field: field.into(),
        condition: FieldCondition::Range { min, max, inclusive },
    }
}

/// Create AND filter from multiple filters
pub fn and_filters(filters: Vec<MetadataFilter>) -> MetadataFilter {
    MetadataFilter::And(filters)
}

/// Create OR filter from multiple filters  
pub fn or_filters(filters: Vec<MetadataFilter>) -> MetadataFilter {
    MetadataFilter::Or(filters)
}