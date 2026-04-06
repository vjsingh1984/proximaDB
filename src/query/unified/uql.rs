//! Unified Query Language (UQL) Parser
//!
//! A SQL-like query language for multi-modal queries that combines:
//! - Vector similarity search (`VECTOR_SIMILAR`, `VECTOR_DISTANCE`)
//! - Document JSON path queries (`$.field`, `JSON_PATH`)
//! - Graph traversal (`GRAPH_TRAVERSE`, `GRAPH_CONNECTED`, `GRAPH_PATH`)
//! - Observability queries (`LOGS`, `METRICS`, `TRACES`)
//!
//! ## Syntax Examples
//!
//! ```sql
//! -- Vector search with metadata filter
//! SELECT * FROM vectors.products
//! WHERE VECTOR_SIMILAR(embedding, ?, 0.8)
//!   AND $.category = 'electronics'
//! LIMIT 10;
//!
//! -- Hybrid vector + graph query
//! SELECT v.*, g.path FROM vectors.items v
//! JOIN GRAPH knowledge g ON v.id = g.start_node
//! WHERE VECTOR_SIMILAR(v.embedding, ?, 0.7)
//!   AND GRAPH_CONNECTED(g, 'RELATED_TO', 2)
//! FUSION RRF(60);
//!
//! -- Multi-modal fusion query
//! MULTIMODAL {
//!     VECTOR: SELECT id, score FROM embeddings WHERE SIMILAR(?, 0.8) LIMIT 100,
//!     DOCUMENT: SELECT id FROM docs WHERE $.status = 'active',
//!     GRAPH: TRAVERSE FROM ? VIA 'KNOWS' DEPTH 2
//! } FUSION INTERSECTION;
//! ```

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use tracing::debug;

use super::ast::{
    DataModel, DistanceMetric, DocumentQueryExpr, FilterOperator, FilterValue, GraphTraversalExpr,
    LogQueryExpr, ModelOperation, MultiModelQuery, PathFilter, QueryComponent, StartNodeSpec,
    TraversalDirection, VectorSearchExpr, VectorSearchParams,
};
use super::fusion::FusionStrategy;

/// UQL Statement types
#[derive(Debug, Clone)]
pub enum UQLStatement {
    /// Standard SELECT query
    Select(SelectStatement),
    /// Multi-modal query with explicit components
    MultiModal(MultiModalStatement),
    /// EXPLAIN query for query planning
    Explain(Box<UQLStatement>),
}

/// SELECT statement structure
#[derive(Debug, Clone)]
pub struct SelectStatement {
    /// Columns to select (* or specific columns)
    pub columns: Vec<String>,
    /// Primary data source
    pub from: DataSource,
    /// Join clauses
    pub joins: Vec<JoinClause>,
    /// WHERE conditions
    pub where_clause: Option<WhereClause>,
    /// ORDER BY
    pub order_by: Option<OrderByClause>,
    /// LIMIT
    pub limit: Option<u32>,
    /// OFFSET
    pub offset: Option<u32>,
    /// Fusion strategy (for multi-model queries)
    pub fusion: Option<FusionStrategy>,
}

/// Multi-modal statement with explicit components
#[derive(Debug, Clone)]
pub struct MultiModalStatement {
    /// Query components per model
    pub components: HashMap<DataModel, String>,
    /// Fusion strategy
    pub fusion: FusionStrategy,
    /// Post-fusion filters
    pub post_filters: Vec<PostFilter>,
    /// Limit
    pub limit: Option<u32>,
}

/// Data source (FROM clause)
#[derive(Debug, Clone)]
pub struct DataSource {
    /// Model type (vectors, documents, graph, logs, metrics)
    pub model: DataModel,
    /// Collection/namespace name
    pub collection: String,
    /// Alias
    pub alias: Option<String>,
}

/// Join clause
#[derive(Debug, Clone)]
pub struct JoinClause {
    /// Join type
    pub join_type: JoinType,
    /// Joined data source
    pub source: DataSource,
    /// Join condition
    pub condition: JoinCondition,
}

/// Join type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinType {
    /// Inner join
    Inner,
    /// Left outer join
    Left,
    /// Right outer join
    Right,
    /// Full outer join
    Full,
    /// Cross join
    Cross,
    /// Graph-specific join
    Graph,
}

/// Join condition
#[derive(Debug, Clone)]
pub enum JoinCondition {
    /// Column equality (a.id = b.id)
    On {
        /// Left column reference
        left: String,
        /// Right column reference
        right: String,
    },
    /// Graph traversal join
    GraphTraversal {
        /// Starting field for graph traversal
        start_field: String,
        /// Edge type to traverse
        edge_type: String,
        /// Maximum traversal depth
        depth: u32,
    },
    /// Vector similarity join
    VectorSimilarity {
        /// Left vector field
        left_vector: String,
        /// Right vector field
        right_vector: String,
        /// Similarity threshold
        threshold: f32,
    },
}

/// WHERE clause
#[derive(Debug, Clone)]
pub struct WhereClause {
    /// List of conditions in the WHERE clause
    pub conditions: Vec<Condition>,
    /// Logic operator combining the conditions
    pub logic: LogicOperator,
}

