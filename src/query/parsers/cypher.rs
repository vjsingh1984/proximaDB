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

//! # Cypher Query Language Parser
//!
//! This module implements a comprehensive Cypher Query Language parser for ProximaDB's
//! graph database. It follows SOLID principles with separate lexer, parser, and AST
//! conversion components.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │              Cypher Query               │
//! │    "MATCH (n:Person) RETURN n"          │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │              Lexer                      │
//! │  • Tokenizes input into tokens          │
//! │  • Handles keywords, identifiers        │
//! │  • Preserves location information       │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │              Parser                     │
//! │  • Parses patterns (nodes, edges)       │
//! │  • Parses clauses (MATCH, WHERE, etc.)  │
//! │  • Parses expressions and functions     │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │           CypherAst                     │
//! │  • Complete AST representation          │
//! │  • Query structure with clauses         │
//! └───────────────┬─────────────────────────┘
//!                 │
//! ┌───────────────▼─────────────────────────┐
//! │        GraphQuery Conversion            │
//! │  • Converts AST to executable query     │
//! │  • Integrates with graph service        │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Supported Cypher Syntax
//!
//! ### Patterns
//! - Nodes: `(n:Label {prop: value})`
//! - Edges: `-[r:TYPE {prop: value}]->`
//! - Variable-length paths: `(a)-[*1..3]->(b)`
//!
//! ### Clauses
//! - MATCH, OPTIONAL MATCH
//! - WHERE with complex conditions
//! - RETURN with aliases
//! - ORDER BY, LIMIT, SKIP
//! - CREATE, MERGE, DELETE, SET
//!
//! ### Expressions
//! - Properties: `n.prop`
//! - Functions: `labels(n)`, `type(r)`, `id(n)`
//! - Aggregations: `count()`, `sum()`, `avg()`, `collect()`

use crate::core::error::ProximaDBError;
use crate::graph::query::ast::{
    CompiledPattern, CreateClause, CreateEdgeSpec, CreateNodeSpec, CypherQuery, DeleteClause,
    EdgeDirection, EdgePattern, MatchPattern, MergeClause, NodePattern,
    PropertyConstraint, PropertyProjection, ReadingClause, RemoveClause, RemoveItem, ReturnSpec,
    SetClause, SetItem, UpdatingClause, WhereClause, WithClause,
};
use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while, take_while1},
    character::complete::{alpha1, alphanumeric1, char, digit1, multispace0, multispace1, one_of},
    combinator::{map, map_res, opt, recognize, value},
    error::VerboseError,
    multi::{many0, many1, separated_list0, separated_list1},
    sequence::{delimited, pair, preceded, separated_pair, tuple},
};
use std::collections::HashMap;

type ParseResult<'a, T> = IResult<&'a str, T, VerboseError<&'a str>>;

// ============================================================================
// LEXER - Token Types and Tokenization
// ============================================================================

/// Token types for the Cypher lexer
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Match,
    OptionalMatch,
    Where,
    Return,
    OrderBy,
    Limit,
    Skip,
    Create,
    Merge,
    Delete,
    DetachDelete,
    Set,
    Remove,
    With,
    As,
    Distinct,
    And,
    Or,
    Not,
    Xor,
    In,
    Is,
    Null,
    True,
    False,
    Asc,
    Desc,
    OnCreate,
    OnMatch,
    Unwind,
    ForEach,
    Call,
    Yield,

    // Delimiters
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    Comma,
    Colon,
    Dot,
    Semicolon,

    // Operators
    Arrow,         // ->
    LeftArrow,     // <-
    Dash,          // -
    Equals,        // =
    NotEquals,     // <>
    LessThan,      // <
    GreaterThan,   // >
    LessOrEqual,   // <=
    GreaterOrEqual,// >=
    Plus,          // +
    Minus,         // -
    Asterisk,      // *
    Slash,         // /
    Percent,       // %
    Caret,         // ^
    PlusEquals,    // +=
    DoubleDot,     // ..

    // Literals
    Integer(i64),
    Float(f64),
    String(String),
    Identifier(String),
    Parameter(String),

    // Special
    Whitespace,
    Comment(String),
    Eof,
}

/// Token with location information
#[derive(Debug, Clone)]
pub struct LocatedToken {
    pub token: Token,
    pub line: usize,
    pub column: usize,
    pub span: std::ops::Range<usize>,
}

/// Lexer for tokenizing Cypher queries
pub struct CypherLexer {
    /// Current position in input
    position: usize,
    /// Current line number
    line: usize,
    /// Current column number
    column: usize,
}

impl CypherLexer {
    /// Create a new lexer
    pub fn new() -> Self {
        Self {
            position: 0,
            line: 1,
            column: 1,
        }
    }

    /// Tokenize input into a vector of located tokens
    pub fn tokenize(&mut self, input: &str) -> Result<Vec<LocatedToken>, ProximaDBError> {
        let mut tokens = Vec::new();
        self.position = 0;
        self.line = 1;
        self.column = 1;

        let chars: Vec<char> = input.chars().collect();

        while self.position < chars.len() {
            let start_pos = self.position;
            let start_line = self.line;
            let start_col = self.column;

            let token = self.next_token(&chars)?;

            if !matches!(token, Token::Whitespace) {
                tokens.push(LocatedToken {
                    token,
                    line: start_line,
                    column: start_col,
                    span: start_pos..self.position,
                });
            }
        }

        tokens.push(LocatedToken {
            token: Token::Eof,
            line: self.line,
            column: self.column,
            span: self.position..self.position,
        });

        Ok(tokens)
    }

    fn next_token(&mut self, chars: &[char]) -> Result<Token, ProximaDBError> {
        let c = chars[self.position];

        // Whitespace
        if c.is_whitespace() {
            self.consume_whitespace(chars);
            return Ok(Token::Whitespace);
        }

        // Comments
        if c == '/' && self.position + 1 < chars.len() && chars[self.position + 1] == '/' {
            let comment = self.consume_line_comment(chars);
            return Ok(Token::Comment(comment));
        }

        // String literals
        if c == '"' || c == '\'' {
            return self.consume_string(chars, c);
        }

        // Numbers
        if c.is_ascii_digit() || (c == '-' && self.position + 1 < chars.len() && chars[self.position + 1].is_ascii_digit()) {
            return self.consume_number(chars);
        }

        // Identifiers and keywords
        if c.is_alphabetic() || c == '_' || c == '`' {
            return self.consume_identifier_or_keyword(chars);
        }

        // Parameters
        if c == '$' {
            return self.consume_parameter(chars);
        }

        // Operators and delimiters
        self.consume_operator(chars)
    }

    fn consume_whitespace(&mut self, chars: &[char]) {
        while self.position < chars.len() && chars[self.position].is_whitespace() {
            if chars[self.position] == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.position += 1;
        }
    }

    fn consume_line_comment(&mut self, chars: &[char]) -> String {
        let mut comment = String::new();
        self.position += 2; // Skip //
        self.column += 2;

        while self.position < chars.len() && chars[self.position] != '\n' {
            comment.push(chars[self.position]);
            self.position += 1;
            self.column += 1;
        }

        comment
    }

    fn consume_string(&mut self, chars: &[char], quote: char) -> Result<Token, ProximaDBError> {
        let mut s = String::new();
        self.position += 1; // Skip opening quote
        self.column += 1;

        while self.position < chars.len() {
            let c = chars[self.position];

            if c == quote {
                self.position += 1;
                self.column += 1;
                return Ok(Token::String(s));
            }

            if c == '\\' && self.position + 1 < chars.len() {
                self.position += 1;
                self.column += 1;
                let escaped = match chars[self.position] {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '\\' => '\\',
                    '"' => '"',
                    '\'' => '\'',
                    _ => chars[self.position],
                };
                s.push(escaped);
            } else {
                s.push(c);
            }

            self.position += 1;
            self.column += 1;
        }

        Err(ProximaDBError::InvalidInput("Unterminated string literal".to_string()))
    }

