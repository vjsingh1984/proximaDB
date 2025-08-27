//! Query processing and optimization
//! 
//! Provides SQL and programmatic query interfaces with intelligent optimization

pub mod sql_engine;
pub mod vector_search;
pub mod unified_query_optimizer;

// Re-export main types
pub use sql_engine::{SqlEngine, SqlExecutionResult, SqlParser, QueryPlanner};
pub use vector_search::{VectorSearchQuery, VectorSearchResult, SearchParameters};
pub use unified_query_optimizer::{
    UnifiedQueryOptimizer as QueryOptimizer, 
    UnifiedExecutionPlan as QueryPlan,
    UnifiedMetadataFilter as MetadataFilter,
    UnifiedOptimizerConfig, UnifiedCostWeights,
};

use crate::storage::StorageEngine;
use crate::services::VectorOperationsService;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

/// Query Engine for ProximaDB
/// 
/// Unified interface for SQL and vector search queries with optimization
#[derive(Clone)]
pub struct QueryEngine {
    /// SQL query engine
    sql_engine: Option<Arc<SqlEngine>>,
    /// Direct vector service reference
    vector_service: Option<Arc<VectorOperationsService>>,
    /// Query optimizer
    optimizer: Arc<unified_query_optimizer::UnifiedQueryOptimizer>,
}

impl QueryEngine {
    /// Create new query engine with storage
    pub async fn new(_storage: &StorageEngine) -> crate::Result<Self> {
        Ok(Self {
            sql_engine: None,
            vector_service: None,
            optimizer: Arc::new(unified_query_optimizer::UnifiedQueryOptimizer::default()),
        })
    }

    /// Create with storage reference
    pub async fn new_with_storage(_storage: Arc<RwLock<StorageEngine>>) -> crate::Result<Self> {
        Ok(Self {
            sql_engine: None,
            vector_service: None,
            optimizer: Arc::new(unified_query_optimizer::UnifiedQueryOptimizer::default()),
        })
    }

    /// Create placeholder instance
    pub async fn new_placeholder() -> crate::Result<Self> {
        Ok(Self {
            sql_engine: None,
            vector_service: None,
            optimizer: Arc::new(unified_query_optimizer::UnifiedQueryOptimizer::default()),
        })
    }
    
    /// Create with vector service
    pub fn new_with_vector_service(vector_service: Arc<VectorOperationsService>) -> Self {
        let sql_engine = Arc::new(SqlEngine::new(vector_service.clone()));
        
        Self {
            sql_engine: Some(sql_engine),
            vector_service: Some(vector_service),
            optimizer: Arc::new(unified_query_optimizer::UnifiedQueryOptimizer::default()),
        }
    }
    
    /// Execute SQL query with optimization
    pub async fn execute_sql(&self, sql: &str) -> Result<SqlExecutionResult> {
        if let Some(sql_engine) = &self.sql_engine {
            // Apply query optimization
            let optimized_query = self.optimizer.optimize_sql(sql).await?;
            sql_engine.execute(&optimized_query).await
        } else {
            Err(anyhow::anyhow!("SQL engine not initialized"))
        }
    }
    
    /// Execute vector search query
    pub async fn execute_vector_search(&self, query: &VectorSearchQuery) -> Result<VectorSearchResult> {
        if let Some(vector_service) = &self.vector_service {
            // Apply search optimization
            let optimized_params = self.optimizer.optimize_vector_search(query).await?;
            vector_search::execute_search(vector_service.as_ref(), &optimized_params).await
        } else {
            Err(anyhow::anyhow!("Vector service not initialized"))
        }
    }
    
    /// Get vector service reference
    pub fn vector_service(&self) -> Option<&Arc<VectorOperationsService>> {
        self.vector_service.as_ref()
    }
    
    /// Get query optimizer
    pub fn optimizer(&self) -> &Arc<unified_query_optimizer::UnifiedQueryOptimizer> {
        &self.optimizer
    }
}