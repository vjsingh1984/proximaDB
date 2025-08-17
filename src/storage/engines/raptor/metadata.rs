use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorMetadata {
    pub version: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub total_vectors: usize,
    pub total_rowgroups: usize,
    pub dimension: usize,
    pub compression_codec: String,
    pub index_type: String,
    pub custom_metadata: HashMap<String, String>,
}

impl Default for RaptorMetadata {
    fn default() -> Self {
        Self {
            version: "1.0.0".to_string(),
            created_at: chrono::Utc::now(),
            total_vectors: 0,
            total_rowgroups: 0,
            dimension: 0,
            compression_codec: "none".to_string(),
            index_type: "hnsw".to_string(),
            custom_metadata: HashMap::new(),
        }
    }
}