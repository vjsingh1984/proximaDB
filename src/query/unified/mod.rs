//! Unified Multi-Model Query Engine
//!
//! This module provides cross-model query capabilities combining:
//! - Vector search (similarity queries)
//! - Document queries (JSON path filtering)
//! - Graph traversal (relationship navigation)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Unified Query                             │
//! │   "SELECT * FROM docs WHERE VECTOR_SIMILAR(...) AND         │
//! │    GRAPH_CONNECTED(source, 'KNOWS', target)"                │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 Query Decomposition                          │
//! │   Parse → Identify model operations → Create sub-queries    │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!         ┌────────────────────┼────────────────────┐
//!         ▼                    ▼                    ▼
//! ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
//! │ Vector Query │    │Document Query│    │ Graph Query  │
//! │   Engine     │    │   Engine     │    │   Engine     │
//! └──────────────┘    └──────────────┘    └──────────────┘
//!         │                    │                    │
//!         └────────────────────┼────────────────────┘
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Result Fusion                             │
//! │   Merge results by strategy (intersection, union, ranked)   │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Query Examples
//!
//! ```sql
//! -- Hybrid vector + document query
//! SELECT * FROM products
//! WHERE $.category = 'electronics'
//!   AND VECTOR_SIMILAR($.embedding, ?, 0.8)
//! ORDER BY VECTOR_DISTANCE($.embedding, ?) ASC
//! LIMIT 10;
//!
//! -- Cross-model join (document + graph)
//! SELECT d.*, g.relationship
//! FROM documents.users d
//! JOIN GRAPH knowledge ON d.id = GRAPH_START(knowledge)
//! WHERE GRAPH_TRAVERSE(knowledge, 'KNOWS', 2)
//!   AND d.$.status = 'active';
//!
//! -- Vector + graph fusion
//! SELECT v.id, v.score, g.path
//! FROM vectors.embeddings v
//! WHERE VECTOR_SIMILAR(v.vector, ?, 0.9)
//!   AND EXISTS (
//!     SELECT 1 FROM GRAPH relations
//!     WHERE GRAPH_CONNECTED(v.id, 'RELATED_TO', ?)
//!   );
//! ```

pub mod ast;
pub mod decomposition;
pub mod evolutionary;
pub mod executor;
pub mod fusion;
pub mod learned_fusion;
pub mod lower; // UQL to MultiModelPlan lowering (Issue #45, SB-15)
pub mod optimizer;
pub mod plan_execution_cache;
pub mod reranking;
pub mod uql;

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info};

pub use ast::{DataModel, MultiModelQuery, QueryComponent};
pub use decomposition::QueryDecomposer;
pub use executor::ParallelExecutor;
pub use fusion::{FusionStrategy, ResultFuser};
pub use learned_fusion::{
    FeedbackSignal, FusionFeatures, FusionModelType, LearnedFusion, LearnedFusionConfig,
    TrainingMetrics, TrainingSample,
};
pub use reranking::{CrossModalReranker, QueryContext, QueryIntent, RerankConfig, RerankedResult};
pub use uql::{UQLParser, UQLStatement};

use crate::services::operations::vectors::VectorOperationsService;
use crate::storage::document::DocumentService;
use crate::storage::traits::UnifiedStorageEngine;

/// Unified query engine for cross-model queries
pub struct UnifiedQueryEngine {
    /// Vector/document storage engine (Phase 2: direct engine access)
    #[allow(dead_code)]
    storage_engine: Arc<dyn UnifiedStorageEngine>,
    /// Vector operations service for vector searches (optional)
    vector_ops: Option<Arc<VectorOperationsService>>,
    /// Document service for JSON document queries
    document_service: Arc<DocumentService>,
    /// Query decomposer
    decomposer: QueryDecomposer,
    /// Parallel executor
    executor: ParallelExecutor,
    /// Result fuser
    fuser: ResultFuser,
    /// Configuration
    config: UnifiedQueryConfig,
}

/// Configuration for unified query engine
#[derive(Debug, Clone)]
pub struct UnifiedQueryConfig {
    /// Maximum parallel sub-queries
    pub max_parallel_queries: usize,
    /// Default fusion strategy
    pub default_fusion: FusionStrategy,
    /// Query timeout in milliseconds
    pub query_timeout_ms: u64,
    /// Enable query caching
    pub enable_cache: bool,
    /// Maximum cache entries
    pub max_cache_entries: usize,
}

impl Default for UnifiedQueryConfig {
    fn default() -> Self {
        Self {
            max_parallel_queries: 4,
            default_fusion: FusionStrategy::Intersection,
            query_timeout_ms: 30000,
            enable_cache: true,
            max_cache_entries: 1000,
        }
    }
}

impl UnifiedQueryEngine {
    /// Create a new unified query engine with full vector search support
    pub fn new(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        vector_ops: Arc<VectorOperationsService>,
        document_service: Arc<DocumentService>,
        config: UnifiedQueryConfig,
    ) -> Self {
        Self {
            storage_engine: storage_engine.clone(),
            vector_ops: Some(vector_ops),
            document_service,
            decomposer: QueryDecomposer::new(),
            executor: ParallelExecutor::new(config.max_parallel_queries),
            fuser: ResultFuser::new(config.default_fusion.clone()),
            config,
        }
    }

