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

//! # Cypher AST Types
//!
//! Defines the Abstract Syntax Tree for the Cypher query language.
//! These types are produced by the recursive-descent parser in `cypher_parser`
//! and consumed by the query planner and executor.

/// Top-level Cypher statement consisting of a sequence of clauses.
#[derive(Debug, Clone)]
pub struct CypherStatement {
    pub clauses: Vec<CypherClause>,
}

/// A single clause within a Cypher statement.
#[derive(Debug, Clone)]
pub enum CypherClause {
    Match(MatchClause),
    OptionalMatch(MatchClause),
    Where(WhereClause),
    Return(ReturnClause),
    OrderBy(OrderByClause),
    Limit(LimitClause),
    Skip(SkipClause),
    Create(CreateClause),
    Set(SetClause),
    Delete(DeleteClause),
    With(WithClause),
    Union(UnionClause),
    Unwind(UnwindClause),
}

/// MATCH or OPTIONAL MATCH clause containing one or more pattern paths.
#[derive(Debug, Clone)]
pub struct MatchClause {
    pub patterns: Vec<PatternPath>,
}

/// A single pattern path: a sequence of alternating nodes and relationships.
#[derive(Debug, Clone)]
pub struct PatternPath {
    pub elements: Vec<PatternElement>,
}

/// An element in a pattern path — either a node or a relationship.
#[derive(Debug, Clone)]
pub enum PatternElement {
    Node(NodePattern),
    Relationship(RelPattern),
}

/// A node pattern: `(variable:Label1:Label2 {key: value, ...})`
#[derive(Debug, Clone)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Vec<(String, CypherValue)>,
}

/// A relationship pattern: `-[variable:TYPE1|TYPE2 {key: value} *min..max]->`
#[derive(Debug, Clone)]
pub struct RelPattern {
    pub variable: Option<String>,
    pub rel_types: Vec<String>,
    pub direction: Direction,
    pub properties: Vec<(String, CypherValue)>,
    /// Variable-length path range: `*min..max`. `None` means single hop.
    pub range: Option<(Option<u32>, Option<u32>)>,
}

/// Direction of a relationship in a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    /// `<-[]-`
    Left,
    /// `-[]->`
    Right,
    /// `-[]-`
    Both,
}

/// WHERE clause.
#[derive(Debug, Clone)]
pub struct WhereClause {
    pub expression: Expression,
}

/// RETURN clause with optional DISTINCT.
#[derive(Debug, Clone)]
pub struct ReturnClause {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
}

/// A single item in a RETURN or WITH clause.
#[derive(Debug, Clone)]
pub struct ReturnItem {
    pub expression: Expression,
    pub alias: Option<String>,
}

/// ORDER BY clause.
#[derive(Debug, Clone)]
pub struct OrderByClause {
    pub items: Vec<(Expression, SortDirection)>,
}

/// Sort direction for ORDER BY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// LIMIT clause.
#[derive(Debug, Clone)]
pub struct LimitClause {
    pub count: u64,
}

/// SKIP clause.
#[derive(Debug, Clone)]
pub struct SkipClause {
    pub count: u64,
}

/// CREATE clause.
#[derive(Debug, Clone)]
pub struct CreateClause {
    pub patterns: Vec<PatternPath>,
}

/// SET clause.
#[derive(Debug, Clone)]
pub struct SetClause {
    pub items: Vec<SetItem>,
}

/// A single SET operation.
#[derive(Debug, Clone)]
pub enum SetItem {
    Property {
        variable: String,
        property: String,
        value: Expression,
    },
}

/// DELETE clause with optional DETACH.
#[derive(Debug, Clone)]
pub struct DeleteClause {
    pub detach: bool,
    pub expressions: Vec<Expression>,
}

/// WITH clause (projection between clauses).
#[derive(Debug, Clone)]
pub struct WithClause {
    pub items: Vec<ReturnItem>,
    pub where_clause: Option<WhereClause>,
}

/// UNION clause.
#[derive(Debug, Clone)]
pub struct UnionClause {
    pub all: bool,
}

/// UNWIND clause for list expansion.
#[derive(Debug, Clone)]
pub struct UnwindClause {
    /// Expression that evaluates to a list
    pub expression: Expression,
    /// Variable name to bind to each element
    pub variable: String,
}

/// An expression in the Cypher language.
#[derive(Debug, Clone)]
pub enum Expression {
    Literal(CypherValue),
    Variable(String),
    Property(Box<Expression>, String),
    BinaryOp(Box<Expression>, BinaryOperator, Box<Expression>),
    UnaryOp(UnaryOperator, Box<Expression>),
    FunctionCall(String, Vec<Expression>),
    Parameter(String),
    List(Vec<Expression>),
    Comparison(Box<Expression>, CompOp, Box<Expression>),
    /// REDUCE expression for list aggregation
    Reduce {
        /// Variable name for the accumulator
        accumulator: String,
        /// Initial value for the accumulator
        initial: Box<Expression>,
        /// Variable name for list elements
        variable: String,
        /// List expression to iterate over
        list: Box<Expression>,
        /// Expression to evaluate for each element
        update: Box<Expression>,
    },
    /// List comprehension: [x IN list WHERE x > 5 | x * 2]
    ListComprehension {
        /// Variable name for list elements
        variable: String,
        /// List expression to iterate over
        list: Box<Expression>,
        /// Optional filter predicate (WHERE clause)
        filter: Option<Box<Expression>>,
        /// Optional transformation expression
        projection: Option<Box<Expression>>,
    },
    /// Pattern comprehension: [ (a)-->(b) WHERE a.name = 'Alice' | b.name ]
    PatternComprehension {
        /// Pattern to match
        pattern: PatternPath,
        /// Optional filter predicate (WHERE clause)
        filter: Option<Box<Expression>>,
        /// Projection expression
        projection: Box<Expression>,
    },
}

/// Binary operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOperator {
    And,
    Or,
    Xor,
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
}

/// Unary operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnaryOperator {
    Not,
    Negate,
}

/// Comparison operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompOp {
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
    Contains,
    StartsWith,
    EndsWith,
    RegexMatch,
    In,
    IsNull,
    IsNotNull,
}

/// A Cypher literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum CypherValue {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    Null,
    List(Vec<CypherValue>),
    Map(Vec<(String, CypherValue)>),
}
