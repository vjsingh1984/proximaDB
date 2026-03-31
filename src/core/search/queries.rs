//! Search query types

use std::collections::HashMap;

/// Search query placeholder
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Query vector for similarity matching
    pub query_vector: Vec<f32>,
    /// Number of nearest neighbors to return
    pub k: usize,
    /// Metadata filter predicates
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
