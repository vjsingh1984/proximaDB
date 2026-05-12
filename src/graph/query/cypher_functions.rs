//! Compatibility shim for Cypher function support.
//!
//! The canonical Cypher function registry is being migrated to the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface.

// TODO: Move implementation from src/graph/query to proximadb-graph crate
// pub use proximadb_graph::query::cypher_functions::*;

use std::collections::HashMap;

/// Cypher function registry
#[derive(Debug, Clone)]
pub struct CypherFunctionRegistry {
    functions: HashMap<String, CypherFunction>,
}

impl CypherFunctionRegistry {
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: String, function: CypherFunction) {
        self.functions.insert(name, function);
    }

    pub fn get(&self, name: &str) -> Option<&CypherFunction> {
        self.functions.get(name)
    }
}

impl Default for CypherFunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Cypher function definition
#[derive(Debug, Clone)]
pub struct CypherFunction {
    pub name: String,
    pub arity: usize,
}
