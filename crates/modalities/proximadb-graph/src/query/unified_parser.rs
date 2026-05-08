/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Unified Cypher Query Parser
//!
//! This module provides the default parser for Cypher graph queries.
//! It uses the full recursive-descent `CypherParser` as the default implementation,
//! which provides comprehensive support for all Cypher clauses and expressions.
//!
//! ## Supported Clauses
//!
//! - **MATCH**: Pattern matching with nodes and relationships
//! - **OPTIONAL MATCH**: Optional pattern matching (null if no match)
//! - **WHERE**: Filtering predicates with AND/OR/NOT
//! - **RETURN**: Result projection with DISTINCT support
//! - **ORDER BY**: Result sorting (ASC/DESC)
//! - **LIMIT/SKIP**: Result pagination
//! - **CREATE**: Create nodes and relationships
//! - **SET**: Update properties
//! - **DELETE**: Remove nodes and relationships
//! - **WITH**: Query chaining and intermediate results
//! - **UNION**: Combine query results
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::graph::query::unified_parser::parse_cypher;
//!
//! let statement = parse_cypher("MATCH (n:Person) RETURN n")?;
//! ```
//!
//! ## Integration with Pattern Matcher
//!
//! The parser integrates with `PatternMatcher` through the `PatternCompiler`:
//!
//! ```rust,ignore
//! use proximadb::graph::query::{PatternMatcher, QueryContext};
//!
//! let matcher = PatternMatcher::new()?;
//! let context = QueryContext::new();
//! // Note: execute_query also requires memory_pool parameter
//! ```

use anyhow::Result;
use proximadb_kernel::error::ProximaDBError;

pub use super::cypher_ast::CypherStatement;
pub use super::cypher_parser::CypherParser;

/// Parse a Cypher query string using the default (full) parser.
///
/// This is the recommended entry point for parsing Cypher queries.
/// It uses the recursive-descent `CypherParser` which provides comprehensive
/// support for all Cypher clauses and expressions.
///
/// # Arguments
///
/// * `input` - The Cypher query string to parse
///
/// # Returns
///
/// A `CypherStatement` AST representing the parsed query
///
/// # Errors
///
/// Returns an error if the query string is invalid Cypher syntax
///
/// # Examples
///
/// ```rust,ignore
/// use proximadb::graph::query::unified_parser::parse_cypher;
///
/// // Simple match query
/// let stmt = parse_cypher("MATCH (n:Person) RETURN n")?;
///
/// // Complex query with WHERE and ORDER BY
/// let stmt = parse_cypher(
///     "MATCH (p:Person)-[:KNOWS]->(f:Person) \
///      WHERE p.age > 25 \
///      RETURN p.name, f.name \
///      ORDER BY p.name ASC \
///      LIMIT 10"
/// )?;
/// ```
pub fn parse_cypher(input: &str) -> Result<CypherStatement, ProximaDBError> {
    CypherParser::parse(input).map_err(|e| {
        ProximaDBError::InvalidInput(format!(
            "Failed to parse Cypher query: {}. Input: {}",
            e,
            if input.len() > 100 {
                format!("{}...", &input[..100])
            } else {
                input.to_string()
            }
        ))
    })
}

/// Parse a Cypher query with additional context information.
///
/// This variant allows passing context that may be used for error reporting
/// or parser configuration in the future.
///
/// # Arguments
///
/// * `input` - The Cypher query string to parse
/// * `query_name` - Optional name/identifier for the query (for error messages)
pub fn parse_cypher_with_context(
    input: &str,
    query_name: Option<&str>,
) -> Result<CypherStatement, ProximaDBError> {
    CypherParser::parse(input).map_err(|e| {
        let context = query_name
            .map(|n| format!(" (query: {})", n))
            .unwrap_or_default();

        ProximaDBError::InvalidInput(format!(
            "Failed to parse Cypher query{}: {}. Input: {}",
            context,
            e,
            if input.len() > 100 {
                format!("{}...", &input[..100])
            } else {
                input.to_string()
            }
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_match_return() {
        let result = parse_cypher("MATCH (n:Person) RETURN n");
        assert!(result.is_ok());

        let stmt = result.unwrap();
        assert!(!stmt.clauses.is_empty());
    }

    #[test]
    fn test_parse_complex_query() {
        let query = "MATCH (p:Person)-[:KNOWS]->(f:Person) \
                     WHERE p.age > 25 \
                     RETURN p.name, f.name \
                     ORDER BY p.name ASC \
                     LIMIT 10";

        let result = parse_cypher(query);
        assert!(result.is_ok());

        let stmt = result.unwrap();
        assert_eq!(stmt.clauses.len(), 5); // Match, Where, Return, OrderBy, Limit
    }

    #[test]
    fn test_parse_with_context() {
        let result = parse_cypher_with_context("MATCH (n:Person) RETURN n", Some("test_query"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_error_invalid_syntax() {
        let result = parse_cypher("MATCH (n:Person"); // Missing closing paren
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_case_insensitive_keywords() {
        // Keywords should be case-insensitive
        let result = parse_cypher("match (n:Person) where n.age > 25 return n");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_distinct_return() {
        let result = parse_cypher("MATCH (n:Person) RETURN DISTINCT n.city");
        assert!(result.is_ok());

        let stmt = result.unwrap();
        // Should have Match and Return clauses
        assert_eq!(stmt.clauses.len(), 2);
    }

    #[test]
    fn test_parse_with_context_includes_query_name_in_error() {
        let result = parse_cypher_with_context("MATCH (n:Person", Some("people_lookup"));
        let error = result.expect_err("invalid cypher should fail");

        let message = error.to_string();
        assert!(message.contains("people_lookup"));
        assert!(message.contains("Failed to parse Cypher query"));
    }

    #[test]
    fn test_parse_error_truncates_long_input() {
        let long_input = format!("MATCH (n:Person {}", "x".repeat(150));
        let result = parse_cypher(&long_input);
        let error = result.expect_err("invalid cypher should fail");

        let message = error.to_string();
        let truncated_prefix = format!("{}...", &long_input[..100]);
        assert!(message.contains("..."));
        assert!(message.contains(&truncated_prefix));
    }
}
