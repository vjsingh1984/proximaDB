/*
 * Copyright 2025 Vijaykumar Singh
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

//! Search trait and optimization framework for ProximaDB storage engines
//!
//! This module defines the search interfaces that each storage engine must implement,
//! allowing for storage-specific optimizations and pushdown operations.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

use crate::core::{CollectionId, VectorId};

/// Search operation type for cost estimation and optimization
#[derive(Debug, Clone, PartialEq)]
pub enum SearchOperation {
    /// Exact vector lookup by ID - O(1) or O(log n) depending on storage
    ExactLookup { vector_id: VectorId },
    /// Similarity search with vector - computational cost varies by algorithm
    SimilaritySearch {
        query_vector: Vec<f32>,
        k: usize,
        algorithm_hint: Option<String>, // "hnsw", "flat", "ivf", etc.
    },
    /// Metadata filtering - cost depends on indexing strategy
    MetadataFilter {
        filters: HashMap<String, FilterCondition>,
        limit: Option<usize>,
    },
    /// Combined operation: filter + similarity search
    HybridSearch {
        query_vector: Vec<f32>,
        k: usize,
        filters: HashMap<String, FilterCondition>,
        strategy: HybridSearchStrategy,
    },
    /// Range query over multiple vectors
    RangeQuery {
        start_id: Option<VectorId>,
        end_id: Option<VectorId>,
        limit: usize,
    },
}

/// Filter condition for metadata
#[derive(Debug, Clone, PartialEq)]
pub enum FilterCondition {
    Equals(Value),
    NotEquals(Value),
    GreaterThan(Value),
    GreaterThanOrEqual(Value),
    LessThan(Value),
    LessThanOrEqual(Value),
    In(Vec<Value>),
    NotIn(Vec<Value>),
    Contains(String),   // For string/array fields
    StartsWith(String), // For string fields
    IsNull,
    IsNotNull,
    And(Vec<FilterCondition>),
    Or(Vec<FilterCondition>),
}

/// Strategy for hybrid search operations
#[derive(Debug, Clone, PartialEq)]
pub enum HybridSearchStrategy {
    /// Filter first, then similarity search on filtered set
    FilterFirst,
    /// Similarity search first, then filter results
    SearchFirst,
    /// Parallel execution with result merging
    Parallel,
    /// Let storage engine decide based on cost estimation
    Auto,
}

/// Search result with metadata about the operation
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: VectorId,
    pub vector: Option<Vec<f32>>,
    pub metadata: HashMap<String, Value>,
    pub score: Option<f32>,    // Similarity score if applicable
    pub distance: Option<f32>, // Distance if applicable
    pub rank: Option<usize>,   // Rank in result set
}

/// Search execution plan with cost estimates
#[derive(Debug, Clone)]
pub struct SearchPlan {
    pub operation: SearchOperation,
    pub estimated_cost: SearchCost,
    pub execution_strategy: ExecutionStrategy,
    pub index_usage: Vec<IndexUsage>,
    pub pushdown_operations: Vec<PushdownOperation>,
}

/// Cost estimation for search operations
#[derive(Debug, Clone)]
pub struct SearchCost {
    pub cpu_cost: f64,               // Computational cost
    pub io_cost: f64,                // Disk I/O cost
    pub memory_cost: f64,            // Memory usage cost
    pub network_cost: f64,           // Network I/O cost (for distributed)
    pub total_estimated_ms: f64,     // Total estimated time in milliseconds
    pub cardinality_estimate: usize, // Expected result count
}

/// Execution strategy chosen by the storage engine
#[derive(Debug, Clone)]
pub enum ExecutionStrategy {
    /// Full table scan
    FullScan,
    /// Index scan using specific index
    IndexScan { index_name: String },
    /// Multiple index intersection
    IndexIntersection { indexes: Vec<String> },
    /// Vector index scan (HNSW/IVF/etc.)
    VectorIndexScan { algorithm: String },
    /// Hybrid approach combining multiple strategies
    Hybrid { strategies: Vec<ExecutionStrategy> },
}

/// Index usage information
#[derive(Debug, Clone)]
pub struct IndexUsage {
    pub index_name: String,
    pub index_type: String, // "btree", "hash", "hnsw", "ivf", etc.
    pub selectivity: f64,   // Fraction of data selected (0.0 to 1.0)
    pub scan_type: String,  // "eq", "range", "vector_sim", etc.
}

/// Operations that can be pushed down to storage
#[derive(Debug, Clone)]
pub enum PushdownOperation {
    /// Filter pushdown to reduce data movement
    Filter { condition: FilterCondition },
    /// Projection pushdown to reduce column reads
    Projection { columns: Vec<String> },
    /// Limit pushdown to reduce result set
    Limit { count: usize },
    /// Sort pushdown to storage engine
    Sort { column: String, ascending: bool },
    /// Aggregation pushdown
    Aggregation { operation: String },
}

/// Search statistics for optimization feedback
#[derive(Debug, Clone)]
pub struct SearchStats {
    pub operation_type: String,
    pub execution_time_ms: f64,
    pub rows_scanned: usize,
    pub rows_returned: usize,
    pub indexes_used: Vec<String>,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub io_operations: usize,
}

/// Main search trait that all storage engines must implement
#[async_trait]
pub trait SearchEngine: Send + Sync {
    /// Get the name of this search engine
    fn engine_name(&self) -> &'static str;

    /// Create an execution plan for the given search operation
    async fn create_plan(
        &self,
        collection_id: &CollectionId,
        operation: SearchOperation,
    ) -> Result<SearchPlan>;

    /// Execute a search operation with the given plan
    async fn execute_search(
        &self,
        collection_id: &CollectionId,
        plan: SearchPlan,
    ) -> Result<Vec<SearchResult>>;

    /// Execute search operation (convenience method that creates plan automatically)
    async fn search(
        &self,
        collection_id: &CollectionId,
        operation: SearchOperation,
    ) -> Result<Vec<SearchResult>> {
        let plan = self.create_plan(collection_id, operation).await?;
        self.execute_search(collection_id, plan).await
    }

    /// Get search statistics for optimization
    async fn get_search_stats(
        &self,
        collection_id: &CollectionId,
    ) -> Result<HashMap<String, SearchStats>>;

    /// Update search statistics (called after each operation)
    async fn update_search_stats(
        &self,
        collection_id: &CollectionId,
        stats: SearchStats,
    ) -> Result<()>;

    /// Get available indexes for the collection
    async fn list_indexes(&self, collection_id: &CollectionId) -> Result<Vec<IndexInfo>>;

    /// Create index for optimization
    async fn create_index(&self, collection_id: &CollectionId, index_spec: IndexSpec)
        -> Result<()>;

    /// Drop index
    async fn drop_index(&self, collection_id: &CollectionId, index_name: &str) -> Result<()>;

    /// Explain query execution plan (for debugging)
    async fn explain_plan(
        &self,
        collection_id: &CollectionId,
        operation: SearchOperation,
    ) -> Result<String>;
}

/// Index information
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub index_type: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub size_bytes: usize,
    pub cardinality: usize,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Index specification for creation
#[derive(Debug, Clone)]
pub struct IndexSpec {
    pub name: String,
    pub index_type: String, // "btree", "hash", "hnsw", "ivf", etc.
    pub columns: Vec<String>,
    pub unique: bool,
    pub parameters: HashMap<String, Value>, // Index-specific parameters
}

/// Storage engine specific search implementations
pub mod engines {

    /// VIPER storage engine search implementation
    pub struct ViperSearchEngine {
        // Implementation will be in viper/search_engine.rs
    }

    /// LSM storage engine search implementation  
    pub struct LsmSearchEngine {
        // Implementation will be in lsm/search_engine.rs
    }

    // /// RocksDB storage engine search implementation
    // pub struct RocksDbSearchEngine {
    //     // Implementation will be in rocksdb/search_engine.rs
    // }
    // Note: RocksDB was planned for sparse vectors KV store but never implemented

    /// Memory storage engine search implementation
    pub struct MemorySearchEngine {
        // Implementation will be in memory/search_engine.rs
    }
}

/// Search optimization utilities
pub mod optimizer {
    use super::*;

    /// Cost-based optimizer for search operations
    pub struct SearchOptimizer {
        pub collection_stats: HashMap<CollectionId, CollectionStats>,
        pub index_stats: HashMap<String, IndexStats>,
    }

    #[derive(Debug, Clone)]
    pub struct CollectionStats {
        pub total_vectors: usize,
        pub avg_vector_size: usize,
        pub metadata_cardinality: HashMap<String, usize>,
        pub data_distribution: DataDistribution,
    }

    #[derive(Debug, Clone)]
    pub struct IndexStats {
        pub selectivity: f64,
        pub access_frequency: usize,
        pub last_used: chrono::DateTime<chrono::Utc>,
    }

    #[derive(Debug, Clone)]
    pub enum DataDistribution {
        Uniform,
        Normal { mean: f64, std_dev: f64 },
        Skewed { skew_factor: f64 },
        Clustered { cluster_count: usize },
    }

    impl SearchOptimizer {
        pub fn new() -> Self {
            Self {
                collection_stats: HashMap::new(),
                index_stats: HashMap::new(),
            }
        }

        /// Choose optimal execution strategy based on cost estimates
        pub async fn optimize_search(
            &self,
            _collection_id: &CollectionId,
            _operation: &SearchOperation,
        ) -> Result<SearchPlan> {
            // Implementation will analyze costs and choose best strategy
            todo!("Implement cost-based optimization")
        }

        /// Update statistics based on execution results
        pub async fn update_stats(
            &mut self,
            _collection_id: &CollectionId,
            _operation: &SearchOperation,
            _stats: &SearchStats,
        ) -> Result<()> {
            // Implementation will update cost models
            todo!("Implement stats update")
        }
    }
}
