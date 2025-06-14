pub mod vector_search;
pub mod sql_engine;

use std::sync::Arc;
use tokio::sync::RwLock;
use crate::storage::StorageEngine;

#[derive(Debug)]
pub struct QueryEngine {
    // TODO: Add vector index, query planner, etc.
}

impl QueryEngine {
    pub async fn new(_storage: &StorageEngine) -> crate::Result<Self> {
        Ok(Self {})
    }
    
    pub async fn new_with_storage(_storage: Arc<RwLock<StorageEngine>>) -> crate::Result<Self> {
        Ok(Self {})
    }
    
    pub async fn new_placeholder() -> crate::Result<Self> {
        Ok(Self {})
    }
}