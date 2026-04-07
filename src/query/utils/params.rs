//! Parameter binding utilities for SQL and programmatic queries.

use std::collections::HashMap;

/// Named parameter bindings for query execution
#[derive(Debug, Clone)]
pub struct Params(pub HashMap<String, serde_json::Value>);

impl Params {
    /// Get a parameter value by name.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }
}
