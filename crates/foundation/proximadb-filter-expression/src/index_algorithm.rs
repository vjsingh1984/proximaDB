//! Index algorithm configuration types

use std::collections::HashMap;

/// Index algorithm configuration (algorithm name + JSON key-value parameters)
#[derive(Debug, Clone)]
pub struct IndexAlgorithmConfig {
    pub name: String,
    pub parameters: HashMap<String, serde_json::Value>,
}

impl Default for IndexAlgorithmConfig {
    fn default() -> Self {
        Self {
            name: "hnsw".to_string(),
            parameters: HashMap::new(),
        }
    }
}
