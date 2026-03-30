//! # Federated Parser
//!
//! Extends SQL with multi-model query capabilities.
//!
//! ## Supported Extensions
//!
//! - **VECTOR_SEARCH(collection, query_vector, top_k)**: Similarity search
//! - **GRAPH_QUERY('cypher_query')**: Graph traversal via Cypher
//! - **DOCUMENT_QUERY(collection, filter)**: Document queries
//! - **LOGS(namespace)**: Observability log queries
//! - **METRICS(namespace)**: Observability metric queries
//! - **<->** operator: Vector distance (pgvector compatible)

use anyhow::Result;
use std::collections::HashMap;

/// Type of query being executed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryType {
    /// Pure SQL query (RDBMS)
    Sql,
    /// Vector similarity search
    VectorSearch,
    /// Graph traversal query
    GraphQuery,
    /// Document query
    DocumentQuery,
    /// Observability log query
    LogQuery,
    /// Observability metric query
    MetricQuery,
    /// Cross-model federated query
    Federated,
}

/// Parsed vector argument for VECTOR_SEARCH and distance operators
#[derive(Debug, Clone, PartialEq)]
pub enum VectorQuery {
    /// Literal vector value from SQL, e.g. `[0.1, 0.2]`
    Literal(Vec<f32>),
    /// Raw SQL expression or column reference, e.g. `u.embedding`
    Expression(String),
}

/// SQL extension type detected in query
#[derive(Debug, Clone, PartialEq)]
pub enum SqlExtension {
    /// VECTOR_SEARCH(collection, vector, top_k)
    VectorSearch {
        collection: String,
        query_vector: VectorQuery,
        top_k: usize,
    },
    /// GRAPH_QUERY('cypher')
    GraphQuery { cypher: String },
    /// DOCUMENT_QUERY(collection, filter)
    DocumentQuery {
        collection: String,
        filter: Option<String>,
    },
    /// LOGS(namespace)
    Logs { namespace: String },
    /// METRICS(namespace)
    Metrics { namespace: String },
    /// Vector distance operator <->
    VectorDistance {
        left_column: String,
        right_literal: String,
    },
}

/// Parsed federated query
#[derive(Debug, Clone)]
pub struct FederatedQuery {
    /// Original SQL text
    pub sql: String,
    /// Primary query type
    pub query_type: QueryType,
    /// Detected SQL extensions
    pub extensions: Vec<SqlExtension>,
    /// Target tables/collections
    pub targets: Vec<QueryTarget>,
    /// Extracted parameters
    pub parameters: HashMap<String, String>,
    /// Whether this is a cross-model join
    pub is_cross_model_join: bool,
}

/// Target of a query operation
#[derive(Debug, Clone)]
pub struct QueryTarget {
    /// Name of the table/collection
    pub name: String,
    /// Alias if specified
    pub alias: Option<String>,
    /// Model type for this target
    pub model_type: TargetModelType,
}

/// Model type for a query target
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetModelType {
    /// RDBMS table
    Table,
    /// Vector collection
    VectorCollection,
    /// Document collection
    DocumentCollection,
    /// Graph
    Graph,
    /// Observability namespace
    Observability,
    /// Unknown (needs resolution)
    Unknown,
}

/// Federated parser for SQL with multi-model extensions
pub struct FederatedParser {
    /// Extension keywords to detect
    extension_keywords: Vec<&'static str>,
}

impl FederatedParser {
    /// Create a new federated parser
    pub fn new() -> Self {
        Self {
            extension_keywords: vec![
                "VECTOR_SEARCH",
                "GRAPH_QUERY",
                "DOCUMENT_QUERY",
                "LOGS",
                "METRICS",
                "<->",
                "::vector",
            ],
        }
    }

    /// Get supported SQL extensions
    pub fn supported_extensions(&self) -> &[&'static str] {
        &self.extension_keywords
    }