/// Condition in WHERE clause
#[derive(Debug, Clone)]
pub enum Condition {
    /// Simple comparison (field = value)
    Comparison {
        /// Field name
        field: String,
        /// Comparison operator
        operator: ComparisonOperator,
        /// Value to compare against
        value: Value,
    },
    /// Vector similarity condition
    VectorSimilar {
        /// Vector field name
        field: String,
        /// Positional parameter index for query vector
        query_param: usize,
        /// Similarity threshold
        threshold: f32,
    },
    /// Vector distance condition
    VectorDistance {
        /// Vector field name
        field: String,
        /// Positional parameter index for query vector
        query_param: usize,
        /// Maximum distance threshold
        max_distance: f32,
    },
    /// Graph connected condition
    GraphConnected {
        /// Start node reference
        start: String,
        /// Edge type to check
        edge_type: String,
        /// End node reference
        end: String,
    },
    /// Graph traversal condition
    GraphTraverse {
        /// Start node reference
        start: String,
        /// Edge types to traverse
        edge_types: Vec<String>,
        /// Maximum depth
        depth: u32,
    },
    /// JSON path condition
    JsonPath {
        /// JSON path expression
        path: String,
        /// Comparison operator
        operator: ComparisonOperator,
        /// Value to compare against
        value: Value,
    },
    /// Exists subquery
    Exists(Box<SelectStatement>),
    /// NOT condition
    Not(Box<Condition>),
    /// Nested conditions with AND/OR
    Nested {
        conditions: Vec<Condition>,
        logic: LogicOperator,
    },
}

/// Comparison operators
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComparisonOperator {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Like,
    In,
    NotIn,
    Between,
    IsNull,
    IsNotNull,
    Contains,
}

/// Logic operators
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicOperator {
    And,
    Or,
}

/// Value in conditions
#[derive(Debug, Clone)]
pub enum Value {
    String(String),
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Null,
    Array(Vec<Value>),
    Param(usize), // Positional parameter (?)
}

/// ORDER BY clause
#[derive(Debug, Clone)]
pub struct OrderByClause {
    pub columns: Vec<(String, SortOrder)>,
}

/// Sort order
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Post-fusion filter
#[derive(Debug, Clone)]
pub struct PostFilter {
    pub field: String,
    pub operator: ComparisonOperator,
    pub value: Value,
}

/// UQL Parser
pub struct UQLParser {
    /// Tokens from lexer
    tokens: Vec<Token>,
    /// Current position
    pos: usize,
}

/// Token types
#[derive(Debug, Clone, PartialEq)]
enum Token {
    // Keywords
    Select,
    From,
    Where,
    Join,
    Inner,
    Left,
    Right,
    Full,
    Cross,
    On,
    And,
    Or,
    Not,
    Order,
    By,
    Asc,
    Desc,
    Limit,
    Offset,
    As,
    Fusion,
    MultiModal,
    Vector,
    Document,
    Graph,
    Logs,
    Metrics,
    Explain,

    // Functions
    VectorSimilar,
    VectorDistance,
    GraphConnected,
    GraphTraverse,
    GraphPath,
    JsonPath,
    Exists,

    // Fusion types
    Rrf,
    Intersection,
    Union,
    Ranked,

    // Operators
    Eq,  // =
    Ne,  // != or <>
    Lt,  // <
    Lte, // <=
    Gt,  // >
    Gte, // >=
    Like,
    In,
    Between,
    Is,
    Null,
    Contains,

    // Literals
    Identifier(String),
    StringLit(String),
    NumberLit(f64),
    IntegerLit(i64),
    Param, // ?

    // Punctuation
    Star,      // *
    Comma,     // ,
    Dot,       // .
    LParen,    // (
    RParen,    // )
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    Colon,     // :
    Semicolon, // ;
    Dollar,    // $

    // Special
    Eof,
}

impl UQLParser {
    /// Create a new UQL parser
    pub fn new() -> Self {
        Self {
            tokens: vec![],
            pos: 0,
        }
    }

    /// Parse a UQL query string into a statement
    pub fn parse(&mut self, query: &str) -> Result<UQLStatement> {
        self.tokens = self.tokenize(query)?;
        self.pos = 0;

        debug!("Parsing UQL: {}", query);

        let stmt = self.parse_statement()?;

        // Ensure we consumed all tokens
        if !self.is_at_end() && self.current() != Token::Semicolon {
            return Err(anyhow!(
                "Unexpected token after statement: {:?}",
                self.current()
            ));
        }

        Ok(stmt)
    }

    /// Parse a UQL query and convert to MultiModelQuery
    pub fn parse_to_multi_model_query(&mut self, query: &str) -> Result<MultiModelQuery> {
        let stmt = self.parse(query)?;
        self.convert_to_multi_model_query(stmt)
    }

    /// Tokenize input string
    fn tokenize(&self, input: &str) -> Result<Vec<Token>> {
        let mut tokens = vec![];
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];

            // Skip whitespace
            if c.is_whitespace() {
                i += 1;
                continue;
            }

            // Skip comments
            if c == '-' && i + 1 < chars.len() && chars[i + 1] == '-' {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }

            // Single character tokens
            match c {
                '*' => {
                    tokens.push(Token::Star);
                    i += 1;
                    continue;
                }
                ',' => {
                    tokens.push(Token::Comma);
                    i += 1;
                    continue;
                }
                '.' => {
                    tokens.push(Token::Dot);
                    i += 1;
                    continue;
                }
                '(' => {
                    tokens.push(Token::LParen);
                    i += 1;
                    continue;
                }
                ')' => {
                    tokens.push(Token::RParen);
                    i += 1;
                    continue;
                }
                '{' => {
                    tokens.push(Token::LBrace);
                    i += 1;
                    continue;
                }
                '}' => {
                    tokens.push(Token::RBrace);
                    i += 1;
                    continue;
                }
                '[' => {
                    tokens.push(Token::LBracket);
                    i += 1;
                    continue;
                }
                ']' => {
                    tokens.push(Token::RBracket);
                    i += 1;
                    continue;
                }
                ':' => {
                    tokens.push(Token::Colon);
                    i += 1;
                    continue;
                }
                ';' => {
                    tokens.push(Token::Semicolon);
                    i += 1;
                    continue;
                }
                '$' => {
                    tokens.push(Token::Dollar);
                    i += 1;
                    continue;
                }
                '?' => {
                    tokens.push(Token::Param);
                    i += 1;
                    continue;
                }
                _ => {}
            }

            // Multi-character operators
            if c == '=' {
                tokens.push(Token::Eq);
                i += 1;
                continue;
            }

