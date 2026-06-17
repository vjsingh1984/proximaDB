//! Compatibility wrapper for the extracted graph pattern runtime.
//!
//! The canonical `PatternMatcher` implementation is being migrated to the
//! `proximadb-graph` workspace crate. This module preserves the historical root
//! API surface.

use super::{CompiledPattern, MatchResult, QueryContext, QueryResult};
use crate::graph::GraphMemoryPool;
use std::sync::Arc;

// TODO: Move implementation to proximadb-graph crate
// For now, provide stub implementations

/// Pattern compiler
#[derive(Debug, Clone)]
pub struct PatternCompiler;

impl PatternCompiler {
    pub fn new() -> Self {
        Self
    }

    pub fn compile(&self, _pattern: &str) -> QueryResult<CompiledPattern> {
        // Placeholder implementation
        Ok(CompiledPattern {
            nodes: vec![],
            edges: vec![],
            paths: vec![],
            where_clause: None,
            where_clauses: vec![],
            with_clauses: vec![],
        })
    }
}

impl Default for PatternCompiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compatibility wrapper preserving the historical root `PatternMatcher` API.
pub struct PatternMatcher {
    _private: (),
}

impl PatternMatcher {
    pub fn new() -> QueryResult<Self> {
        Ok(Self { _private: () })
    }

    pub fn compile_pattern(&mut self, _pattern_str: &str) -> QueryResult<CompiledPattern> {
        Ok(CompiledPattern {
            nodes: vec![],
            edges: vec![],
            paths: vec![],
            where_clause: None,
            where_clauses: vec![],
            with_clauses: vec![],
        })
    }

    pub fn validate_query(&self, _query: &str) -> QueryResult<()> {
        Ok(())
    }

    pub fn execute_query(
        &mut self,
        _query: &str,
        _memory_pool: &Arc<GraphMemoryPool>,
        _context: &QueryContext,
    ) -> QueryResult<Vec<MatchResult>> {
        Ok(vec![])
    }

    pub fn execute_pattern(
        &self,
        _pattern: &CompiledPattern,
        _memory_pool: &Arc<GraphMemoryPool>,
        _context: &QueryContext,
    ) -> QueryResult<Vec<MatchResult>> {
        Ok(vec![])
    }

    pub fn apply_union(
        &self,
        _left_results: Vec<MatchResult>,
        _right_results: Vec<MatchResult>,
        _distinct: bool,
    ) -> QueryResult<Vec<MatchResult>> {
        Ok(vec![])
    }
}
