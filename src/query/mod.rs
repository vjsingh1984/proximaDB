pub mod sql_engine;
pub mod vector_search;
pub mod unified_query_optimizer; // Consolidated optimizer (merged metadata filtering + search optimization)

// Stub module for compatibility with legacy code
pub mod unified_search_optimizer {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum OptimizationGoal {
        Speed,
        Memory,
        Recall,
        Latency,
        Balanced,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SearchHints {
        /// Primary optimization goal
        pub goal: OptimizationGoal,
        
        /// Minimum acceptable recall (0.0-1.0)
        pub recall_threshold: Option<f32>,
        
        /// Maximum memory budget in MB
        pub memory_budget_mb: Option<usize>,
        
        /// Maximum latency budget in milliseconds
        pub latency_budget_ms: Option<u64>,
    }
    
    impl Default for SearchHints {
        fn default() -> Self {
            Self {
                goal: OptimizationGoal::Balanced,
                recall_threshold: Some(0.9),
                memory_budget_mb: None,
                latency_budget_ms: None,
            }
        }
    }
}

// Stub module for consolidated query optimizer
pub mod unified_query_optimizer_consolidated {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct QueryOptimizerConfig {
        pub enable_optimization: bool,
        pub cache_size: usize,
    }
    
    impl Default for QueryOptimizerConfig {
        fn default() -> Self {
            Self {
                enable_optimization: true,
                cache_size: 1000,
            }
        }
    }
    
    #[derive(Debug, Clone)]
    pub struct ConsolidatedOptimizer;
    
    impl ConsolidatedOptimizer {
        pub fn new() -> Self {
            Self
        }
    }
}

use crate::storage::StorageEngine;
use crate::services::VectorOperationsService;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

pub use sql_engine::{SqlEngine, SqlExecutionResult};

/// Query Engine for ProximaDB
/// 
/// Provides both SQL and programmatic query interfaces.
#[derive(Clone)]
pub struct QueryEngine {
    /// SQL query engine
    sql_engine: Option<Arc<SqlEngine>>,
    /// Direct vector service reference
    vector_service: Option<Arc<VectorOperationsService>>,
}

impl QueryEngine {
    /// Create new query engine with storage
    pub async fn new(_storage: &StorageEngine) -> crate::Result<Self> {
        Ok(Self {
            sql_engine: None,
            vector_service: None,
        })
    }

    /// Create with storage reference
    pub async fn new_with_storage(_storage: Arc<RwLock<StorageEngine>>) -> crate::Result<Self> {
        Ok(Self {
            sql_engine: None,
            vector_service: None,
        })
    }

    /// Create placeholder instance
    pub async fn new_placeholder() -> crate::Result<Self> {
        Ok(Self {
            sql_engine: None,
            vector_service: None,
        })
    }
    
    /// Create with vector service
    pub fn new_with_vector_service(vector_service: Arc<VectorOperationsService>) -> Self {
        let sql_engine = Arc::new(SqlEngine::new(vector_service.clone()));
        
        Self {
            sql_engine: Some(sql_engine),
            vector_service: Some(vector_service),
        }
    }
    
    /// Execute SQL query
    pub async fn execute_sql(&self, sql: &str) -> Result<SqlExecutionResult> {
        if let Some(sql_engine) = &self.sql_engine {
            sql_engine.execute(sql).await
        } else {
            Err(anyhow::anyhow!("SQL engine not initialized"))
        }
    }
    
    /// Get vector service reference
    pub fn vector_service(&self) -> Option<&Arc<VectorOperationsService>> {
        self.vector_service.as_ref()
    }
}
