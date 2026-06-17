//! Parameter binding utilities for SQL and programmatic queries.

use std::collections::HashMap;

/// Named parameter bindings for query execution
#[derive(Debug, Clone)]
pub struct Params(pub HashMap<String, serde_json::Value>);

impl Params {
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_get_returns_bound_values_and_misses() {
        let mut values = HashMap::new();
        values.insert("tenant".to_string(), json!("acme"));
        values.insert("limit".to_string(), json!(10));
        let params = Params(values);

        assert_eq!(params.get("tenant"), Some(&json!("acme")));
        assert_eq!(params.get("limit"), Some(&json!(10)));
        assert_eq!(params.get("missing"), None);
    }
}