    /// Parse a SQL query with extensions
    pub fn parse(&self, sql: &str) -> Result<FederatedQuery> {
        let sql_upper = sql.to_uppercase();
        let mut extensions = Vec::new();
        let mut query_type = QueryType::Sql;
        let mut is_cross_model_join = false;

        // Detect VECTOR_SEARCH extension
        if let Some(ext) = self.parse_vector_search(sql) {
            extensions.push(ext);
            query_type = QueryType::VectorSearch;
        }

        // Detect GRAPH_QUERY extension
        if let Some(ext) = self.parse_graph_query(sql) {
            extensions.push(ext);
            query_type = QueryType::GraphQuery;
        }

        // Detect DOCUMENT_QUERY extension
        if let Some(ext) = self.parse_document_query(sql) {
            extensions.push(ext);
            query_type = QueryType::DocumentQuery;
        }

        // Detect LOGS extension
        if let Some(ext) = self.parse_logs_query(sql) {
            extensions.push(ext);
            query_type = QueryType::LogQuery;
        }

        // Detect METRICS extension
        if let Some(ext) = self.parse_metrics_query(sql) {
            extensions.push(ext);
            query_type = QueryType::MetricQuery;
        }

        // Detect vector distance operator <->
        if let Some(ext) = self.parse_vector_distance(sql) {
            extensions.push(ext);
            if query_type == QueryType::Sql {
                query_type = QueryType::VectorSearch;
            }
        }

        // Parse FROM clause targets (excluding function calls)
        let targets = self.parse_from_targets(sql);

        // Filter out function calls (like VECTOR_SEARCH, GRAPH_QUERY) from targets
        let real_target_count = targets
            .iter()
            .filter(|t| {
                let upper = t.name.to_uppercase();
                !upper.starts_with("VECTOR_SEARCH")
                    && !upper.starts_with("GRAPH_QUERY")
                    && !upper.starts_with("DOCUMENT_QUERY")
                    && !upper.starts_with("LOGS")
                    && !upper.starts_with("METRICS")
            })
            .count();

        // Detect cross-model joins (only when we have multiple real tables or extensions)
        if extensions.len() > 1 || (extensions.len() >= 1 && real_target_count > 1) {
            is_cross_model_join = true;
            query_type = QueryType::Federated;
        }

        // Check for JOIN with different model types
        if sql_upper.contains("JOIN") && sql_upper.contains("LATERAL")
            && extensions.len() > 0 {
                is_cross_model_join = true;
                query_type = QueryType::Federated;
            }

        Ok(FederatedQuery {
            sql: sql.to_string(),
            query_type,
            extensions,
            targets,
            parameters: HashMap::new(),
            is_cross_model_join,
        })
    }

    /// Parse VECTOR_SEARCH(collection, vector, top_k)
    fn parse_vector_search(&self, sql: &str) -> Option<SqlExtension> {
        let args = self.extract_function_args(sql, "VECTOR_SEARCH")?;
        let parts = self.split_function_args(args);
        if parts.len() < 2 {
            return None;
        }

        let collection = parts[0].trim_matches('\'').trim_matches('"').to_string();
        let query_vector = self.parse_vector_argument(&parts[1])?;
        let top_k = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);

