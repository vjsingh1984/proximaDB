//! Search query types

use std::collections::HashMap;

/// Search query for vector similarity matching with optional metadata filters
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_search_query_uses_empty_vector_top_ten_and_owned_filters() {
        let mut query = SearchQuery::default();
        assert!(query.query_vector.is_empty());
        assert_eq!(query.k, 10);
        assert!(query.filters.is_empty());

        query.query_vector = vec![0.1, 0.2, 0.3];
        query.k = 3;
        query.filters.insert("tenant".to_string(), json!("acme"));

        let cloned = query.clone();
        assert_eq!(cloned.query_vector, vec![0.1, 0.2, 0.3]);
        assert_eq!(cloned.k, 3);
        assert_eq!(cloned.filters.get("tenant"), Some(&json!("acme")));
        assert!(format!("{:?}", cloned).contains("query_vector"));
    }
}
