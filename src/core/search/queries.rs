//! Search query types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Search query placeholder
#[derive(Debug, Clone, Serialize, Deserialize)]
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