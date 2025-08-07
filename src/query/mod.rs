pub mod sql_engine;
pub mod vector_search;
pub mod compression_aware_planner;
pub mod unified_query_planner;

use crate::storage::StorageEngine;
use crate::services::DirectVectorService;
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
    vector_service: Option<Arc<DirectVectorService>>,
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
    pub fn new_with_vector_service(vector_service: Arc<DirectVectorService>) -> Self {
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
    pub fn vector_service(&self) -> Option<&Arc<DirectVectorService>> {
        self.vector_service.as_ref()
    }
}
