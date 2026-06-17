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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_index_algorithm_is_hnsw_and_parameters_are_owned() {
        let mut config = IndexAlgorithmConfig::default();
        assert_eq!(config.name, "hnsw");
        assert!(config.parameters.is_empty());

        config.parameters.insert("ef".to_string(), json!(128));
        let cloned = config.clone();

        assert_eq!(cloned.parameters.get("ef"), Some(&json!(128)));
        assert!(format!("{:?}", cloned).contains("hnsw"));
    }
}