    /// Create a new unified query engine without vector operations (document + graph only)
    ///
    /// Note: This constructor is provided for backward compatibility but vector search
    /// queries will return empty results. Use `new()` with VectorOperationsService for
    /// full functionality.
    pub fn without_vector_ops(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        document_service: Arc<DocumentService>,
        config: UnifiedQueryConfig,
    ) -> Self {
        Self {
            storage_engine: storage_engine.clone(),
            vector_ops: None,
            document_service,
            decomposer: QueryDecomposer::new(),
            executor: ParallelExecutor::new(config.max_parallel_queries),
            fuser: ResultFuser::new(config.default_fusion.clone()),
            config,
        }
    }

    /// Execute a multi-model query
    pub async fn execute(&self, query: &str) -> Result<QueryResult> {
        info!("Executing unified query: {}", query);

        // 1. Parse and decompose the query
        let multi_model_query = self.decomposer.decompose(query)?;
        debug!(
            "Decomposed into {} components",
            multi_model_query.components.len()
        );

        // 2. Execute sub-queries in parallel
        let sub_results = self
            .executor
            .execute_parallel_with_services(
                &multi_model_query,
                self.vector_ops.clone(),
                self.document_service.clone(),
            )
            .await?;

        // 3. Fuse results based on strategy
        let fused_result = self
            .fuser
            .fuse(sub_results, &multi_model_query.fusion_strategy)?;

        Ok(fused_result)
    }

    /// Execute with a specific fusion strategy
    pub async fn execute_with_fusion(
        &self,
        query: &str,
        fusion: FusionStrategy,
    ) -> Result<QueryResult> {
        let mut multi_model_query = self.decomposer.decompose(query)?;
        multi_model_query.fusion_strategy = fusion;

        let sub_results = self
            .executor
            .execute_parallel_with_services(
                &multi_model_query,
                self.vector_ops.clone(),
                self.document_service.clone(),
            )
            .await?;

        self.fuser
            .fuse(sub_results, &multi_model_query.fusion_strategy)
    }

    /// Explain the query execution plan
    pub fn explain(&self, query: &str) -> Result<QueryPlan> {
        let multi_model_query = self.decomposer.decompose(query)?;

        // Compute estimated cost before moving fusion_strategy
        let estimated_total_cost = self.estimate_total_cost(&multi_model_query);

        Ok(QueryPlan {
            components: multi_model_query
                .components
                .iter()
                .map(|c| ComponentPlan {
                    model: c.model.clone(),
                    estimated_cost: self.estimate_cost(c),
                    parallelizable: c.is_parallelizable(),
                })
                .collect(),
            fusion_strategy: multi_model_query.fusion_strategy,
            estimated_total_cost,
        })
    }

    fn estimate_cost(&self, component: &QueryComponent) -> f64 {
        // Simple cost estimation based on model type
        match component.model {
            DataModel::Vector => 1.0,   // Vector search is typically fast
            DataModel::Document => 2.0, // Document queries vary
            DataModel::Graph => 3.0,    // Graph traversal can be expensive
            DataModel::Observability | DataModel::TimeSeries => 2.5,
            DataModel::Relational => 1.5,
            DataModel::Event => 2.0,
        }
    }

    fn estimate_total_cost(&self, query: &MultiModelQuery) -> f64 {
        // Parallel execution reduces total cost
        let component_costs: Vec<f64> = query
            .components
            .iter()
            .map(|c| self.estimate_cost(c))
            .collect();

        if query.components.len() <= self.config.max_parallel_queries {
            // All can run in parallel - cost is the max
            component_costs.iter().cloned().fold(0.0, f64::max)
        } else {
            // Some sequential execution needed
            component_costs.iter().sum::<f64>() / self.config.max_parallel_queries as f64
        }
    }
}

/// Result of a unified query
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Result records
    pub records: Vec<UnifiedRecord>,
    /// Total count (if available)
    pub total_count: Option<u64>,
    /// Execution metrics
    pub metrics: QueryMetrics,
}

/// A unified record from any data model
#[derive(Debug, Clone)]
pub struct UnifiedRecord {
    /// Record ID
    pub id: String,
    /// Source model
    pub source_model: DataModel,
    /// Record data as JSON
    pub data: serde_json::Value,
    /// Relevance score (if applicable)
    pub score: Option<f64>,
    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// Query execution metrics
#[derive(Debug, Clone, Default)]
pub struct QueryMetrics {
    /// Total execution time in microseconds
    pub total_time_us: u64,
    /// Time per sub-query
    pub sub_query_times: Vec<(DataModel, u64)>,
    /// Number of records scanned
    pub records_scanned: u64,
    /// Number of records returned
    pub records_returned: u64,
    /// Cache hit rate
    pub cache_hit_rate: f64,
}

/// Query execution plan
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Component plans
    pub components: Vec<ComponentPlan>,
    /// Fusion strategy
    pub fusion_strategy: FusionStrategy,
    /// Estimated total cost
    pub estimated_total_cost: f64,
}

/// Plan for a single query component
#[derive(Debug, Clone)]
pub struct ComponentPlan {
    /// Data model
    pub model: DataModel,
    /// Estimated cost (relative units)
    pub estimated_cost: f64,
    /// Whether this can run in parallel
    pub parallelizable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = UnifiedQueryConfig::default();
        assert_eq!(config.max_parallel_queries, 4);
        assert!(config.enable_cache);
    }
}

// RBAC integration tests
#[cfg(test)]
pub mod rbac_integration_tests;