    fn consume_number(&mut self, chars: &[char]) -> Result<Token, ProximaDBError> {
        let mut num_str = String::new();
        let mut is_float = false;

        // Handle negative sign
        if chars[self.position] == '-' {
            num_str.push('-');
            self.position += 1;
            self.column += 1;
        }

        // Integer part
        while self.position < chars.len() && chars[self.position].is_ascii_digit() {
            num_str.push(chars[self.position]);
            self.position += 1;
            self.column += 1;
        }

        // Decimal part
        if self.position < chars.len() && chars[self.position] == '.' {
            if self.position + 1 < chars.len() && chars[self.position + 1].is_ascii_digit() {
                is_float = true;
                num_str.push('.');
                self.position += 1;
                self.column += 1;

                while self.position < chars.len() && chars[self.position].is_ascii_digit() {
                    num_str.push(chars[self.position]);
                    self.position += 1;
                    self.column += 1;
                }
            }
        }

        // Exponent part
        if self.position < chars.len() && (chars[self.position] == 'e' || chars[self.position] == 'E') {
            is_float = true;
            num_str.push('e');
            self.position += 1;
            self.column += 1;

            if self.position < chars.len() && (chars[self.position] == '+' || chars[self.position] == '-') {
                num_str.push(chars[self.position]);
                self.position += 1;
                self.column += 1;
            }

            while self.position < chars.len() && chars[self.position].is_ascii_digit() {
                num_str.push(chars[self.position]);
                self.position += 1;
                self.column += 1;
            }
        }

        if is_float {
            let f: f64 = num_str.parse().map_err(|_| {
                ProximaDBError::InvalidInput(format!("Invalid float: {}", num_str))
            })?;
            Ok(Token::Float(f))
        } else {
            let i: i64 = num_str.parse().map_err(|_| {
                ProximaDBError::InvalidInput(format!("Invalid integer: {}", num_str))
            })?;
            Ok(Token::Integer(i))
        }
    }

    fn consume_identifier_or_keyword(&mut self, chars: &[char]) -> Result<Token, ProximaDBError> {
        let mut ident = String::new();
        let is_quoted = chars[self.position] == '`';

        if is_quoted {
            self.position += 1;
            self.column += 1;

            while self.position < chars.len() && chars[self.position] != '`' {
                ident.push(chars[self.position]);
                self.position += 1;
                self.column += 1;
            }

            if self.position < chars.len() {
                self.position += 1; // Skip closing backtick
                self.column += 1;
            }

            return Ok(Token::Identifier(ident));
        }

        while self.position < chars.len() && (chars[self.position].is_alphanumeric() || chars[self.position] == '_') {
            ident.push(chars[self.position]);
            self.position += 1;
            self.column += 1;
        }

        // Check for keywords (case-insensitive)
        let token = match ident.to_uppercase().as_str() {
            "MATCH" => Token::Match,
            "OPTIONAL" => {
                // Check for OPTIONAL MATCH
                let saved_pos = self.position;
                let saved_col = self.column;
                self.consume_whitespace(chars);
                let next_start = self.position;
                let mut next_word = String::new();
                while self.position < chars.len() && chars[self.position].is_alphabetic() {
                    next_word.push(chars[self.position]);
                    self.position += 1;
                    self.column += 1;
                }
                if next_word.to_uppercase() == "MATCH" {
                    Token::OptionalMatch
                } else {
                    self.position = saved_pos;
                    self.column = saved_col;
                    Token::Identifier(ident)
                }
            }
            "WHERE" => Token::Where,
            "RETURN" => Token::Return,
            "ORDER" => Token::OrderBy,
            "BY" => Token::Identifier(ident), // BY is handled with ORDER
            "LIMIT" => Token::Limit,
            "SKIP" => Token::Skip,
            "CREATE" => Token::Create,
            "MERGE" => Token::Merge,
            "DELETE" => Token::Delete,
            "DETACH" => {
                // Check for DETACH DELETE
                let saved_pos = self.position;
                let saved_col = self.column;
                self.consume_whitespace(chars);
                let mut next_word = String::new();
                while self.position < chars.len() && chars[self.position].is_alphabetic() {
                    next_word.push(chars[self.position]);
                    self.position += 1;
                    self.column += 1;
                }
                if next_word.to_uppercase() == "DELETE" {
                    Token::DetachDelete
                } else {
                    self.position = saved_pos;
                    self.column = saved_col;
                    Token::Identifier(ident)
                }
            }
            "SET" => Token::Set,
            "REMOVE" => Token::Remove,
            "WITH" => Token::With,
            "AS" => Token::As,
            "DISTINCT" => Token::Distinct,
            "AND" => Token::And,
            "OR" => Token::Or,
            "NOT" => Token::Not,
            "XOR" => Token::Xor,
            "IN" => Token::In,
            "IS" => Token::Is,
            "NULL" => Token::Null,
            "TRUE" => Token::True,
            "FALSE" => Token::False,
            "ASC" | "ASCENDING" => Token::Asc,
            "DESC" | "DESCENDING" => Token::Desc,
            "ON" => Token::Identifier(ident), // ON CREATE / ON MATCH handled separately
            "UNWIND" => Token::Unwind,
            "FOREACH" => Token::ForEach,
            "CALL" => Token::Call,
            "YIELD" => Token::Yield,
            _ => Token::Identifier(ident),
        };

        Ok(token)
    }

    fn consume_parameter(&mut self, chars: &[char]) -> Result<Token, ProximaDBError> {
        self.position += 1; // Skip $
        self.column += 1;

        let mut name = String::new();
        while self.position < chars.len() && (chars[self.position].is_alphanumeric() || chars[self.position] == '_') {
            name.push(chars[self.position]);
            self.position += 1;
            self.column += 1;
        }

        Ok(Token::Parameter(name))
    }

    fn consume_operator(&mut self, chars: &[char]) -> Result<Token, ProximaDBError> {
        let c = chars[self.position];
        let next = if self.position + 1 < chars.len() { Some(chars[self.position + 1]) } else { None };

        let token = match (c, next) {
            ('-', Some('>')) => { self.position += 2; self.column += 2; Token::Arrow }
            ('<', Some('-')) => { self.position += 2; self.column += 2; Token::LeftArrow }
            ('<', Some('>')) => { self.position += 2; self.column += 2; Token::NotEquals }
            ('<', Some('=')) => { self.position += 2; self.column += 2; Token::LessOrEqual }
            ('>', Some('=')) => { self.position += 2; self.column += 2; Token::GreaterOrEqual }
            ('+', Some('=')) => { self.position += 2; self.column += 2; Token::PlusEquals }
            ('.', Some('.')) => { self.position += 2; self.column += 2; Token::DoubleDot }
            ('(', _) => { self.position += 1; self.column += 1; Token::OpenParen }
            (')', _) => { self.position += 1; self.column += 1; Token::CloseParen }
            ('[', _) => { self.position += 1; self.column += 1; Token::OpenBracket }
            (']', _) => { self.position += 1; self.column += 1; Token::CloseBracket }
            ('{', _) => { self.position += 1; self.column += 1; Token::OpenBrace }
            ('}', _) => { self.position += 1; self.column += 1; Token::CloseBrace }
            (',', _) => { self.position += 1; self.column += 1; Token::Comma }
            (':', _) => { self.position += 1; self.column += 1; Token::Colon }
            ('.', _) => { self.position += 1; self.column += 1; Token::Dot }
            (';', _) => { self.position += 1; self.column += 1; Token::Semicolon }
            ('-', _) => { self.position += 1; self.column += 1; Token::Dash }
            ('=', _) => { self.position += 1; self.column += 1; Token::Equals }
            ('<', _) => { self.position += 1; self.column += 1; Token::LessThan }
            ('>', _) => { self.position += 1; self.column += 1; Token::GreaterThan }
            ('+', _) => { self.position += 1; self.column += 1; Token::Plus }
            ('*', _) => { self.position += 1; self.column += 1; Token::Asterisk }
            ('/', _) => { self.position += 1; self.column += 1; Token::Slash }
            ('%', _) => { self.position += 1; self.column += 1; Token::Percent }
            ('^', _) => { self.position += 1; self.column += 1; Token::Caret }
            _ => return Err(ProximaDBError::InvalidInput(format!("Unexpected character: {}", c))),
        };

        Ok(token)
    }
}

impl Default for CypherLexer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// PARSER - Pattern, Clause, and Expression Parsing
// ============================================================================

/// Main Cypher parser following SOLID principles
pub struct CypherParser;

impl CypherParser {
    /// Create a new parser instance
    pub fn new() -> Self {
        Self
    }