        Some(SqlExtension::VectorSearch {
            collection,
            query_vector,
            top_k,
        })
    }

    /// Parse GRAPH_QUERY('cypher')
    fn parse_graph_query(&self, sql: &str) -> Option<SqlExtension> {
        let cypher = self
            .extract_function_args(sql, "GRAPH_QUERY")?
            .trim()
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
        Some(SqlExtension::GraphQuery { cypher })
    }

    /// Parse DOCUMENT_QUERY(collection, filter)
    fn parse_document_query(&self, sql: &str) -> Option<SqlExtension> {
        let args = self.extract_function_args(sql, "DOCUMENT_QUERY")?;
        let parts = self.split_function_args(args);
        let collection = parts
            .first()?
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
        let filter = parts
            .get(1)
            .map(|s| s.trim_matches('\'').trim_matches('"').to_string());
        Some(SqlExtension::DocumentQuery { collection, filter })
    }

    /// Parse LOGS(namespace)
    fn parse_logs_query(&self, sql: &str) -> Option<SqlExtension> {
        let namespace = self
            .extract_function_args(sql, "LOGS")?
            .trim()
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
        Some(SqlExtension::Logs { namespace })
    }

    /// Parse METRICS(namespace)
    fn parse_metrics_query(&self, sql: &str) -> Option<SqlExtension> {
        let namespace = self
            .extract_function_args(sql, "METRICS")?
            .trim()
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
        Some(SqlExtension::Metrics { namespace })
    }

    /// Parse vector distance operator <->
    fn parse_vector_distance(&self, sql: &str) -> Option<SqlExtension> {
        if !sql.contains("<->") {
            return None;
        }

        // Find <-> and extract left/right operands
        if let Some(pos) = sql.find("<->") {
            // Simple extraction - find word before and after
            let before = sql[..pos].trim();
            let after = sql[pos + 3..].trim();

            // Get last word before <->
            let left_column = before
                .split_whitespace()
                .last()
                .unwrap_or("embedding")
                .to_string();

            // Get first token after <->
            let right_literal = after.split_whitespace().next().unwrap_or("[]").to_string();

            return Some(SqlExtension::VectorDistance {
                left_column,
                right_literal,
            });
        }
        None
    }

    /// Parse FROM clause to extract table targets
    fn parse_from_targets(&self, sql: &str) -> Vec<QueryTarget> {
        let mut targets = Vec::new();
        let upper = sql.to_uppercase();

        // Find FROM clause
        if let Some(from_pos) = upper.find("FROM") {
            let after_from = &sql[from_pos + 4..];

            // Find end of FROM clause (WHERE, JOIN, ORDER, GROUP, LIMIT, etc.)
            let end_keywords = ["WHERE", "JOIN", "ORDER", "GROUP", "LIMIT", "HAVING", ";"];
            let mut end_pos = after_from.len();
            for keyword in end_keywords {
                if let Some(pos) = after_from.to_uppercase().find(keyword)
                    && pos < end_pos {
                        end_pos = pos;
                    }
            }

            let from_clause = after_from[..end_pos].trim();

            // Check if this is a function call (contains parentheses at start)
            let from_upper = from_clause.to_uppercase();
            let is_function_call = from_upper.starts_with("VECTOR_SEARCH")
                || from_upper.starts_with("GRAPH_QUERY")
                || from_upper.starts_with("DOCUMENT_QUERY")
                || from_upper.starts_with("LOGS(")
                || from_upper.starts_with("METRICS(");

            // If it's a function call, don't split by comma
            if is_function_call {
                return targets; // Return empty - no real table targets
            }

            // Split by comma for multiple tables (respecting parentheses depth)
            let table_refs = self.split_respecting_parens(from_clause);
            for table_ref in table_refs {
                let parts: Vec<&str> = table_ref.trim().split_whitespace().collect();
                if !parts.is_empty() {
                    let name = parts[0].to_string();
                    let alias = if parts.len() > 1 && parts[1].to_uppercase() != "AS" {
                        Some(parts[1].to_string())
                    } else if parts.len() > 2 {
                        Some(parts[2].to_string())
                    } else {
                        None
                    };

                    targets.push(QueryTarget {
                        name,
                        alias,
                        model_type: TargetModelType::Unknown,
                    });
                }
            }
        }

        targets
    }

    /// Split a string by commas, respecting parentheses depth
    fn split_respecting_parens(&self, s: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let mut depth = 0;

        for c in s.chars() {
            match c {
                '(' => {
                    depth += 1;
                    current.push(c);
                }
                ')' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => {
                    if !current.trim().is_empty() {
                        result.push(current.trim().to_string());
                    }
                    current = String::new();
                }
                _ => {
                    current.push(c);
                }
            }
        }

        if !current.trim().is_empty() {
            result.push(current.trim().to_string());
        }

        result
    }

    /// Split function arguments while respecting quotes, brackets, and parentheses.
    fn split_function_args(&self, s: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = String::new();
        let mut paren_depth = 0;
        let mut bracket_depth = 0;
        let mut in_quote = None;
        let mut escaped = false;

        for c in s.chars() {
            if let Some(quote) = in_quote {
                current.push(c);
                if c == quote && !escaped {
                    in_quote = None;
                }
                escaped = c == '\\' && !escaped;
                continue;
            }

            match c {
                '\'' | '"' => {
                    in_quote = Some(c);
                    current.push(c);
                }
                '(' => {
                    paren_depth += 1;
                    current.push(c);
                }
                ')' => {
                    paren_depth -= 1;
                    current.push(c);
                }
                '[' => {
                    bracket_depth += 1;
                    current.push(c);
                }
                ']' => {
                    bracket_depth -= 1;
                    current.push(c);
                }
                ',' if paren_depth == 0 && bracket_depth == 0 => {
                    if !current.trim().is_empty() {
                        result.push(current.trim().to_string());
                    }
                    current.clear();
                }
                _ => current.push(c),
            }

            escaped = false;
        }

        if !current.trim().is_empty() {
            result.push(current.trim().to_string());
        }

        result
    }

    fn extract_function_args<'a>(&self, sql: &'a str, function_name: &str) -> Option<&'a str> {
        let upper = sql.to_uppercase();
        let function_call = format!("{}(", function_name);
        let start = upper.find(&function_call)?;
        let content = &sql[start + function_name.len() + 1..];
        let mut depth = 1;
        let mut in_quote = None;
        let mut escaped = false;

        for (i, c) in content.char_indices() {
            if let Some(quote) = in_quote {
                if c == quote && !escaped {
                    in_quote = None;
                }
                escaped = c == '\\' && !escaped;
                continue;
            }

            match c {
                '\'' | '"' => in_quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&content[..i]);
                    }
                }
                _ => {}
            }

            escaped = false;
        }

        None
    }

    fn parse_vector_argument(&self, arg: &str) -> Option<VectorQuery> {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            return None;
        }

        if let Some(literal) = Self::parse_vector_literal(trimmed) {
            return Some(VectorQuery::Literal(literal));
        }

        Some(VectorQuery::Expression(trimmed.to_string()))
    }

    fn parse_vector_literal(raw: &str) -> Option<Vec<f32>> {
        let trimmed = raw.trim();
        let without_cast = trimmed
            .strip_suffix("::vector")
            .or_else(|| trimmed.strip_suffix("::VECTOR"))
            .unwrap_or(trimmed)
            .trim();
        let unquoted = without_cast.trim_matches('\'').trim_matches('"').trim();

        if !(unquoted.starts_with('[') && unquoted.ends_with(']')) {
            return None;
        }

        let inner = &unquoted[1..unquoted.len() - 1];
        if inner.trim().is_empty() {
            return Some(Vec::new());
        }

        inner
            .split(',')
            .map(|value| value.trim().parse::<f32>().ok())
            .collect()
    }

    /// Resolve model types for targets using catalog
    pub fn resolve_model_types(&self, query: &mut FederatedQuery, catalog: &impl ModelTypeCatalog) {
        for target in &mut query.targets {
            target.model_type = catalog.get_model_type(&target.name);
        }
    }
}

