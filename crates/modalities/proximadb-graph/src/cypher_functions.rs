//! Cypher function registry for graph query evaluation.

use std::collections::HashMap;

/// Registry of available Cypher functions
#[derive(Debug, Clone)]
pub struct CypherFunctionRegistry {
    functions: HashMap<String, CypherFunction>,
}

impl CypherFunctionRegistry {
    pub fn new() -> Self {
        Self { functions: HashMap::new() }
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

/// Cypher function definition (name + arity)
#[derive(Debug, Clone)]
pub struct CypherFunction {
    pub name: String,
    pub arity: usize,
}