            if c == '!' && i + 1 < chars.len() && chars[i + 1] == '=' {
                tokens.push(Token::Ne);
                i += 2;
                continue;
            }

            if c == '<' {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(Token::Lte);
                    i += 2;
                } else if i + 1 < chars.len() && chars[i + 1] == '>' {
                    tokens.push(Token::Ne);
                    i += 2;
                } else {
                    tokens.push(Token::Lt);
                    i += 1;
                }
                continue;
            }

            if c == '>' {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(Token::Gte);
                    i += 2;
                } else {
                    tokens.push(Token::Gt);
                    i += 1;
                }
                continue;
            }

            // String literals
            if c == '\'' || c == '"' {
                let quote = c;
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                let s: String = chars[start..i].iter().collect();
                tokens.push(Token::StringLit(s));
                i += 1; // Skip closing quote
                continue;
            }

            // Numbers
            if c.is_ascii_digit()
                || (c == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
            {
                let start = i;
                if c == '-' {
                    i += 1;
                }
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                if num_str.contains('.') {
                    // TD-007: unwrap_or with safe default - fallback to 0.0 for malformed number tokens
                    // In production, valid number strings are guaranteed by the lexer pattern
                    tokens.push(Token::NumberLit(num_str.parse().unwrap_or(0.0)));
                } else {
                    // TD-007: unwrap_or with safe default - fallback to 0 for malformed integer tokens
                    // In production, valid number strings are guaranteed by the lexer pattern
                    tokens.push(Token::IntegerLit(num_str.parse().unwrap_or(0)));
                }
                continue;
            }

            // Identifiers and keywords
            if c.is_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                let token = match ident.to_uppercase().as_str() {
                    "SELECT" => Token::Select,
                    "FROM" => Token::From,
                    "WHERE" => Token::Where,
                    "JOIN" => Token::Join,
                    "INNER" => Token::Inner,
                    "LEFT" => Token::Left,
                    "RIGHT" => Token::Right,
                    "FULL" => Token::Full,
                    "CROSS" => Token::Cross,
                    "ON" => Token::On,
                    "AND" => Token::And,
                    "OR" => Token::Or,
                    "NOT" => Token::Not,
                    "ORDER" => Token::Order,
                    "BY" => Token::By,
                    "ASC" => Token::Asc,
                    "DESC" => Token::Desc,
                    "LIMIT" => Token::Limit,
                    "OFFSET" => Token::Offset,
                    "AS" => Token::As,
                    "FUSION" => Token::Fusion,
                    "MULTIMODAL" => Token::MultiModal,
                    "VECTOR" => Token::Vector,
                    "VECTORS" => Token::Vector,
                    "DOCUMENT" => Token::Document,
                    "DOCUMENTS" => Token::Document,
                    "DOCS" => Token::Document,
                    "GRAPH" => Token::Graph,
                    "LOGS" => Token::Logs,
                    "METRICS" => Token::Metrics,
                    "EXPLAIN" => Token::Explain,
                    "VECTOR_SIMILAR" => Token::VectorSimilar,
                    "SIMILAR" => Token::VectorSimilar,
                    "VECTOR_DISTANCE" => Token::VectorDistance,
                    "DISTANCE" => Token::VectorDistance,
                    "GRAPH_CONNECTED" => Token::GraphConnected,
                    "CONNECTED" => Token::GraphConnected,
                    "GRAPH_TRAVERSE" => Token::GraphTraverse,
                    "TRAVERSE" => Token::GraphTraverse,
                    "GRAPH_PATH" => Token::GraphPath,
                    "PATH" => Token::GraphPath,
                    "JSON_PATH" => Token::JsonPath,
                    "EXISTS" => Token::Exists,
                    "RRF" => Token::Rrf,
                    "INTERSECTION" => Token::Intersection,
                    "UNION" => Token::Union,
                    "RANKED" => Token::Ranked,
                    "LIKE" => Token::Like,
                    "IN" => Token::In,
                    "BETWEEN" => Token::Between,
                    "IS" => Token::Is,
                    "NULL" => Token::Null,
                    "CONTAINS" => Token::Contains,
                    _ => Token::Identifier(ident),
                };
                tokens.push(token);
                continue;
            }

            return Err(anyhow!("Unexpected character: '{}'", c));
        }

        tokens.push(Token::Eof);
        Ok(tokens)
    }

    fn current(&self) -> Token {
        // TD-007: unwrap_or with safe default - Token::Eof is correct fallback for parser position
        // When pos exceeds tokens length, we're at end of input, so Eof is the correct token
        self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let token = self.current();
        self.pos += 1;
        token
    }

    fn is_at_end(&self) -> bool {
        self.current() == Token::Eof
    }

    fn expect(&mut self, expected: Token) -> Result<()> {
        if self.current() == expected {
            self.advance();
            Ok(())
        } else {
            Err(anyhow!("Expected {:?}, got {:?}", expected, self.current()))
        }
    }

    fn parse_statement(&mut self) -> Result<UQLStatement> {
        match self.current() {
            Token::Explain => {
                self.advance();
                let inner = self.parse_statement()?;
                Ok(UQLStatement::Explain(Box::new(inner)))
            }
            Token::Select => {
                let select = self.parse_select()?;
                Ok(UQLStatement::Select(select))
            }
            Token::MultiModal => {
                let mm = self.parse_multimodal()?;
                Ok(UQLStatement::MultiModal(mm))
            }
            _ => Err(anyhow!(
                "Expected SELECT or MULTIMODAL, got {:?}",
                self.current()
            )),
        }
    }

    fn parse_select(&mut self) -> Result<SelectStatement> {
        self.expect(Token::Select)?;

        // Parse columns
        let columns = self.parse_columns()?;

        // Parse FROM
        self.expect(Token::From)?;
        let from = self.parse_data_source()?;

        // Parse optional JOINs
        let mut joins = vec![];
        while matches!(
            self.current(),
            Token::Join | Token::Inner | Token::Left | Token::Right | Token::Full | Token::Cross
        ) {
            joins.push(self.parse_join()?);
        }

        // Parse optional WHERE
        let where_clause = if self.current() == Token::Where {
            self.advance();
            Some(self.parse_where()?)
        } else {
            None
        };

        // Parse optional ORDER BY
        let order_by = if self.current() == Token::Order {
            self.advance();
            self.expect(Token::By)?;
            Some(self.parse_order_by()?)
        } else {
            None
        };

        // Parse optional LIMIT
        let limit = if self.current() == Token::Limit {
            self.advance();
            if let Token::IntegerLit(n) = self.advance() {
                Some(n as u32)
            } else {
                return Err(anyhow!("Expected number after LIMIT"));
            }
        } else {
            None
        };

        // Parse optional OFFSET
        let offset = if self.current() == Token::Offset {
            self.advance();
            if let Token::IntegerLit(n) = self.advance() {
                Some(n as u32)
            } else {
                return Err(anyhow!("Expected number after OFFSET"));
            }
        } else {
            None
        };

        // Parse optional FUSION
        let fusion = if self.current() == Token::Fusion {
            self.advance();
            Some(self.parse_fusion_strategy()?)
        } else {
            None
        };

        Ok(SelectStatement {
            columns,
            from,
            joins,
            where_clause,
            order_by,
            limit,
            offset,
            fusion,
        })
    }

    fn parse_columns(&mut self) -> Result<Vec<String>> {
        let mut columns = vec![];

        if self.current() == Token::Star {
            self.advance();
            columns.push("*".to_string());
        } else {
            loop {
                let col = self.parse_column_expr()?;
                columns.push(col);

                if self.current() != Token::Comma {
                    break;
                }
                self.advance(); // Skip comma
            }
        }

        Ok(columns)
    }

    fn parse_column_expr(&mut self) -> Result<String> {
        let mut expr = String::new();

        // Handle table.column or just column
        if let Token::Identifier(name) = self.current() {
            expr.push_str(&name);
            self.advance();

            if self.current() == Token::Dot {
                self.advance();
                if let Token::Identifier(col) = self.advance() {
                    expr.push('.');
                    expr.push_str(&col);
                } else if self.current() == Token::Star {
                    self.advance();
                    expr.push_str(".*");
                }
            }

            // Handle alias
            if self.current() == Token::As {
                self.advance();
                if let Token::Identifier(alias) = self.advance() {
                    expr.push_str(" AS ");
                    expr.push_str(&alias);
                }
            }
        }

        Ok(expr)
    }

    fn parse_data_source(&mut self) -> Result<DataSource> {
        // Parse model.collection or just collection
        let (model, collection) = if let Token::Identifier(first) = self.current() {
            self.advance();

            if self.current() == Token::Dot {
                self.advance();
                if let Token::Identifier(second) = self.advance() {
                    let model = self.string_to_model(&first)?;
                    (model, second)
                } else {
                    return Err(anyhow!("Expected collection name after dot"));
                }
            } else {
                // No model prefix, infer from context
                (DataModel::Vector, first.to_string())
            }
        } else if matches!(
            self.current(),
            Token::Vector | Token::Document | Token::Graph | Token::Logs | Token::Metrics
        ) {
            let model = match self.advance() {
                Token::Vector => DataModel::Vector,
                Token::Document => DataModel::Document,
                Token::Graph => DataModel::Graph,
                Token::Logs | Token::Metrics => DataModel::Observability,
                _ => DataModel::Vector,
            };

            self.expect(Token::Dot)?;

            if let Token::Identifier(collection) = self.advance() {
                (model, collection)
            } else {
                return Err(anyhow!("Expected collection name"));
            }
        } else {
            return Err(anyhow!("Expected data source"));
        };

        // Parse optional alias
        let alias = if self.current() == Token::As {
            self.advance();
            if let Token::Identifier(a) = self.advance() {
                Some(a)
            } else {
                None
            }
        } else if let Token::Identifier(a) = self.current() {
            // Alias without AS
            if !matches!(
                self.current(),
                Token::Where | Token::Join | Token::Order | Token::Limit | Token::Fusion
            ) {
                self.advance();
                Some(a)
            } else {
                None
            }
        } else {
            None
        };

        Ok(DataSource {
            model,
            collection,
            alias,
        })
    }

    fn parse_join(&mut self) -> Result<JoinClause> {
        // Parse join type
        let join_type = match self.current() {
            Token::Inner => {
                self.advance();
                self.expect(Token::Join)?;
                JoinType::Inner
            }
            Token::Left => {
                self.advance();
                self.expect(Token::Join)?;
                JoinType::Left
            }
            Token::Right => {
                self.advance();
                self.expect(Token::Join)?;
                JoinType::Right
            }
            Token::Full => {
                self.advance();
                self.expect(Token::Join)?;
                JoinType::Full
            }
            Token::Cross => {
                self.advance();
                self.expect(Token::Join)?;
                JoinType::Cross
            }
            Token::Join => {
                self.advance();
                JoinType::Inner
            }
            Token::Graph => {
                self.advance();
                JoinType::Graph
            }
            _ => return Err(anyhow!("Expected JOIN keyword")),
        };

        let source = self.parse_data_source()?;

        // Parse ON condition
        self.expect(Token::On)?;
        let condition = self.parse_join_condition()?;

        Ok(JoinClause {
            join_type,
            source,
            condition,
        })
    }

    fn parse_join_condition(&mut self) -> Result<JoinCondition> {
        // Check for special join conditions
        if self.current() == Token::GraphConnected || self.current() == Token::GraphTraverse {
            return self.parse_graph_join_condition();
        }

        // Standard ON a.id = b.id
        let left = self.parse_column_expr()?;
        self.expect(Token::Eq)?;
        let right = self.parse_column_expr()?;

        Ok(JoinCondition::On { left, right })
    }

    fn parse_graph_join_condition(&mut self) -> Result<JoinCondition> {
        match self.advance() {
            Token::GraphConnected | Token::GraphTraverse => {
                self.expect(Token::LParen)?;
                let start_field = self.parse_column_expr()?;
                self.expect(Token::Comma)?;
                let edge_type = if let Token::StringLit(s) = self.advance() {
                    s
                } else {
                    return Err(anyhow!("Expected edge type string"));
                };
                self.expect(Token::Comma)?;
                let depth = if let Token::IntegerLit(d) = self.advance() {
                    d as u32
                } else {
                    1
                };
                self.expect(Token::RParen)?;

                Ok(JoinCondition::GraphTraversal {
                    start_field,
                    edge_type,
                    depth,
                })
            }
            _ => Err(anyhow!("Expected GRAPH_CONNECTED or GRAPH_TRAVERSE")),
        }
    }

    fn parse_where(&mut self) -> Result<WhereClause> {
        let mut conditions = vec![];
        let mut logic = LogicOperator::And;

        loop {
            let condition = self.parse_condition()?;
            conditions.push(condition);

            match self.current() {
                Token::And => {
                    self.advance();
                    logic = LogicOperator::And;
                }
                Token::Or => {
                    self.advance();
                    logic = LogicOperator::Or;
                }
                _ => break,
            }
        }

        Ok(WhereClause { conditions, logic })
    }

    fn parse_condition(&mut self) -> Result<Condition> {
        // Check for NOT
        if self.current() == Token::Not {
            self.advance();
            let inner = self.parse_condition()?;
            return Ok(Condition::Not(Box::new(inner)));
        }

        // Check for EXISTS
        if self.current() == Token::Exists {
            self.advance();
            self.expect(Token::LParen)?;
            let subquery = self.parse_select()?;
            self.expect(Token::RParen)?;
            return Ok(Condition::Exists(Box::new(subquery)));
        }

        // Check for parenthesized conditions
        if self.current() == Token::LParen {
            self.advance();
            let where_clause = self.parse_where()?;
            self.expect(Token::RParen)?;
            return Ok(Condition::Nested {
                conditions: where_clause.conditions,
                logic: where_clause.logic,
            });
        }

        // Check for vector/graph functions
        match self.current() {
            Token::VectorSimilar => return self.parse_vector_similar_condition(),
            Token::VectorDistance => return self.parse_vector_distance_condition(),
            Token::GraphConnected => return self.parse_graph_connected_condition(),
            Token::GraphTraverse => return self.parse_graph_traverse_condition(),
            _ => {}
        }

        // Check for JSON path (starts with $)
        if self.current() == Token::Dollar {
            return self.parse_json_path_condition();
        }

        // Standard comparison
        let field = self.parse_column_expr()?;
        let operator = self.parse_comparison_operator()?;
        let value = self.parse_value()?;

        Ok(Condition::Comparison {
            field,
            operator,
            value,
        })
    }

    fn parse_vector_similar_condition(&mut self) -> Result<Condition> {
        self.advance(); // Skip VECTOR_SIMILAR
        self.expect(Token::LParen)?;

        let field = self.parse_column_expr()?;
        self.expect(Token::Comma)?;

        // Parse query parameter (? or specific param)
        let query_param = if self.current() == Token::Param {
            self.advance();
            0 // First positional parameter
        } else if let Token::IntegerLit(n) = self.current() {
            self.advance();
            n as usize
        } else {
            0
        };

        self.expect(Token::Comma)?;

        // Parse threshold
        let threshold = if let Token::NumberLit(t) = self.advance() {
            t as f32
        } else {
            0.8
        };

        self.expect(Token::RParen)?;

        Ok(Condition::VectorSimilar {
            field,
            query_param,
            threshold,
        })
    }

    fn parse_vector_distance_condition(&mut self) -> Result<Condition> {
        self.advance(); // Skip VECTOR_DISTANCE
        self.expect(Token::LParen)?;

        let field = self.parse_column_expr()?;
        self.expect(Token::Comma)?;

        let query_param = if self.current() == Token::Param {
            self.advance();
            0
        } else {
            0
        };

        self.expect(Token::RParen)?;

        // Parse comparison
        let max_distance = if matches!(self.current(), Token::Lt | Token::Lte) {
            self.advance();
            if let Token::NumberLit(d) = self.advance() {
                d as f32
            } else {
                1.0
            }
        } else {
            1.0
        };

        Ok(Condition::VectorDistance {
            field,
            query_param,
            max_distance,
        })
    }

    fn parse_graph_connected_condition(&mut self) -> Result<Condition> {
        self.advance(); // Skip GRAPH_CONNECTED
        self.expect(Token::LParen)?;

        let start = self.parse_column_expr()?;
        self.expect(Token::Comma)?;

        let edge_type = if let Token::StringLit(s) = self.advance() {
            s
        } else {
            "RELATED_TO".to_string()
        };

        self.expect(Token::Comma)?;

        let end = self.parse_column_expr()?;
        self.expect(Token::RParen)?;

        Ok(Condition::GraphConnected {
            start,
            edge_type,
            end,
        })
    }

    fn parse_graph_traverse_condition(&mut self) -> Result<Condition> {
        self.advance(); // Skip GRAPH_TRAVERSE
        self.expect(Token::LParen)?;

        let start = self.parse_column_expr()?;
        self.expect(Token::Comma)?;

        // Parse edge types (comma-separated strings or array)
        let mut edge_types = vec![];
        while let Token::StringLit(s) = self.advance() {
            edge_types.push(s);
            if self.current() != Token::Comma {
                break;
            }
            self.advance();
        }

        self.expect(Token::Comma)?;

        let depth = if let Token::IntegerLit(d) = self.advance() {
            d as u32
        } else {
            2
        };

        self.expect(Token::RParen)?;

        Ok(Condition::GraphTraverse {
            start,
            edge_types,
            depth,
        })
    }

    fn parse_json_path_condition(&mut self) -> Result<Condition> {
        self.advance(); // Skip $

        // Parse the path
        let mut path = String::from("$");

        // Parse path segments
        while self.current() == Token::Dot || self.current() == Token::LBracket {
            if self.current() == Token::Dot {
                self.advance();
                if let Token::Identifier(name) = self.advance() {
                    path.push('.');
                    path.push_str(&name);
                }
            } else if self.current() == Token::LBracket {
                self.advance();
                if let Token::IntegerLit(i) = self.advance() {
                    path.push_str(&format!("[{}]", i));
                } else if let Token::StringLit(s) = self.current() {
                    self.advance();
                    path.push_str(&format!("['{}']", s));
                }
                self.expect(Token::RBracket)?;
            }
        }

        let operator = self.parse_comparison_operator()?;
        let value = self.parse_value()?;

        Ok(Condition::JsonPath {
            path,
            operator,
            value,
        })
    }

    fn parse_comparison_operator(&mut self) -> Result<ComparisonOperator> {
        let op = match self.advance() {
            Token::Eq => ComparisonOperator::Eq,
            Token::Ne => ComparisonOperator::Ne,
            Token::Lt => ComparisonOperator::Lt,
            Token::Lte => ComparisonOperator::Lte,
            Token::Gt => ComparisonOperator::Gt,
            Token::Gte => ComparisonOperator::Gte,
            Token::Like => ComparisonOperator::Like,
            Token::In => ComparisonOperator::In,
            Token::Between => ComparisonOperator::Between,
            Token::Contains => ComparisonOperator::Contains,
            Token::Is => {
                if self.current() == Token::Not {
                    self.advance();
                    self.expect(Token::Null)?;
                    ComparisonOperator::IsNotNull
                } else {
                    self.expect(Token::Null)?;
                    ComparisonOperator::IsNull
                }
            }
            Token::Not => {
                self.expect(Token::In)?;
                ComparisonOperator::NotIn
            }
            t => return Err(anyhow!("Expected comparison operator, got {:?}", t)),
        };
        Ok(op)
    }

    fn parse_value(&mut self) -> Result<Value> {
        match self.advance() {
            Token::StringLit(s) => Ok(Value::String(s)),
            Token::NumberLit(n) => Ok(Value::Number(n)),
            Token::IntegerLit(i) => Ok(Value::Integer(i)),
            Token::Null => Ok(Value::Null),
            Token::Param => Ok(Value::Param(0)),
            Token::LParen => {
                // Parse array of values
                let mut values = vec![];
                loop {
                    values.push(self.parse_value()?);
                    if self.current() != Token::Comma {
                        break;
                    }
                    self.advance();
                }
                self.expect(Token::RParen)?;
                Ok(Value::Array(values))
            }
            Token::Identifier(s) => {
                if s.eq_ignore_ascii_case("true") {
                    Ok(Value::Boolean(true))
                } else if s.eq_ignore_ascii_case("false") {
                    Ok(Value::Boolean(false))
                } else {
                    Ok(Value::String(s))
                }
            }
            t => Err(anyhow!("Expected value, got {:?}", t)),
        }
    }

    fn parse_order_by(&mut self) -> Result<OrderByClause> {
        let mut columns = vec![];

        loop {
            let col = self.parse_column_expr()?;
            let order = if self.current() == Token::Desc {
                self.advance();
                SortOrder::Desc
            } else {
                if self.current() == Token::Asc {
                    self.advance();
                }
                SortOrder::Asc
            };
            columns.push((col, order));

            if self.current() != Token::Comma {
                break;
            }
            self.advance();
        }

        Ok(OrderByClause { columns })
    }

    fn parse_fusion_strategy(&mut self) -> Result<FusionStrategy> {
        match self.current() {
            Token::Rrf => {
                self.advance();
                let k = if self.current() == Token::LParen {
                    self.advance();
                    let k = if let Token::IntegerLit(n) = self.advance() {
                        n as u32
                    } else {
                        60
                    };
                    self.expect(Token::RParen)?;
                    k
                } else {
                    60
                };
                Ok(FusionStrategy::ReciprocalRankFusion { k })
            }
            Token::Intersection => {
                self.advance();
                Ok(FusionStrategy::Intersection)
            }
            Token::Union => {
                self.advance();
                Ok(FusionStrategy::Union)
            }
            Token::Ranked => {
                self.advance();
                Ok(FusionStrategy::RankedFusion {
                    weights: HashMap::new(),
                    normalize: true,
                })
            }
            Token::Identifier(name) => {
                self.advance();
                Ok(FusionStrategy::Custom(name))
            }
            _ => Ok(FusionStrategy::Intersection),
        }
    }

    fn parse_multimodal(&mut self) -> Result<MultiModalStatement> {
        self.expect(Token::MultiModal)?;
        self.expect(Token::LBrace)?;

        let mut components = HashMap::new();

        loop {
            // Parse model: query pairs
            let model = match self.current() {
                Token::Vector => {
                    self.advance();
                    DataModel::Vector
                }
                Token::Document => {
                    self.advance();
                    DataModel::Document
                }
                Token::Graph => {
                    self.advance();
                    DataModel::Graph
                }
                Token::Logs | Token::Metrics => {
                    self.advance();
                    DataModel::Observability
                }
                _ => break,
            };

            self.expect(Token::Colon)?;

            // Collect query string until comma or closing brace
            let mut query_tokens = vec![];
            let mut depth = 0;
            while !self.is_at_end() {
                match self.current() {
                    Token::Comma if depth == 0 => break,
                    Token::RBrace if depth == 0 => break,
                    Token::LParen | Token::LBrace | Token::LBracket => {
                        depth += 1;
                        query_tokens.push(self.advance());
                    }
                    Token::RParen | Token::RBrace | Token::RBracket => {
                        depth -= 1;
                        query_tokens.push(self.advance());
                    }
                    _ => query_tokens.push(self.advance()),
                }
            }

            components.insert(model, format!("{:?}", query_tokens));

            if self.current() == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }

        self.expect(Token::RBrace)?;

        // Parse FUSION
        let fusion = if self.current() == Token::Fusion {
            self.advance();
            self.parse_fusion_strategy()?
        } else {
            FusionStrategy::Intersection
        };

        // Parse optional LIMIT
        let limit = if self.current() == Token::Limit {
            self.advance();
            if let Token::IntegerLit(n) = self.advance() {
                Some(n as u32)
            } else {
                None
            }
        } else {
            None
        };

        Ok(MultiModalStatement {
            components,
            fusion,
            post_filters: vec![],
            limit,
        })
    }

    fn string_to_model(&self, s: &str) -> Result<DataModel> {
        match s.to_lowercase().as_str() {
            "vectors" | "vector" | "v" => Ok(DataModel::Vector),
            "documents" | "document" | "docs" | "doc" | "d" => Ok(DataModel::Document),
            "graph" | "graphs" | "g" => Ok(DataModel::Graph),
            "logs" | "metrics" | "traces" | "observability" | "o" => Ok(DataModel::Observability),
            _ => Err(anyhow!("Unknown data model: {}", s)),
        }
    }

    /// Convert UQL statement to MultiModelQuery
    fn convert_to_multi_model_query(&self, stmt: UQLStatement) -> Result<MultiModelQuery> {
        match stmt {
            UQLStatement::Select(select) => self.convert_select_to_query(select),
            UQLStatement::MultiModal(mm) => self.convert_multimodal_to_query(mm),
            UQLStatement::Explain(inner) => self.convert_to_multi_model_query(*inner),
        }
    }

    fn convert_select_to_query(&self, select: SelectStatement) -> Result<MultiModelQuery> {
        let mut query = MultiModelQuery::new();

        // Convert primary data source to component
        let primary_component =
            self.data_source_to_component(&select.from, &select.where_clause)?;
        query.components.push(primary_component);

        // Convert joins to additional components
        for join in &select.joins {
            let join_component = self.join_to_component(join)?;
            query.components.push(join_component);
        }

        // Set fusion strategy
        // TD-007: unwrap_or with safe default - Intersection is sensible default for multi-model queries
        query.fusion_strategy = select.fusion.unwrap_or(FusionStrategy::Intersection);

        // Set limit
        query.limit = select.limit;
        query.offset = select.offset;

        Ok(query)
    }

    fn convert_multimodal_to_query(&self, mm: MultiModalStatement) -> Result<MultiModelQuery> {
        let mut query = MultiModelQuery::new();
        query.fusion_strategy = mm.fusion;
        query.limit = mm.limit;

        // For now, create placeholder components
        // Full implementation would parse each component's query string

        Ok(query)
    }

    fn data_source_to_component(
        &self,
        source: &DataSource,
        where_clause: &Option<WhereClause>,
    ) -> Result<QueryComponent> {
        let operation = match source.model {
            DataModel::Vector => {
                // Extract vector search from WHERE clause if present
                let (query_vector, threshold) = if let Some(wc) = where_clause {
                    self.extract_vector_search_params(wc)?
                } else {
                    (vec![0.0; 384], 0.8)
                };

                ModelOperation::VectorSearch(VectorSearchExpr {
                    collection: source.collection.clone(),
                    query_vector,
                    top_k: 100,
                    threshold: Some(threshold),
                    metric: DistanceMetric::Cosine,
                    params: VectorSearchParams::default(),
                })
            }
            DataModel::Document => {
                let path_filters = if let Some(wc) = where_clause {
                    self.extract_path_filters(wc)?
                } else {
                    vec![]
                };

                ModelOperation::DocumentQuery(DocumentQueryExpr {
                    collection: source.collection.clone(),
                    path_filters,
                    text_search: None,
                    projection: vec![],
                    sort: None,
                    limit: Some(100),
                })
            }
            DataModel::Graph => ModelOperation::GraphTraversal(GraphTraversalExpr {
                graph_name: source.collection.clone(),
                start_nodes: StartNodeSpec::Ids(vec![]),
                edge_types: vec![],
                direction: TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: vec![],
                edge_filters: vec![],
                return_paths: false,
            }),
            DataModel::Observability
            | DataModel::TimeSeries
            | DataModel::Relational
            | DataModel::Event => ModelOperation::LogQuery(LogQueryExpr {
                namespace: source.collection.clone(),
                start_time_ns: 0,
                end_time_ns: i64::MAX,
                query: None,
                severities: vec![],
                services: vec![],
                limit: 100,
            }),
        };

        Ok(QueryComponent {
            model: source.model.clone(),
            operation,
            filters: vec![],
            dependencies: vec![],
        })
    }

    fn join_to_component(&self, join: &JoinClause) -> Result<QueryComponent> {
        self.data_source_to_component(&join.source, &None)
    }

    fn extract_vector_search_params(&self, _wc: &WhereClause) -> Result<(Vec<f32>, f32)> {
        // Placeholder - would extract from VECTOR_SIMILAR conditions
        Ok((vec![0.0; 384], 0.8))
    }

    fn extract_path_filters(&self, wc: &WhereClause) -> Result<Vec<PathFilter>> {
        let mut filters = vec![];

        for condition in &wc.conditions {
            if let Condition::JsonPath {
                path,
                operator,
                value,
            } = condition
            {
                filters.push(PathFilter {
                    path: path.clone(),
                    operator: self.convert_operator(operator),
                    value: self.convert_value(value),
                });
            }
        }

        Ok(filters)
    }

    fn convert_operator(&self, op: &ComparisonOperator) -> FilterOperator {
        match op {
            ComparisonOperator::Eq => FilterOperator::Eq,
            ComparisonOperator::Ne => FilterOperator::Ne,
            ComparisonOperator::Lt => FilterOperator::Lt,
            ComparisonOperator::Lte => FilterOperator::Lte,
            ComparisonOperator::Gt => FilterOperator::Gt,
            ComparisonOperator::Gte => FilterOperator::Gte,
            ComparisonOperator::In => FilterOperator::In,
            ComparisonOperator::NotIn => FilterOperator::NotIn,
            ComparisonOperator::Contains => FilterOperator::Contains,
            ComparisonOperator::Like => FilterOperator::Contains,
            _ => FilterOperator::Eq,
        }
    }

    fn convert_value(&self, value: &Value) -> FilterValue {
        match value {
            Value::String(s) => FilterValue::String(s.clone()),
            Value::Number(n) => FilterValue::Number(*n),
            Value::Integer(i) => FilterValue::Number(*i as f64),
            Value::Boolean(b) => FilterValue::Bool(*b),
            Value::Null => FilterValue::Null,
            Value::Array(arr) => {
                FilterValue::Array(arr.iter().map(|v| self.convert_value(v)).collect())
            }
            Value::Param(_) => FilterValue::Null, // Placeholder
        }
    }
}