impl Default for FederatedParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for resolving model types from catalog
pub trait ModelTypeCatalog {
    fn get_model_type(&self, name: &str) -> TargetModelType;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_sql() {
        let parser = FederatedParser::new();
        let query = parser.parse("SELECT * FROM users WHERE id = 1").unwrap();
        assert_eq!(query.query_type, QueryType::Sql);
        assert!(query.extensions.is_empty());
        assert_eq!(query.targets.len(), 1);
        assert_eq!(query.targets[0].name, "users");
    }

    #[test]
    fn test_parse_vector_search() {
        let parser = FederatedParser::new();
        let query = parser
            .parse("SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1,0.2]', 10)")
            .unwrap();
        assert_eq!(query.query_type, QueryType::VectorSearch);
        assert_eq!(query.extensions.len(), 1);
        match &query.extensions[0] {
            SqlExtension::VectorSearch {
                collection,
                query_vector,
                top_k,
            } => {
                assert_eq!(collection, "embeddings");
                assert_eq!(*query_vector, VectorQuery::Literal(vec![0.1, 0.2]));
                assert_eq!(*top_k, 10);
            }
            _ => panic!("Expected VectorSearch extension"),
        }
    }

    #[test]
    fn test_parse_vector_search_with_expression() {
        let parser = FederatedParser::new();
        let query = parser
            .parse("SELECT * FROM VECTOR_SEARCH('embeddings', u.preference_vector, 5)")
            .unwrap();

        match &query.extensions[0] {
            SqlExtension::VectorSearch { query_vector, .. } => {
                assert_eq!(
                    *query_vector,
                    VectorQuery::Expression("u.preference_vector".to_string())
                );
            }
            _ => panic!("Expected VectorSearch extension"),
        }
    }

