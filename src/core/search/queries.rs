//! Search query types

use std::collections::HashMap;

/// Search query placeholder
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub query_vector: Vec<f32>,
    pub k: usize,
    pub filters: HashMap<String, serde_json::Value>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query_vector: Vec::new(),
            k: 10,
            filters: HashMap::new(),
        }
    }
}