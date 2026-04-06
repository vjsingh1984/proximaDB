//! Defines the data structures for tracking symbols in the current query scope.

use crate::query::ast::Expr;
use std::collections::HashMap;

/// Represents a symbol in the query scope.
#[derive(Debug, Clone)]
pub enum Symbol {
    /// A table or collection.
    Table {
        /// Table or collection name
        name: String,
        /// Columns in the table by name
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
    /// Column name
    pub name: String,
    /// Data type of the column
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

    /// Searches for a column across all tables in the scope.
    /// Returns a tuple of (found_column, found_count) where found_count > 1 indicates ambiguity.
    pub fn find_column_in_tables(&self, column_name: &str) -> (Option<Column>, usize, Vec<String>) {
        let mut found_column: Option<Column> = None;
        let mut found_count = 0;
        let mut table_names = Vec::new();

        // Search all symbols in current scope
        for (table_key, symbol) in &self.symbols {
            if let Symbol::Table { columns, .. } = symbol
                && let Some(column) = columns.get(column_name)
            {
                if found_column.is_none() {
                    found_column = Some(column.clone());
                }
                found_count += 1;
                table_names.push(table_key.clone());
            }
        }

        // Also search in parent scope if exists
        if let Some(parent) = &self.parent {
            let (parent_column, parent_count, parent_tables) =
                parent.find_column_in_tables(column_name);
            if parent_column.is_some() {
                if found_column.is_none() {
                    found_column = parent_column;
                }
                found_count += parent_count;
                table_names.extend(parent_tables);
            }
        }

        (found_column, found_count, table_names)
    }
}