    #[test]
    fn test_parse_graph_query() {
        let parser = FederatedParser::new();
        let query = parser
            .parse("SELECT * FROM GRAPH_QUERY('MATCH (a)-[:KNOWS]->(b) RETURN b.name')")
            .unwrap();
        assert_eq!(query.query_type, QueryType::GraphQuery);
        assert_eq!(query.extensions.len(), 1);
        match &query.extensions[0] {
            SqlExtension::GraphQuery { cypher } => {
                assert!(cypher.contains("MATCH"));
                assert!(cypher.contains("KNOWS"));
            }
            _ => panic!("Expected GraphQuery extension"),
        }
    }

    #[test]
    fn test_parse_vector_distance() {
        let parser = FederatedParser::new();
        let query = parser
            .parse("SELECT * FROM products ORDER BY embedding <-> '[0.1,0.2]'::vector LIMIT 10")
            .unwrap();
        assert_eq!(query.query_type, QueryType::VectorSearch);
        assert_eq!(query.extensions.len(), 1);
        match &query.extensions[0] {
            SqlExtension::VectorDistance { left_column, .. } => {
                assert_eq!(left_column, "embedding");
            }
            _ => panic!("Expected VectorDistance extension"),
        }
    }

    #[test]
    fn test_parse_logs_query() {
        let parser = FederatedParser::new();
        let query = parser
            .parse("SELECT * FROM LOGS('production') WHERE timestamp > now() - interval '1h'")
            .unwrap();
        assert_eq!(query.query_type, QueryType::LogQuery);
        match &query.extensions[0] {
            SqlExtension::Logs { namespace } => {
                assert_eq!(namespace, "production");
            }
            _ => panic!("Expected Logs extension"),
        }
    }

    #[test]
    fn test_parse_cross_model_join() {
        let parser = FederatedParser::new();
        let query = parser.parse(
            "SELECT u.*, v.similar FROM users u JOIN LATERAL VECTOR_SEARCH('embeddings', u.vector, 10) v ON true"
        ).unwrap();
        assert_eq!(query.query_type, QueryType::Federated);
        assert!(query.is_cross_model_join);
    }

    #[test]
    fn test_supported_extensions() {
        let parser = FederatedParser::new();
        let extensions = parser.supported_extensions();
        assert!(extensions.contains(&"VECTOR_SEARCH"));
        assert!(extensions.contains(&"GRAPH_QUERY"));
        assert!(extensions.contains(&"LOGS"));
    }
}
