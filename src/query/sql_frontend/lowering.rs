//! Lowering from SQL-frontend AST (or external AST) to internal AST nodes.

use anyhow::Result;
use crate::query::ast::Query;

pub struct Lowering;

impl Lowering {
    pub fn new() -> Self { Self }

    /// Lower from a frontend AST into the internal AST. Placeholder until parser lands.
    pub fn lower_from_sqlparser(&self, _sql: &str) -> Result<Query> {
        anyhow::bail!("Lowering not implemented (design stub)")
    }
}