    /// Parse a Cypher query string into a CypherQuery AST
    pub fn parse(&self, input: &str) -> Result<CypherQuery, ProximaDBError> {
        match parse_cypher_query(input.trim()) {
            Ok((remaining, query)) => {
                if remaining.trim().is_empty() {
                    Ok(query)
                } else {
                    Err(ProximaDBError::InvalidInput(format!(
                        "Unexpected input remaining after parsing: '{}'",
                        remaining.chars().take(50).collect::<String>()
                    )))
                }
            }
            Err(e) => Err(ProximaDBError::InvalidInput(format!(
                "Failed to parse Cypher query: {:?}",
                e
            ))),
        }
    }

    /// Parse into CompiledPattern for backward compatibility
    pub fn parse_to_compiled_pattern(&self, input: &str) -> Result<CompiledPattern, ProximaDBError> {
        match parse_simple_query(input.trim()) {
            Ok((_, compiled)) => Ok(compiled),
            Err(e) => Err(ProximaDBError::InvalidInput(format!(
                "Failed to parse query pattern: {:?}",
                e
            ))),
        }
    }
}

impl Default for CypherParser {
    fn default() -> Self {
        Self::new()
    }
}

impl super::QueryParser for CypherParser {
    type Output = CypherQuery;

    fn parse(&self, input: &str) -> anyhow::Result<Self::Output> {
        CypherParser::parse(self, input).map_err(|e| anyhow::anyhow!("{}", e))
    }
}

// ============================================================================
// NOM PARSER COMBINATORS
// ============================================================================

/// Parse identifier (variable names, labels, property keys)
fn identifier(input: &str) -> ParseResult<String> {
    map(
        recognize(tuple((
            alt((alpha1, tag("_"))),
            many0(alt((alphanumeric1, tag("_")))),
        ))),
        String::from,
    )(input)
}

/// Parse backtick-quoted identifier
fn quoted_identifier(input: &str) -> ParseResult<String> {
    map(
        delimited(
            char('`'),
            take_while1(|c| c != '`'),
            char('`'),
        ),
        String::from,
    )(input)
}

/// Parse any identifier (normal or quoted)
fn any_identifier(input: &str) -> ParseResult<String> {
    alt((quoted_identifier, identifier))(input)
}

/// Parse string literal (single or double quoted)
fn string_literal(input: &str) -> ParseResult<String> {
    alt((
        map(
            delimited(
                char('"'),
                take_while(|c| c != '"'),
                char('"'),
            ),
            String::from,
        ),
        map(
            delimited(
                char('\''),
                take_while(|c| c != '\''),
                char('\''),
            ),
            String::from,
        ),
    ))(input)
}

/// Parse integer literal
fn integer_literal(input: &str) -> ParseResult<i64> {
    map_res(
        recognize(tuple((
            opt(char('-')),
            digit1,
        ))),
        |s: &str| s.parse::<i64>(),
    )(input)
}

/// Parse float literal
fn float_literal(input: &str) -> ParseResult<f64> {
    map_res(
        recognize(tuple((
            opt(char('-')),
            digit1,
            char('.'),
            digit1,
            opt(tuple((
                one_of("eE"),
                opt(one_of("+-")),
                digit1,
            ))),
        ))),
        |s: &str| s.parse::<f64>(),
    )(input)
}

/// Parse boolean literal
fn boolean_literal(input: &str) -> ParseResult<bool> {
    alt((
        value(true, tag_no_case("true")),
        value(false, tag_no_case("false")),
    ))(input)
}

/// Parse null literal
fn null_literal(input: &str) -> ParseResult<serde_json::Value> {
    value(serde_json::Value::Null, tag_no_case("null"))(input)
}

/// Parse a property value (string, number, boolean, null)
fn property_value(input: &str) -> ParseResult<serde_json::Value> {
    alt((
        map(string_literal, serde_json::Value::String),
        map(float_literal, |f| serde_json::Value::Number(
            serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0))
        )),
        map(integer_literal, |i| serde_json::Value::Number(serde_json::Number::from(i))),
        map(boolean_literal, serde_json::Value::Bool),
        null_literal,
        // Array value
        map(
            delimited(
                char('['),
                separated_list0(
                    delimited(multispace0, char(','), multispace0),
                    property_value,
                ),
                char(']'),
            ),
            serde_json::Value::Array,
        ),
    ))(input)
}

/// Parse property assignment: key: value
fn property_assignment(input: &str) -> ParseResult<(String, PropertyConstraint)> {
    map(
        separated_pair(
            any_identifier,
            delimited(multispace0, char(':'), multispace0),
            property_value,
        ),
        |(key, value)| (key, PropertyConstraint::Equals(value)),
    )(input)
}

/// Parse property map: {key: value, ...}
fn property_map(input: &str) -> ParseResult<HashMap<String, PropertyConstraint>> {
    map(
        delimited(
            pair(char('{'), multispace0),
            separated_list0(
                delimited(multispace0, char(','), multispace0),
                property_assignment,
            ),
            pair(multispace0, char('}')),
        ),
        |pairs| pairs.into_iter().collect(),
    )(input)
}

/// Parse property value map: {key: value, ...} for CREATE
fn property_value_map(input: &str) -> ParseResult<HashMap<String, serde_json::Value>> {
    map(
        delimited(
            pair(char('{'), multispace0),
            separated_list0(
                delimited(multispace0, char(','), multispace0),
                separated_pair(
                    any_identifier,
                    delimited(multispace0, char(':'), multispace0),
                    property_value,
                ),
            ),
            pair(multispace0, char('}')),
        ),
        |pairs| pairs.into_iter().collect(),
    )(input)
}

/// Parse labels: :Label1:Label2
fn parse_labels(input: &str) -> ParseResult<Vec<String>> {
    many1(preceded(char(':'), any_identifier))(input)
}

/// Parse optional labels
fn parse_optional_labels(input: &str) -> ParseResult<Vec<String>> {
    map(
        opt(parse_labels),
        |labels| labels.unwrap_or_default(),
    )(input)
}

/// Parse node pattern: (variable:Label {props})
fn parse_node_pattern(input: &str) -> ParseResult<NodePattern> {
    map(
        delimited(
            char('('),
            tuple((
                preceded(multispace0, opt(any_identifier)),
                parse_optional_labels,
                preceded(multispace0, opt(property_map)),
                multispace0,
            )),
            char(')'),
        ),
        |(variable_opt, labels, properties_opt, _)| NodePattern {
            variable: variable_opt.unwrap_or_else(|| "_anon".to_string()),
            labels,
            properties: properties_opt.unwrap_or_default(),
            optional: false,
        },
    )(input)
}

/// Parse edge direction left: <- or -
fn parse_edge_left(input: &str) -> ParseResult<bool> {
    alt((
        value(true, tag("<-")),
        value(false, tag("-")),
    ))(input)
}

/// Parse edge direction right: -> or -
fn parse_edge_right(input: &str) -> ParseResult<bool> {
    alt((
        value(true, tag("->")),
        value(false, tag("-")),
    ))(input)
}

/// Parse variable-length path: *min..max or *max or *
fn parse_var_length(input: &str) -> ParseResult<(u32, u32)> {
    preceded(
        char('*'),
        alt((
            // *min..max
            map(
                separated_pair(
                    map_res(digit1, |s: &str| s.parse::<u32>()),
                    tag(".."),
                    map_res(digit1, |s: &str| s.parse::<u32>()),
                ),
                |(min, max)| (min, max),
            ),
            // *n (exactly n or 1..n)
            map(map_res(digit1, |s: &str| s.parse::<u32>()), |n| (1, n)),
            // * (default: 1..infinity, we use 99 as max)
            value((1, 99), multispace0),
        )),
    )(input)
}

/// Parse edge type: :TYPE1|TYPE2
fn parse_edge_types(input: &str) -> ParseResult<Vec<String>> {
    preceded(
        char(':'),
        separated_list1(char('|'), any_identifier),
    )(input)
}

/// Parse edge specification: [variable:TYPE {props}]
fn parse_edge_spec(input: &str) -> ParseResult<(
    Option<String>,      // variable
    Vec<String>,         // edge types
    HashMap<String, PropertyConstraint>, // properties
    Option<(u32, u32)>,  // variable length
)> {
    map(
        delimited(
            char('['),
            tuple((
                preceded(multispace0, opt(any_identifier)),
                opt(parse_edge_types),
                preceded(multispace0, opt(parse_var_length)),
                preceded(multispace0, opt(property_map)),
                multispace0,
            )),
            char(']'),
        ),
        |(var, types, var_len, props, _)| (
            var,
            types.unwrap_or_default(),
            props.unwrap_or_default(),
            var_len,
        ),
    )(input)
}

