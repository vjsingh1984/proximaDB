pub mod sql_engine;
pub mod vector_search;

use crate::storage::StorageEngine;
use std::sync::Arc;
use tokio::sync::RwLock;

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
