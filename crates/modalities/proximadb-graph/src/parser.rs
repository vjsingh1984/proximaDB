//! Graph pattern query parser stubs.
//!
//! Legacy stub types maintained for root-crate compatibility shims.
//! The canonical Cypher/GQL parser lives in the query execution layer.

/// Parse result
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub query: GraphQuery,
}

/// Graph query (parsed representation)
#[derive(Debug, Clone)]
pub struct GraphQuery {
    pub patterns: Vec<Pattern>,
    pub filters: Vec<FilterExpression>,
    pub projections: Vec<Projection>,
}

/// Pattern
#[derive(Debug, Clone)]
pub struct Pattern {
    pub subject: String,
    pub predicate: Option<String>,
    pub object: String,
}

/// Filter expression
#[derive(Debug, Clone)]
pub enum FilterExpression {
    Property {
        variable: String,
        property: String,
        operator: String,
        value: String,
    },
}

/// Projection
#[derive(Debug, Clone)]
pub struct Projection {
    pub variable: String,
    pub property: Option<String>,
    pub alias: Option<String>,
}

/// Parse a graph query string
pub fn parse_query(_input: &str) -> Result<ParseResult, String> {
    Ok(ParseResult {
        query: GraphQuery {
            patterns: vec![],
            filters: vec![],
            projections: vec![],
        },
    })
}
