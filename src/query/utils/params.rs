//! Parameter binding utilities for SQL and programmatic queries.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Params(pub HashMap<String, serde_json::Value>);

impl Params {
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }
}