/// Parse full edge pattern: <-[spec]- or -[spec]-> or -[spec]-
fn parse_edge_pattern_full(input: &str) -> ParseResult<(
    bool,                 // is incoming
    Option<String>,       // variable
    Vec<String>,          // edge types
    HashMap<String, PropertyConstraint>, // properties
    bool,                 // is outgoing
    Option<(u32, u32)>,   // variable length
)> {
    map(
        tuple((
            parse_edge_left,
            parse_edge_spec,
            parse_edge_right,
        )),
        |(left, (var, types, props, var_len), right)| (
            left,
            var,
            types,
            props,
            right,
            var_len,
        ),
    )(input)
}

/// Parse a path segment: (node)-[edge]->(node)
fn parse_path_segment(input: &str) -> ParseResult<(NodePattern, EdgePattern, NodePattern)> {
    map(
        tuple((
            parse_node_pattern,
            preceded(multispace0, parse_edge_pattern_full),
            preceded(multispace0, parse_node_pattern),
        )),
        |(from_node, (is_incoming, edge_var, edge_types, edge_props, is_outgoing, _var_len), to_node)| {
            let direction = match (is_incoming, is_outgoing) {
                (false, true) => EdgeDirection::Outgoing,
                (true, false) => EdgeDirection::Incoming,
                _ => EdgeDirection::Bidirectional,
            };

            let edge = EdgePattern {
                variable: edge_var,
                from_variable: from_node.variable.clone(),
                to_variable: to_node.variable.clone(),
                edge_types,
                properties: edge_props,
                direction,
                optional: false,
            };

            (from_node, edge, to_node)
        },
    )(input)
}

/// Parse comparison operator
fn parse_comparison_op(input: &str) -> ParseResult<&str> {
    alt((
        tag(">="),
        tag("<="),
        tag("<>"),
        tag("!="),
        tag("=~"),
        tag("="),
        tag("<"),
        tag(">"),
    ))(input)
}

/// Parse a WHERE property condition: variable.property op value
fn parse_where_property(input: &str) -> ParseResult<WhereClause> {
    map(
        tuple((
            any_identifier,
            char('.'),
            any_identifier,
            delimited(multispace0, parse_comparison_op, multispace0),
            property_value,
        )),
        |(variable, _, property, op, value)| WhereClause::Property {
            variable,
            property,
            constraint: match op {
                "=" => PropertyConstraint::Equals(value),
                "<>" | "!=" => PropertyConstraint::NotEquals(value),
                ">" => PropertyConstraint::GreaterThan(value),
                ">=" => PropertyConstraint::GreaterOrEqual(value),
                "<" => PropertyConstraint::LessThan(value),
                "<=" => PropertyConstraint::LessOrEqual(value),
                "=~" => {
                    if let serde_json::Value::String(s) = value {
                        PropertyConstraint::Regex(s)
                    } else {
                        PropertyConstraint::Equals(value)
                    }
                }
                _ => PropertyConstraint::Equals(value),
            },
        },
    )(input)
}

/// Parse IS NULL / IS NOT NULL
fn parse_is_null(input: &str) -> ParseResult<WhereClause> {
    alt((
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                delimited(multispace1, tag_no_case("IS"), multispace1),
                tag_no_case("NULL"),
            )),
            |(variable, _, property, _, _)| WhereClause::Property {
                variable,
                property,
                constraint: PropertyConstraint::NotExists,
            },
        ),
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                delimited(multispace1, tag_no_case("IS"), multispace1),
                tag_no_case("NOT"),
                multispace1,
                tag_no_case("NULL"),
            )),
            |(variable, _, property, _, _, _, _)| WhereClause::Property {
                variable,
                property,
                constraint: PropertyConstraint::Exists,
            },
        ),
    ))(input)
}

/// Parse IN clause
fn parse_in_clause(input: &str) -> ParseResult<WhereClause> {
    map(
        tuple((
            any_identifier,
            char('.'),
            any_identifier,
            delimited(multispace1, tag_no_case("IN"), multispace1),
            delimited(
                char('['),
                separated_list0(
                    delimited(multispace0, char(','), multispace0),
                    property_value,
                ),
                char(']'),
            ),
        )),
        |(variable, _, property, _, values)| WhereClause::Property {
            variable,
            property,
            constraint: PropertyConstraint::In(values),
        },
    )(input)
}

/// Parse STARTS WITH / ENDS WITH / CONTAINS
fn parse_string_predicates(input: &str) -> ParseResult<WhereClause> {
    alt((
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                delimited(multispace1, tag_no_case("STARTS"), multispace1),
                tag_no_case("WITH"),
                multispace1,
                string_literal,
            )),
            |(variable, _, property, _, _, _, value)| WhereClause::Property {
                variable,
                property,
                constraint: PropertyConstraint::StartsWith(value),
            },
        ),
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                delimited(multispace1, tag_no_case("ENDS"), multispace1),
                tag_no_case("WITH"),
                multispace1,
                string_literal,
            )),
            |(variable, _, property, _, _, _, value)| WhereClause::Property {
                variable,
                property,
                constraint: PropertyConstraint::EndsWith(value),
            },
        ),
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                delimited(multispace1, tag_no_case("CONTAINS"), multispace1),
                string_literal,
            )),
            |(variable, _, property, _, value)| WhereClause::Property {
                variable,
                property,
                constraint: PropertyConstraint::Contains(value),
            },
        ),
    ))(input)
}

/// Parse primary WHERE condition
fn parse_where_primary(input: &str) -> ParseResult<WhereClause> {
    alt((
        // NOT condition
        map(
            preceded(
                pair(tag_no_case("NOT"), multispace1),
                parse_where_primary,
            ),
            |cond| WhereClause::Not(Box::new(cond)),
        ),
        // Parenthesized condition
        delimited(
            pair(char('('), multispace0),
            parse_where_condition,
            pair(multispace0, char(')')),
        ),
        // IS NULL / IS NOT NULL
        parse_is_null,
        // IN clause
        parse_in_clause,
        // String predicates
        parse_string_predicates,
        // Property comparison
        parse_where_property,
    ))(input)
}

/// Parse WHERE condition with AND/OR
fn parse_where_condition(input: &str) -> ParseResult<WhereClause> {
    let (input, first) = parse_where_primary(input)?;

    // Try to parse chained AND/OR
    let result = many0(tuple((
        delimited(multispace1, alt((tag_no_case("AND"), tag_no_case("OR"), tag_no_case("XOR"))), multispace1),
        parse_where_primary,
    )))(input);

    match result {
        Ok((remaining, pairs)) => {
            let mut current = first;
            for (op, right) in pairs {
                current = match op.to_uppercase().as_str() {
                    "AND" => WhereClause::And(Box::new(current), Box::new(right)),
                    "OR" => WhereClause::Or(Box::new(current), Box::new(right)),
                    "XOR" => {
                        // XOR = (A OR B) AND NOT (A AND B)
                        let a_or_b = WhereClause::Or(Box::new(current.clone()), Box::new(right.clone()));
                        let a_and_b = WhereClause::And(Box::new(current), Box::new(right));
                        WhereClause::And(Box::new(a_or_b), Box::new(WhereClause::Not(Box::new(a_and_b))))
                    }
                    _ => unreachable!(),
                };
            }
            Ok((remaining, current))
        }
        Err(e) => Err(e),
    }
}

/// Parse WHERE clause
fn parse_where_clause(input: &str) -> ParseResult<WhereClause> {
    preceded(
        pair(tag_no_case("WHERE"), multispace1),
        parse_where_condition,
    )(input)
}

