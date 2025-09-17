//! Defines the data structures for tracking symbols in the current query scope.

use std::collections::HashMap;
use crate::query::ast::Expr;

/// Represents a symbol in the query scope.
#[derive(Debug, Clone)]
pub enum Symbol {
    /// A table or collection.
    Table {
        name: String,
        columns: HashMap<String, Column>,
    },
    /// A column in a table.
    Column(Column),
    /// An alias for an expression.
    Alias(Expr),
}

/// Represents a column in a table.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
}

/// Represents the data type of a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    /// A string type.
    String,
    /// A 64-bit integer type.
    Int64,
    /// A 64-bit float type.
    Float64,
    /// A boolean type.
    Boolean,
    /// A vector type with a specific dimension.
    Vector(usize),
    /// An unknown type.
    Unknown,
}

/// Represents the scope of a query.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    symbols: HashMap<String, Symbol>,
    parent: Option<Box<Scope>>,
}

impl Scope {
    /// Creates a new scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new scope with a parent.
    pub fn new_with_parent(parent: Scope) -> Self {
        Self {
            symbols: HashMap::new(),
            parent: Some(Box::new(parent)),
        }
    }

    /// Inserts a symbol into the scope.
    pub fn insert(&mut self, name: &str, symbol: Symbol) {
        self.symbols.insert(name.to_string(), symbol);
    }

    /// Looks up a symbol in the scope and its parents.
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        if let Some(symbol) = self.symbols.get(name) {
            Some(symbol)
        } else if let Some(parent) = &self.parent {
            parent.lookup(name)
        } else {
            None
        }
    }
}