impl Default for UQLParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple_select() -> Result<()> {
        let parser = UQLParser::new();
        let tokens = parser.tokenize("SELECT * FROM vectors.products")?;

        assert!(matches!(tokens[0], Token::Select));
        assert!(matches!(tokens[1], Token::Star));
        assert!(matches!(tokens[2], Token::From));
        assert!(matches!(tokens[3], Token::Vector));
        Ok(())
    }

    #[test]
    fn test_parse_simple_select() -> Result<()> {
        let mut parser = UQLParser::new();
        let result = parser.parse("SELECT * FROM vectors.products LIMIT 10")?;

        if let UQLStatement::Select(select) = result {
            assert_eq!(select.columns, vec!["*"]);
            assert_eq!(select.from.collection, "products");
            assert_eq!(select.limit, Some(10));
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }

    #[test]
    fn test_parse_where_clause() -> Result<()> {
        let mut parser = UQLParser::new();
        let result =
            parser.parse("SELECT * FROM docs.products WHERE $.category = 'electronics'")?;

        if let UQLStatement::Select(select) = result {
            assert!(select.where_clause.is_some());
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }

    #[test]
    fn test_parse_vector_similar() -> Result<()> {
        let mut parser = UQLParser::new();
        let result = parser
            .parse("SELECT * FROM vectors.products WHERE VECTOR_SIMILAR(embedding, ?, 0.8)")?;

        if let UQLStatement::Select(select) = result {
            let wc = select
                .where_clause
                .ok_or_else(|| anyhow!("Expected where clause"))?;
            assert!(!wc.conditions.is_empty());
            if let Condition::VectorSimilar { threshold, .. } = &wc.conditions[0] {
                assert_eq!(*threshold, 0.8);
            }
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }

    #[test]
    fn test_parse_join() -> Result<()> {
        let mut parser = UQLParser::new();
        let result = parser.parse(
            "SELECT v.*, d.title FROM vectors.items v \
             JOIN documents.metadata d ON v.id = d.item_id",
        )?;

        if let UQLStatement::Select(select) = result {
            assert_eq!(select.joins.len(), 1);
            assert_eq!(select.joins[0].source.collection, "metadata");
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }

    #[test]
    fn test_parse_fusion() -> Result<()> {
        let mut parser = UQLParser::new();
        let result = parser.parse("SELECT * FROM vectors.products FUSION RRF(60)")?;

        if let UQLStatement::Select(select) = result {
            if let Some(FusionStrategy::ReciprocalRankFusion { k }) = select.fusion {
                assert_eq!(k, 60);
            } else {
                return Err(anyhow!("Expected RRF fusion"));
            }
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }

    #[test]
    fn test_parse_explain() -> Result<()> {
        let mut parser = UQLParser::new();
        let result = parser.parse("EXPLAIN SELECT * FROM vectors.products")?;

        assert!(matches!(result, UQLStatement::Explain(_)));
        Ok(())
    }

    #[test]
    fn test_parse_order_by() -> Result<()> {
        let mut parser = UQLParser::new();
        let result =
            parser.parse("SELECT * FROM vectors.products ORDER BY score DESC, name ASC")?;

        if let UQLStatement::Select(select) = result {
            let order = select
                .order_by
                .ok_or_else(|| anyhow!("Expected order_by clause"))?;
            assert_eq!(order.columns.len(), 2);
            assert_eq!(order.columns[0].1, SortOrder::Desc);
            assert_eq!(order.columns[1].1, SortOrder::Asc);
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }

    #[test]
    fn test_convert_to_multi_model_query() -> Result<()> {
        let mut parser = UQLParser::new();
        let query = parser.parse_to_multi_model_query(
            "SELECT * FROM vectors.products WHERE VECTOR_SIMILAR(embedding, ?, 0.8) LIMIT 10",
        )?;

        assert_eq!(query.components.len(), 1);
        assert_eq!(query.limit, Some(10));
        Ok(())
    }

    #[test]
    fn test_parse_complex_query() -> Result<()> {
        let mut parser = UQLParser::new();
        let result = parser.parse(
            "SELECT v.id, v.score, d.title \
             FROM vectors.embeddings v \
             JOIN documents.metadata d ON v.id = d.embedding_id \
             WHERE VECTOR_SIMILAR(v.embedding, ?, 0.7) \
               AND $.status = 'active' \
             ORDER BY v.score DESC \
             LIMIT 50 \
             FUSION INTERSECTION",
        )?;

        if let UQLStatement::Select(select) = result {
            assert_eq!(select.joins.len(), 1);
            assert!(select.where_clause.is_some());
            assert_eq!(select.limit, Some(50));
            assert!(matches!(select.fusion, Some(FusionStrategy::Intersection)));
        } else {
            return Err(anyhow!("Expected SELECT statement"));
        }
        Ok(())
    }
}
