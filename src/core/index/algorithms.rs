//! Index algorithm implementations

/// Index algorithm configuration placeholder
#[derive(Debug, Clone)]
pub struct IndexAlgorithmConfig {
    /// Algorithm name (e.g., "hnsw", "ivf", "lsh")
    pub name: String,
    /// Algorithm-specific parameters as JSON key-value pairs
    pub parameters: std::collections::HashMap<String, serde_json::Value>,
}

impl Default for IndexAlgorithmConfig {
    fn default() -> Self {
        Self {
            name: "hnsw".to_string(),
            parameters: std::collections::HashMap::new(),
        }
    }
}
