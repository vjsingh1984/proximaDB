//! Consolidated test module for the query system.
//!
//! This module contains all tests for the query subsystem, organized by category:
//! - parsing_tests: SQL parsing and AST construction
//! - semantic_tests: Semantic analysis and type checking
//! - planning_tests: Query planning and optimization
//! - execution_tests: Query execution
//! - optimization_tests: Query optimization strategies
//! - sks_tests: SKS (Semantic Knowledge System) specific tests

#[cfg(test)]
mod parsing_tests;

#[cfg(test)]
mod semantic_tests;

#[cfg(test)]
mod planning_tests;

#[cfg(test)]
mod execution_tests;

#[cfg(test)]
mod optimization_tests;

#[cfg(test)]
mod sks_tests;