/// Parse aggregation function
fn parse_aggregation(input: &str) -> ParseResult<PropertyProjection> {
    alt((
        // COUNT(*)
        map(
            tuple((
                tag_no_case("COUNT"),
                multispace0,
                char('('),
                multispace0,
                char('*'),
                multispace0,
                char(')'),
            )),
            |_| PropertyProjection::Count,
        ),
        // COUNT(DISTINCT expr) - we simplify to count
        map(
            tuple((
                tag_no_case("COUNT"),
                multispace0,
                char('('),
                multispace0,
                tag_no_case("DISTINCT"),
                multispace1,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |_| PropertyProjection::Count,
        ),
        // SUM(variable.property)
        map(
            tuple((
                tag_no_case("SUM"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                char('.'),
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, prop, _, _)| PropertyProjection::Sum {
                variable: var,
                property: prop,
            },
        ),
        // AVG(variable.property)
        map(
            tuple((
                tag_no_case("AVG"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                char('.'),
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, prop, _, _)| PropertyProjection::Avg {
                variable: var,
                property: prop,
            },
        ),
        // MIN(variable.property)
        map(
            tuple((
                tag_no_case("MIN"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                char('.'),
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, prop, _, _)| PropertyProjection::Min {
                variable: var,
                property: prop,
            },
        ),
        // MAX(variable.property)
        map(
            tuple((
                tag_no_case("MAX"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                char('.'),
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, prop, _, _)| PropertyProjection::Max {
                variable: var,
                property: prop,
            },
        ),
    ))(input)
}

/// Parse function call (labels, type, id, etc.)
fn parse_function_call(input: &str) -> ParseResult<CypherFunction> {
    alt((
        map(
            tuple((
                tag_no_case("labels"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, _)| CypherFunction::Labels(var),
        ),
        map(
            tuple((
                tag_no_case("type"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, _)| CypherFunction::Type(var),
        ),
        map(
            tuple((
                tag_no_case("id"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, _)| CypherFunction::Id(var),
        ),
        map(
            tuple((
                tag_no_case("properties"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, _)| CypherFunction::Properties(var),
        ),
        map(
            tuple((
                tag_no_case("keys"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, _)| CypherFunction::Keys(var),
        ),
        map(
            tuple((
                tag_no_case("exists"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                char('.'),
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, prop, _, _)| CypherFunction::Exists(var, prop),
        ),
        map(
            tuple((
                tag_no_case("collect"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, _)| CypherFunction::Collect(var),
        ),
        map(
            tuple((
                tag_no_case("collect"),
                multispace0,
                char('('),
                multispace0,
                any_identifier,
                char('.'),
                any_identifier,
                multispace0,
                char(')'),
            )),
            |(_, _, _, _, var, _, prop, _, _)| CypherFunction::CollectProperty(var, prop),
        ),
    ))(input)
}

/// Cypher function types
#[derive(Debug, Clone, PartialEq)]
pub enum CypherFunction {
    Labels(String),
    Type(String),
    Id(String),
    Properties(String),
    Keys(String),
    Exists(String, String),
    Collect(String),
    CollectProperty(String, String),
}

/// Parse return item: variable, variable.property, aggregation, or function
fn parse_return_item(input: &str) -> ParseResult<(String, PropertyProjection)> {
    alt((
        // Aggregation with alias
        map(
            tuple((
                parse_aggregation,
                opt(preceded(
                    delimited(multispace1, tag_no_case("AS"), multispace1),
                    any_identifier,
                )),
            )),
            |(agg, alias)| {
                let name = alias.unwrap_or_else(|| match &agg {
                    PropertyProjection::Count => "count".to_string(),
                    PropertyProjection::Sum { property, .. } => format!("sum_{}", property),
                    PropertyProjection::Avg { property, .. } => format!("avg_{}", property),
                    PropertyProjection::Min { property, .. } => format!("min_{}", property),
                    PropertyProjection::Max { property, .. } => format!("max_{}", property),
                    _ => "result".to_string(),
                });
                (name, agg)
            },
        ),
        // Property with alias: n.name AS person_name
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                opt(preceded(
                    delimited(multispace1, tag_no_case("AS"), multispace1),
                    any_identifier,
                )),
            )),
            |(var, _, prop, alias)| {
                let name = alias.unwrap_or_else(|| format!("{}.{}", var, prop));
                (
                    name,
                    PropertyProjection::Property {
                        variable: var,
                        property: prop,
                    },
                )
            },
        ),
        // Simple variable with alias
        map(
            tuple((
                any_identifier,
                opt(preceded(
                    delimited(multispace1, tag_no_case("AS"), multispace1),
                    any_identifier,
                )),
            )),
            |(var, alias)| {
                let name = alias.unwrap_or_else(|| var.clone());
                (name, PropertyProjection::Variable(var))
            },
        ),
    ))(input)
}

/// Parse ORDER BY clause
fn parse_order_by(input: &str) -> ParseResult<Vec<(String, bool)>> {
    preceded(
        tuple((
            tag_no_case("ORDER"),
            multispace1,
            tag_no_case("BY"),
            multispace1,
        )),
        separated_list1(
            delimited(multispace0, char(','), multispace0),
            map(
                tuple((
                    any_identifier,
                    opt(preceded(
                        multispace1,
                        alt((
                            value(true, alt((tag_no_case("ASC"), tag_no_case("ASCENDING")))),
                            value(false, alt((tag_no_case("DESC"), tag_no_case("DESCENDING")))),
                        )),
                    )),
                )),
                |(var, asc)| (var, asc.unwrap_or(true)),
            ),
        ),
    )(input)
}

/// Parse LIMIT clause
fn parse_limit(input: &str) -> ParseResult<usize> {
    preceded(
        pair(tag_no_case("LIMIT"), multispace1),
        map_res(digit1, |s: &str| s.parse::<usize>()),
    )(input)
}

/// Parse SKIP clause
fn parse_skip(input: &str) -> ParseResult<usize> {
    preceded(
        pair(tag_no_case("SKIP"), multispace1),
        map_res(digit1, |s: &str| s.parse::<usize>()),
    )(input)
}

/// Parse RETURN clause
fn parse_return_clause(input: &str) -> ParseResult<ReturnSpec> {
    map(
        tuple((
            tag_no_case("RETURN"),
            opt(preceded(multispace1, tag_no_case("DISTINCT"))),
            multispace1,
            separated_list1(
                delimited(multispace0, char(','), multispace0),
                parse_return_item,
            ),
            opt(preceded(multispace1, parse_order_by)),
            opt(preceded(multispace1, parse_skip)),
            opt(preceded(multispace1, parse_limit)),
        )),
        |(_, distinct, _, items, order_by, skip, limit)| {
            let (vars, projections): (Vec<_>, Vec<_>) = items.into_iter().unzip();
            ReturnSpec {
                variables: vars,
                projections,
                distinct: distinct.is_some(),
                order_by: order_by.unwrap_or_default(),
                limit,
                skip,
            }
        },
    )(input)
}

/// Parse MATCH clause
fn parse_match_clause(input: &str) -> ParseResult<(Vec<NodePattern>, Vec<EdgePattern>)> {
    preceded(
        pair(tag_no_case("MATCH"), multispace1),
        map(
            separated_list1(
                delimited(multispace0, char(','), multispace0),
                alt((
                    // Path pattern
                    map(parse_path_segment, |(from, edge, to)| (vec![from, to], vec![edge])),
                    // Simple node
                    map(parse_node_pattern, |node| (vec![node], vec![])),
                )),
            ),
            |patterns| {
                let mut nodes = Vec::new();
                let mut edges = Vec::new();
                for (mut ns, mut es) in patterns {
                    nodes.append(&mut ns);
                    edges.append(&mut es);
                }
                (nodes, edges)
            },
        ),
    )(input)
}

/// Parse OPTIONAL MATCH clause
fn parse_optional_match_clause(input: &str) -> ParseResult<(Vec<NodePattern>, Vec<EdgePattern>)> {
    preceded(
        tuple((tag_no_case("OPTIONAL"), multispace1, tag_no_case("MATCH"), multispace1)),
        map(
            separated_list1(
                delimited(multispace0, char(','), multispace0),
                alt((
                    map(parse_path_segment, |(from, edge, to)| (vec![from, to], vec![edge])),
                    map(parse_node_pattern, |node| (vec![node], vec![])),
                )),
            ),
            |patterns| {
                let mut nodes = Vec::new();
                let mut edges = Vec::new();
                for (mut ns, mut es) in patterns {
                    for n in ns.iter_mut() {
                        n.optional = true;
                    }
                    for e in es.iter_mut() {
                        e.optional = true;
                    }
                    nodes.append(&mut ns);
                    edges.append(&mut es);
                }
                (nodes, edges)
            },
        ),
    )(input)
}

/// Parse CREATE node spec
fn parse_create_node_spec(input: &str) -> ParseResult<CreateNodeSpec> {
    map(
        delimited(
            char('('),
            tuple((
                preceded(multispace0, opt(any_identifier)),
                parse_optional_labels,
                preceded(multispace0, opt(property_value_map)),
                multispace0,
            )),
            char(')'),
        ),
        |(var, labels, props, _)| CreateNodeSpec {
            variable: var,
            labels,
            properties: props.unwrap_or_default(),
        },
    )(input)
}

/// Parse CREATE edge spec
fn parse_create_edge_spec(input: &str) -> ParseResult<(
    Option<String>,       // variable
    Option<String>,       // edge type
    HashMap<String, serde_json::Value>, // properties
    EdgeDirection,
)> {
    map(
        tuple((
            parse_edge_left,
            delimited(
                char('['),
                tuple((
                    preceded(multispace0, opt(any_identifier)),
                    opt(preceded(char(':'), any_identifier)),
                    preceded(multispace0, opt(property_value_map)),
                    multispace0,
                )),
                char(']'),
            ),
            parse_edge_right,
        )),
        |(left, (var, edge_type, props, _), right)| {
            let direction = match (left, right) {
                (false, true) => EdgeDirection::Outgoing,
                (true, false) => EdgeDirection::Incoming,
                _ => EdgeDirection::Bidirectional,
            };
            (var, edge_type, props.unwrap_or_default(), direction)
        },
    )(input)
}

/// Parse CREATE edge pattern: (a)-[r:TYPE]->(b)
fn parse_create_edge_pattern(input: &str) -> ParseResult<(CreateNodeSpec, CreateEdgeSpec, CreateNodeSpec)> {
    map(
        tuple((
            parse_create_node_spec,
            preceded(multispace0, parse_create_edge_spec),
            preceded(multispace0, parse_create_node_spec),
        )),
        |(from_node, (edge_var, edge_type, edge_props, _direction), to_node)| {
            let from_var = from_node.variable.clone().unwrap_or_else(|| "_from".to_string());
            let to_var = to_node.variable.clone().unwrap_or_else(|| "_to".to_string());

            let edge = CreateEdgeSpec {
                variable: edge_var,
                from_variable: from_var,
                to_variable: to_var,
                edge_type: edge_type.unwrap_or_default(),
                properties: edge_props,
            };

            (from_node, edge, to_node)
        },
    )(input)
}

/// Parse CREATE clause
fn parse_create_clause(input: &str) -> ParseResult<CreateClause> {
    preceded(
        pair(tag_no_case("CREATE"), multispace1),
        map(
            separated_list1(
                delimited(multispace0, char(','), multispace0),
                alt((
                    map(parse_create_edge_pattern, |(from, edge, to)| (vec![from, to], vec![edge])),
                    map(parse_create_node_spec, |node| (vec![node], vec![])),
                )),
            ),
            |patterns| {
                let mut nodes = Vec::new();
                let mut edges = Vec::new();
                for (mut ns, mut es) in patterns {
                    nodes.append(&mut ns);
                    edges.append(&mut es);
                }
                CreateClause { nodes, edges }
            },
        ),
    )(input)
}

/// Parse DELETE clause
fn parse_delete_clause(input: &str) -> ParseResult<DeleteClause> {
    alt((
        // DETACH DELETE
        map(
            preceded(
                tuple((tag_no_case("DETACH"), multispace1, tag_no_case("DELETE"), multispace1)),
                separated_list1(delimited(multispace0, char(','), multispace0), any_identifier),
            ),
            |vars| DeleteClause {
                variables: vars,
                detach: true,
            },
        ),
        // DELETE
        map(
            preceded(
                pair(tag_no_case("DELETE"), multispace1),
                separated_list1(delimited(multispace0, char(','), multispace0), any_identifier),
            ),
            |vars| DeleteClause {
                variables: vars,
                detach: false,
            },
        ),
    ))(input)
}

/// Parse SET item
fn parse_set_item(input: &str) -> ParseResult<SetItem> {
    alt((
        // n.prop = value
        map(
            tuple((
                any_identifier,
                char('.'),
                any_identifier,
                delimited(multispace0, char('='), multispace0),
                property_value,
            )),
            |(var, _, prop, _, val)| SetItem::Property {
                variable: var,
                property: prop,
                value: val,
            },
        ),
        // n:Label
        map(
            tuple((any_identifier, char(':'), any_identifier)),
            |(var, _, label)| SetItem::AddLabel {
                variable: var,
                label,
            },
        ),
        // n += {props}
        map(
            tuple((
                any_identifier,
                delimited(multispace0, tag("+="), multispace0),
                property_value_map,
            )),
            |(var, _, props)| SetItem::MergeProperties {
                variable: var,
                properties: props,
            },
        ),
        // n = {props}
        map(
            tuple((
                any_identifier,
                delimited(multispace0, char('='), multispace0),
                property_value_map,
            )),
            |(var, _, props)| SetItem::AllProperties {
                variable: var,
                properties: props,
            },
        ),
    ))(input)
}

/// Parse SET clause
fn parse_set_clause(input: &str) -> ParseResult<SetClause> {
    preceded(
        pair(tag_no_case("SET"), multispace1),
        map(
            separated_list1(delimited(multispace0, char(','), multispace0), parse_set_item),
            |items| SetClause { items },
        ),
    )(input)
}

/// Parse REMOVE item
fn parse_remove_item(input: &str) -> ParseResult<RemoveItem> {
    alt((
        // n.prop
        map(
            tuple((any_identifier, char('.'), any_identifier)),
            |(var, _, prop)| RemoveItem::Property {
                variable: var,
                property: prop,
            },
        ),
        // n:Label
        map(
            tuple((any_identifier, char(':'), any_identifier)),
            |(var, _, label)| RemoveItem::Label {
                variable: var,
                label,
            },
        ),
    ))(input)
}

/// Parse REMOVE clause
fn parse_remove_clause(input: &str) -> ParseResult<RemoveClause> {
    preceded(
        pair(tag_no_case("REMOVE"), multispace1),
        map(
            separated_list1(delimited(multispace0, char(','), multispace0), parse_remove_item),
            |items| RemoveClause { items },
        ),
    )(input)
}

/// Parse MERGE clause
fn parse_merge_clause(input: &str) -> ParseResult<MergeClause> {
    map(
        tuple((
            preceded(
                pair(tag_no_case("MERGE"), multispace1),
                map(
                    separated_list1(
                        delimited(multispace0, char(','), multispace0),
                        alt((
                            map(parse_path_segment, |(from, edge, to)| (vec![from, to], vec![edge])),
                            map(parse_node_pattern, |node| (vec![node], vec![])),
                        )),
                    ),
                    |patterns| {
                        let mut nodes = Vec::new();
                        let mut edges = Vec::new();
                        for (mut ns, mut es) in patterns {
                            nodes.append(&mut ns);
                            edges.append(&mut es);
                        }
                        MatchPattern {
                            nodes,
                            edges,
                            paths: Vec::new(),
                            where_clause: None,
                        }
                    },
                ),
            ),
            opt(preceded(
                tuple((multispace1, tag_no_case("ON"), multispace1, tag_no_case("CREATE"), multispace1)),
                parse_set_clause,
            )),
            opt(preceded(
                tuple((multispace1, tag_no_case("ON"), multispace1, tag_no_case("MATCH"), multispace1)),
                parse_set_clause,
            )),
        )),
        |(pattern, on_create, on_match)| MergeClause {
            pattern,
            on_create,
            on_match,
        },
    )(input)
}

/// Parse WITH clause
fn parse_with_clause(input: &str) -> ParseResult<WithClause> {
    map(
        tuple((
            tag_no_case("WITH"),
            opt(preceded(multispace1, tag_no_case("DISTINCT"))),
            multispace1,
            separated_list1(
                delimited(multispace0, char(','), multispace0),
                parse_return_item,
            ),
            opt(preceded(multispace1, parse_order_by)),
            opt(preceded(multispace1, parse_skip)),
            opt(preceded(multispace1, parse_limit)),
            opt(preceded(multispace1, parse_where_clause)),
        )),
        |(_, distinct, _, items, order_by, skip, limit, where_clause)| WithClause {
            projections: items,
            distinct: distinct.is_some(),
            order_by: order_by.unwrap_or_default(),
            limit,
            skip,
            where_clause,
        },
    )(input)
}

/// Parse a complete Cypher query
fn parse_cypher_query(input: &str) -> ParseResult<CypherQuery> {
    map(
        tuple((
            // Reading clauses
            many0(preceded(
                multispace0,
                alt((
                    map(
                        tuple((
                            parse_optional_match_clause,
                            opt(preceded(multispace1, parse_where_clause)),
                        )),
                        |((nodes, edges), where_opt)| ReadingClause::Match {
                            pattern: MatchPattern {
                                nodes,
                                edges,
                                paths: Vec::new(),
                                where_clause: where_opt,
                            },
                            optional: true,
                        },
                    ),
                    map(
                        tuple((
                            parse_match_clause,
                            opt(preceded(multispace1, parse_where_clause)),
                        )),
                        |((nodes, edges), where_opt)| ReadingClause::Match {
                            pattern: MatchPattern {
                                nodes,
                                edges,
                                paths: Vec::new(),
                                where_clause: where_opt,
                            },
                            optional: false,
                        },
                    ),
                )),
            )),
            // WITH clauses
            many0(preceded(multispace0, parse_with_clause)),
            // Updating clauses
            many0(preceded(
                multispace0,
                alt((
                    map(parse_create_clause, UpdatingClause::Create),
                    map(parse_delete_clause, UpdatingClause::Delete),
                    map(parse_set_clause, UpdatingClause::Set),
                    map(parse_remove_clause, UpdatingClause::Remove),
                    map(parse_merge_clause, UpdatingClause::Merge),
                )),
            )),
            // Optional RETURN
            opt(preceded(multispace0, parse_return_clause)),
            multispace0,
        )),
        |(reading_clauses, with_clauses, updating_clauses, return_spec, _)| CypherQuery {
            reading_clauses,
            updating_clauses,
            with_clauses,
            return_spec,
        },
    )(input)
}

/// Parse simple MATCH query into CompiledPattern
fn parse_simple_query(input: &str) -> ParseResult<CompiledPattern> {
    map(
        tuple((
            parse_match_clause,
            multispace0,
            opt(parse_where_clause),
            multispace0,
            parse_return_clause,
            multispace0,
        )),
        |((nodes, edges), _, where_opt, _, return_spec, _)| CompiledPattern {
            nodes,
            edges,
            paths: Vec::new(),
            where_clauses: where_opt.into_iter().collect(),
            return_spec,
            variables: HashMap::new(),
        },
    )(input)
}

// ============================================================================
// AST VISITOR PATTERN
// ============================================================================

/// Visitor trait for CypherQuery AST
pub trait CypherVisitor {
    type Output;

    fn visit_query(&mut self, query: &CypherQuery) -> Self::Output;
    fn visit_reading_clause(&mut self, clause: &ReadingClause) -> Self::Output;
    fn visit_updating_clause(&mut self, clause: &UpdatingClause) -> Self::Output;
    fn visit_node_pattern(&mut self, pattern: &NodePattern) -> Self::Output;
    fn visit_edge_pattern(&mut self, pattern: &EdgePattern) -> Self::Output;
    fn visit_where_clause(&mut self, clause: &WhereClause) -> Self::Output;
    fn visit_return_spec(&mut self, spec: &ReturnSpec) -> Self::Output;
}

/// Query validator visitor
pub struct QueryValidator {
    errors: Vec<String>,
    variables: std::collections::HashSet<String>,
}

impl QueryValidator {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            variables: std::collections::HashSet::new(),
        }
    }

    pub fn validate(query: &CypherQuery) -> Result<(), Vec<String>> {
        let mut validator = Self::new();
        validator.visit_query(query);
        if validator.errors.is_empty() {
            Ok(())
        } else {
            Err(validator.errors)
        }
    }
}

impl Default for QueryValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl CypherVisitor for QueryValidator {
    type Output = ();

    fn visit_query(&mut self, query: &CypherQuery) {
        // Visit all reading clauses
        for clause in &query.reading_clauses {
            self.visit_reading_clause(clause);
        }

        // Visit all updating clauses
        for clause in &query.updating_clauses {
            self.visit_updating_clause(clause);
        }

        // Validate return spec references defined variables
        if let Some(ref return_spec) = query.return_spec {
            self.visit_return_spec(return_spec);
        }
    }

    fn visit_reading_clause(&mut self, clause: &ReadingClause) {
        match clause {
            ReadingClause::Match { pattern, .. } => {
                for node in &pattern.nodes {
                    self.visit_node_pattern(node);
                }
                for edge in &pattern.edges {
                    self.visit_edge_pattern(edge);
                }
                if let Some(ref where_clause) = pattern.where_clause {
                    self.visit_where_clause(where_clause);
                }
            }
            ReadingClause::Unwind { variable, .. } => {
                self.variables.insert(variable.clone());
            }
            ReadingClause::Call { yield_items, .. } => {
                for item in yield_items {
                    self.variables.insert(item.clone());
                }
            }
        }
    }

    fn visit_updating_clause(&mut self, clause: &UpdatingClause) {
        match clause {
            UpdatingClause::Create(create) => {
                for node in &create.nodes {
                    if let Some(ref var) = node.variable {
                        self.variables.insert(var.clone());
                    }
                }
                for edge in &create.edges {
                    if let Some(ref var) = edge.variable {
                        self.variables.insert(var.clone());
                    }
                }
            }
            UpdatingClause::Delete(delete) => {
                for var in &delete.variables {
                    if !self.variables.contains(var) {
                        self.errors.push(format!("DELETE references undefined variable: {}", var));
                    }
                }
            }
            UpdatingClause::Set(set) => {
                for item in &set.items {
                    let var = match item {
                        SetItem::Property { variable, .. } => variable,
                        SetItem::AllProperties { variable, .. } => variable,
                        SetItem::MergeProperties { variable, .. } => variable,
                        SetItem::AddLabel { variable, .. } => variable,
                    };
                    if !self.variables.contains(var) {
                        self.errors.push(format!("SET references undefined variable: {}", var));
                    }
                }
            }
            UpdatingClause::Remove(remove) => {
                for item in &remove.items {
                    let var = match item {
                        RemoveItem::Property { variable, .. } => variable,
                        RemoveItem::Label { variable, .. } => variable,
                    };
                    if !self.variables.contains(var) {
                        self.errors.push(format!("REMOVE references undefined variable: {}", var));
                    }
                }
            }
            UpdatingClause::Merge(merge) => {
                for node in &merge.pattern.nodes {
                    self.visit_node_pattern(node);
                }
                for edge in &merge.pattern.edges {
                    self.visit_edge_pattern(edge);
                }
            }
            UpdatingClause::ForEach(foreach) => {
                self.variables.insert(foreach.variable.clone());
            }
        }
    }

    fn visit_node_pattern(&mut self, pattern: &NodePattern) {
        self.variables.insert(pattern.variable.clone());
    }

    fn visit_edge_pattern(&mut self, pattern: &EdgePattern) {
        if let Some(ref var) = pattern.variable {
            self.variables.insert(var.clone());
        }
    }

    fn visit_where_clause(&mut self, clause: &WhereClause) {
        match clause {
            WhereClause::Property { variable, .. } => {
                if !self.variables.contains(variable) {
                    self.errors.push(format!("WHERE references undefined variable: {}", variable));
                }
            }
            WhereClause::And(left, right) | WhereClause::Or(left, right) => {
                self.visit_where_clause(left);
                self.visit_where_clause(right);
            }
            WhereClause::Not(inner) => {
                self.visit_where_clause(inner);
            }
        }
    }

    fn visit_return_spec(&mut self, spec: &ReturnSpec) {
        for projection in &spec.projections {
            let var = match projection {
                PropertyProjection::Variable(v) => v,
                PropertyProjection::Property { variable, .. } => variable,
                PropertyProjection::Sum { variable, .. } => variable,
                PropertyProjection::Avg { variable, .. } => variable,
                PropertyProjection::Min { variable, .. } => variable,
                PropertyProjection::Max { variable, .. } => variable,
                PropertyProjection::Count => continue,
            };
            if !self.variables.contains(var) {
                self.errors.push(format!("RETURN references undefined variable: {}", var));
            }
        }
    }
}

// ============================================================================
// GRAPH QUERY CONVERSION
// ============================================================================

/// GraphQuery represents an executable graph query
#[derive(Debug, Clone)]
pub struct GraphQuery {
    /// Graph ID to execute against
    pub graph_id: String,
    /// Query type
    pub query_type: GraphQueryType,
    /// Query parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Timeout in milliseconds
    pub timeout_ms: Option<u64>,
}

/// Types of graph queries
#[derive(Debug, Clone)]
pub enum GraphQueryType {
    /// Read-only pattern match
    Match {
        patterns: Vec<MatchPattern>,
        return_spec: ReturnSpec,
    },
    /// Create nodes and edges
    Create {
        nodes: Vec<CreateNodeSpec>,
        edges: Vec<CreateEdgeSpec>,
        return_spec: Option<ReturnSpec>,
    },
    /// Merge (create if not exists)
    Merge {
        pattern: MatchPattern,
        on_create: Option<SetClause>,
        on_match: Option<SetClause>,
        return_spec: Option<ReturnSpec>,
    },
    /// Delete nodes and edges
    Delete {
        variables: Vec<String>,
        detach: bool,
        patterns: Vec<MatchPattern>,
    },
    /// Update properties
    Update {
        patterns: Vec<MatchPattern>,
        set_clause: SetClause,
        return_spec: Option<ReturnSpec>,
    },
}

/// Convert CypherQuery AST to executable GraphQuery
pub fn cypher_to_graph_query(cypher: &CypherQuery, graph_id: &str) -> Result<GraphQuery, ProximaDBError> {
    // Determine query type based on clauses
    let query_type = if !cypher.updating_clauses.is_empty() {
        // Has updating clauses
        let first_update = &cypher.updating_clauses[0];
        match first_update {
            UpdatingClause::Create(create) => GraphQueryType::Create {
                nodes: create.nodes.clone(),
                edges: create.edges.clone(),
                return_spec: cypher.return_spec.clone(),
            },
            UpdatingClause::Merge(merge) => GraphQueryType::Merge {
                pattern: merge.pattern.clone(),
                on_create: merge.on_create.clone(),
                on_match: merge.on_match.clone(),
                return_spec: cypher.return_spec.clone(),
            },
            UpdatingClause::Delete(delete) => {
                let patterns: Vec<MatchPattern> = cypher
                    .reading_clauses
                    .iter()
                    .filter_map(|c| match c {
                        ReadingClause::Match { pattern, .. } => Some(pattern.clone()),
                        _ => None,
                    })
                    .collect();
                GraphQueryType::Delete {
                    variables: delete.variables.clone(),
                    detach: delete.detach,
                    patterns,
                }
            }
            UpdatingClause::Set(set) => {
                let patterns: Vec<MatchPattern> = cypher
                    .reading_clauses
                    .iter()
                    .filter_map(|c| match c {
                        ReadingClause::Match { pattern, .. } => Some(pattern.clone()),
                        _ => None,
                    })
                    .collect();
                GraphQueryType::Update {
                    patterns,
                    set_clause: set.clone(),
                    return_spec: cypher.return_spec.clone(),
                }
            }
            _ => {
                return Err(ProximaDBError::InvalidInput(
                    "Unsupported updating clause type".to_string(),
                ));
            }
        }
    } else {
        // Read-only query
        let patterns: Vec<MatchPattern> = cypher
            .reading_clauses
            .iter()
            .filter_map(|c| match c {
                ReadingClause::Match { pattern, .. } => Some(pattern.clone()),
                _ => None,
            })
            .collect();

        if patterns.is_empty() {
            return Err(ProximaDBError::InvalidInput(
                "Query must have at least one MATCH clause".to_string(),
            ));
        }

        let return_spec = cypher
            .return_spec
            .clone()
            .ok_or_else(|| ProximaDBError::InvalidInput("Read query must have RETURN clause".to_string()))?;

        GraphQueryType::Match {
            patterns,
            return_spec,
        }
    };

    Ok(GraphQuery {
        graph_id: graph_id.to_string(),
        query_type,
        parameters: HashMap::new(),
        timeout_ms: None,
    })
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer_basic() {
        let mut lexer = CypherLexer::new();
        let tokens = lexer.tokenize("MATCH (n) RETURN n").unwrap();
        assert!(tokens.len() >= 5);
        assert!(matches!(tokens[0].token, Token::Match));
    }

    #[test]
    fn test_lexer_string_literals() {
        let mut lexer = CypherLexer::new();
        let tokens = lexer.tokenize("'hello' \"world\"").unwrap();
        assert!(matches!(&tokens[0].token, Token::String(s) if s == "hello"));
        assert!(matches!(&tokens[1].token, Token::String(s) if s == "world"));
    }

    #[test]
    fn test_lexer_numbers() {
        let mut lexer = CypherLexer::new();
        let tokens = lexer.tokenize("42 3.14 -5").unwrap();
        assert!(matches!(tokens[0].token, Token::Integer(42)));
        assert!(matches!(tokens[1].token, Token::Float(f) if (f - 3.14).abs() < 0.001));
        assert!(matches!(tokens[2].token, Token::Integer(-5)));
    }

    #[test]
    fn test_parse_simple_match() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) RETURN n").unwrap();
        assert_eq!(query.reading_clauses.len(), 1);
        assert!(query.is_read_only());
    }

    #[test]
    fn test_parse_match_with_properties() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person {name: 'Alice', age: 30}) RETURN n").unwrap();
        assert_eq!(query.reading_clauses.len(), 1);
    }

    #[test]
    fn test_parse_match_with_edge() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a, r, b").unwrap();
        assert_eq!(query.reading_clauses.len(), 1);
        if let ReadingClause::Match { pattern, .. } = &query.reading_clauses[0] {
            assert_eq!(pattern.nodes.len(), 2);
            assert_eq!(pattern.edges.len(), 1);
        }
    }

    #[test]
    fn test_parse_where_clause() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) WHERE n.age > 25 RETURN n").unwrap();
        if let ReadingClause::Match { pattern, .. } = &query.reading_clauses[0] {
            assert!(pattern.where_clause.is_some());
        }
    }

    #[test]
    fn test_parse_where_and() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) WHERE n.age > 25 AND n.name = 'Alice' RETURN n").unwrap();
        if let ReadingClause::Match { pattern, .. } = &query.reading_clauses[0] {
            assert!(pattern.where_clause.is_some());
            if let Some(WhereClause::And(_, _)) = &pattern.where_clause {
                // Success
            } else {
                panic!("Expected AND clause");
            }
        }
    }

    #[test]
    fn test_parse_create() {
        let parser = CypherParser::new();
        let query = parser.parse("CREATE (n:Person {name: 'Alice'}) RETURN n").unwrap();
        assert!(!query.is_read_only());
        assert!(query.has_create());
    }

    #[test]
    fn test_parse_delete() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) WHERE n.age > 100 DELETE n").unwrap();
        assert!(query.has_delete());
    }

    #[test]
    fn test_parse_set() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person {name: 'Alice'}) SET n.age = 31 RETURN n").unwrap();
        assert!(!query.is_read_only());
    }

    #[test]
    fn test_parse_merge() {
        let parser = CypherParser::new();
        let query = parser.parse("MERGE (n:Person {name: 'Alice'}) RETURN n").unwrap();
        assert!(query.has_merge());
    }

    #[test]
    fn test_parse_aggregation() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) RETURN COUNT(*)").unwrap();
        if let Some(ref return_spec) = query.return_spec {
            assert!(matches!(return_spec.projections[0], PropertyProjection::Count));
        }
    }

    #[test]
    fn test_parse_order_limit() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) RETURN n ORDER BY n ASC LIMIT 10").unwrap();
        if let Some(ref return_spec) = query.return_spec {
            assert_eq!(return_spec.order_by.len(), 1);
            assert_eq!(return_spec.limit, Some(10));
        }
    }

    #[test]
    fn test_validator() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) RETURN n").unwrap();
        assert!(QueryValidator::validate(&query).is_ok());
    }

    #[test]
    fn test_conversion_to_graph_query() {
        let parser = CypherParser::new();
        let cypher = parser.parse("MATCH (n:Person) RETURN n").unwrap();
        let graph_query = cypher_to_graph_query(&cypher, "test_graph").unwrap();
        assert_eq!(graph_query.graph_id, "test_graph");
        assert!(matches!(graph_query.query_type, GraphQueryType::Match { .. }));
    }

    #[test]
    fn test_parse_optional_match() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person) OPTIONAL MATCH (n)-[r:KNOWS]->(m) RETURN n, m").unwrap();
        assert_eq!(query.reading_clauses.len(), 2);
        if let ReadingClause::Match { optional, .. } = &query.reading_clauses[1] {
            assert!(*optional);
        }
    }

    #[test]
    fn test_parse_multiple_labels() {
        let parser = CypherParser::new();
        let query = parser.parse("MATCH (n:Person:Employee) RETURN n").unwrap();
        if let ReadingClause::Match { pattern, .. } = &query.reading_clauses[0] {
            assert_eq!(pattern.nodes[0].labels.len(), 2);
        }
    }
}
