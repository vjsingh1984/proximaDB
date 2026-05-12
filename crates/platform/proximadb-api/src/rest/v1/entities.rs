//! # Entity Handlers
//!
//! Entity and vector CRUD operations.

use serde::{Deserialize, Serialize};

/// Entity handler for generic entity operations
pub struct EntityHandler {
    // Service dependencies will be added here
}

impl EntityHandler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for EntityHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Vector handler for vector operations
pub struct VectorHandler {
    // Service dependencies will be added here
}

impl VectorHandler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for VectorHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Entity insert request
#[derive(Debug, Deserialize)]
pub struct InsertEntityRequest {
    pub collection: String,
    pub entities: Vec<EntityData>,
}

/// Entity data
#[derive(Debug, Deserialize, Serialize)]
pub struct EntityData {
    pub id: Option<String>,
    pub vector: Option<Vec<f32>>,
    pub properties: Option<serde_json::Value>,
}

// TODO: Move entity logic from src/network/rest/v1/entities.rs
