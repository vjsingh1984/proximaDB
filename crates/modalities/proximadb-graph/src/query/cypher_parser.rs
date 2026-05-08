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

//! # Recursive-Descent Cypher Parser
//!
//! A hand-written tokenizer and recursive-descent parser for the Cypher query language.
//! Produces a [`CypherStatement`] AST that can be consumed by the query planner.
//!
//! ## Supported Clauses
//!
//! MATCH, OPTIONAL MATCH, WHERE, RETURN (DISTINCT), ORDER BY, LIMIT, SKIP,
//! CREATE, SET, DELETE (DETACH), WITH, UNION (ALL).
//!
//! ## Supported Expressions
//!
//! Literals, variables, property access, binary/unary operators, comparisons,
//! function calls (COUNT, SUM, AVG, MIN, MAX, COLLECT, etc.), parameters, lists.

use super::cypher_ast::*;
use anyhow::{Context, Result, bail};

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// Token kinds produced by the tokenizer.
#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    // Literals
    Integer(i64),
    Float(f64),
    StringLit(String),
    BoolTrue,
    BoolFalse,
    Null,

    // Identifiers & keywords
    Ident(String),

    // Keywords (uppercased during tokenization for case-insensitive matching)
    Match,
    Optional,
    Where,
    Return,
    Order,
    By,
    Limit,
    Skip,
    Create,
    Set,
    Delete,
    Detach,
    With,
    Union,
    All,
    Distinct,
    As,
    And,
    Or,
    Xor,
    Not,
    In,
    Is,
    Contains,
    StartsWith,
    EndsWith,
    Asc,
    Desc,
    Ascending,
    Descending,
    Unwind,
    Reduce,

    // Punctuation / operators
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Dot,
    Pipe,
    Star,
    Plus,
    Minus,
    Slash,
    Percent,
    Eq,      // =
    Neq,     // <>
    Lt,      // <
    Gt,      // >
    Lte,     // <=
    Gte,     // >=
    RegexOp, // =~
    Arrow,   // ->
    LArrow,  // <-
    DotDot,  // ..
    Dollar,  // $

    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    pos: usize,
}

/// Tokenize a Cypher query string into a sequence of tokens.
fn tokenize(input: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut tokens = Vec::new();

    while i < len {
        // Skip whitespace
        if chars[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Skip line comments
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        let pos = i;

        // String literals (single or double quoted)
        if chars[i] == '\'' || chars[i] == '"' {
            let quote = chars[i];
            i += 1;
            let mut s = String::new();
            while i < len && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < len {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        c if c == quote => s.push(c),
                        c => {
                            s.push('\\');
                            s.push(c);
                        }
                    }
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= len {
                bail!("Unterminated string literal starting at position {pos}");
            }
            i += 1; // closing quote
            tokens.push(Token {
                kind: TokenKind::StringLit(s),
                pos,
            });
            continue;
        }

        // Numbers
        if chars[i].is_ascii_digit()
            || (chars[i] == '-'
                && i + 1 < len
                && chars[i + 1].is_ascii_digit()
                && (tokens.is_empty()
                    || matches!(
                        tokens.last().map(|t| &t.kind),
                        Some(
                            TokenKind::LParen
                                | TokenKind::Comma
                                | TokenKind::Colon
                                | TokenKind::Eq
                                | TokenKind::Neq
                                | TokenKind::Lt
                                | TokenKind::Gt
                                | TokenKind::Lte
                                | TokenKind::Gte
                        )
                    )))
        {
            let start = i;
            if chars[i] == '-' {
                i += 1;
            }
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i < len && chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit() {
                i += 1;
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let val: f64 = input[start..i]
                    .parse()
                    .with_context(|| format!("Invalid float at position {start}"))?;
                tokens.push(Token {
                    kind: TokenKind::Float(val),
                    pos,
                });
            } else {
                let val: i64 = input[start..i]
                    .parse()
                    .with_context(|| format!("Invalid integer at position {start}"))?;
                tokens.push(Token {
                    kind: TokenKind::Integer(val),
                    pos,
                });
            }
            continue;
        }

        // Identifiers and keywords
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word = &input[start..i];
            let kind = match word.to_ascii_uppercase().as_str() {
                "MATCH" => TokenKind::Match,
                "OPTIONAL" => TokenKind::Optional,
                "WHERE" => TokenKind::Where,
                "RETURN" => TokenKind::Return,
                "ORDER" => TokenKind::Order,
                "BY" => TokenKind::By,
                "LIMIT" => TokenKind::Limit,
                "SKIP" => TokenKind::Skip,
                "CREATE" => TokenKind::Create,
                "SET" => TokenKind::Set,
                "DELETE" => TokenKind::Delete,
                "DETACH" => TokenKind::Detach,
                "WITH" => TokenKind::With,
                "UNION" => TokenKind::Union,
                "ALL" => TokenKind::All,
                "DISTINCT" => TokenKind::Distinct,
                "AS" => TokenKind::As,
                "AND" => TokenKind::And,
                "OR" => TokenKind::Or,
                "XOR" => TokenKind::Xor,
                "NOT" => TokenKind::Not,
                "IN" => TokenKind::In,
                "IS" => TokenKind::Is,
                "CONTAINS" => TokenKind::Contains,
                "STARTS" => TokenKind::StartsWith,
                "ENDS" => TokenKind::EndsWith,
                "ASC" => TokenKind::Asc,
                "DESC" => TokenKind::Desc,
                "ASCENDING" => TokenKind::Ascending,
                "DESCENDING" => TokenKind::Descending,
                "UNWIND" => TokenKind::Unwind,
                "REDUCE" => TokenKind::Reduce,
                "TRUE" => TokenKind::BoolTrue,
                "FALSE" => TokenKind::BoolFalse,
                "NULL" => TokenKind::Null,
                _ => TokenKind::Ident(word.to_string()),
            };
            tokens.push(Token { kind, pos });
            continue;
        }

        // Multi-char operators
        if i + 1 < len {
            let two = &input[i..i + 2];
            match two {
                "->" => {
                    tokens.push(Token {
                        kind: TokenKind::Arrow,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                "<-" => {
                    tokens.push(Token {
                        kind: TokenKind::LArrow,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                "<>" => {
                    tokens.push(Token {
                        kind: TokenKind::Neq,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                "<=" => {
                    tokens.push(Token {
                        kind: TokenKind::Lte,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                ">=" => {
                    tokens.push(Token {
                        kind: TokenKind::Gte,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                "=~" => {
                    tokens.push(Token {
                        kind: TokenKind::RegexOp,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                ".." => {
                    tokens.push(Token {
                        kind: TokenKind::DotDot,
                        pos,
                    });
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // Single-char tokens
        let kind = match chars[i] {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            '|' => TokenKind::Pipe,
            '*' => TokenKind::Star,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '=' => TokenKind::Eq,
            '<' => TokenKind::Lt,
            '>' => TokenKind::Gt,
            '$' => TokenKind::Dollar,
            c => bail!("Unexpected character '{}' at position {}", c, i),
        };
        tokens.push(Token { kind, pos });
        i += 1;
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        pos: len,
    });
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Recursive-descent Cypher parser.
pub struct CypherParser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl CypherParser {
    /// Parse a Cypher query string into a [`CypherStatement`].
    pub fn parse(input: &str) -> Result<CypherStatement> {
        let tokens = tokenize(input)?;
        let mut parser = CypherParser { tokens, cursor: 0 };
        parser.parse_statement()
    }

    // -- helpers ----------------------------------------------------------

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.cursor].kind
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.cursor];
        if self.cursor + 1 < self.tokens.len() {
            self.cursor += 1;
        }
        tok
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<()> {
        if !self.at(expected) {
            bail!(
                "Expected {:?} but found {:?} at position {}",
                expected,
                self.peek(),
                self.tokens[self.cursor].pos
            );
        }
        self.advance();
        Ok(())
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            TokenKind::Ident(s) => {
                self.advance();
                Ok(s)
            }
            other => bail!(
                "Expected identifier but found {:?} at position {}",
                other,
                self.tokens[self.cursor].pos
            ),
        }
    }

    #[allow(dead_code)]
    fn at_keyword_ident(&self) -> bool {
        // Some keywords can appear as identifiers in certain contexts (e.g., property keys).
        matches!(
            self.peek(),
            TokenKind::Ident(_)
                | TokenKind::Order
                | TokenKind::By
                | TokenKind::Asc
                | TokenKind::Desc
                | TokenKind::Ascending
                | TokenKind::Descending
                | TokenKind::All
                | TokenKind::Set
                | TokenKind::In
                | TokenKind::Is
                | TokenKind::Contains
                | TokenKind::StartsWith
                | TokenKind::EndsWith
                | TokenKind::Not
        )
    }

    #[allow(dead_code)]
    fn consume_ident_or_keyword(&mut self) -> Result<String> {
        let tok = self.peek().clone();
        match tok {
            TokenKind::Ident(s) => {
                self.advance();
                Ok(s)
            }
            // Allow reserved words as property names / labels
            _ if self.at_keyword_ident() => {
                let name = format!("{:?}", self.peek());
                self.advance();
                // Return a cleaned-up version
                Ok(name)
            }
            _ => self.expect_ident(),
        }
    }

    /// Consume an identifier, also accepting keywords that are valid as names.
    fn consume_name(&mut self) -> Result<String> {
        match self.peek().clone() {
            TokenKind::Ident(s) => {
                self.advance();
                Ok(s)
            }
            // Keywords that can also be used as variable/label/property names
            TokenKind::Order => {
                self.advance();
                Ok("order".into())
            }
            TokenKind::By => {
                self.advance();
                Ok("by".into())
            }
            TokenKind::Asc => {
                self.advance();
                Ok("asc".into())
            }
            TokenKind::Desc => {
                self.advance();
                Ok("desc".into())
            }
            TokenKind::Ascending => {
                self.advance();
                Ok("ascending".into())
            }
            TokenKind::Descending => {
                self.advance();
                Ok("descending".into())
            }
            TokenKind::All => {
                self.advance();
                Ok("all".into())
            }
            TokenKind::Set => {
                self.advance();
                Ok("set".into())
            }
            TokenKind::Contains => {
                self.advance();
                Ok("contains".into())
            }
            _ => self.expect_ident(),
        }
    }

    // -- statement --------------------------------------------------------

    fn parse_statement(&mut self) -> Result<CypherStatement> {
        let mut clauses = Vec::new();

        while !self.at(&TokenKind::Eof) {
            match self.peek() {
                TokenKind::Match => {
                    self.advance();
                    let mc = self.parse_match_clause()?;
                    clauses.push(CypherClause::Match(mc));
                }
                TokenKind::Optional => {
                    self.advance();
                    self.expect(&TokenKind::Match)?;
                    let mc = self.parse_match_clause()?;
                    clauses.push(CypherClause::OptionalMatch(mc));
                }
                TokenKind::Where => {
                    self.advance();
                    let wc = self.parse_where_clause()?;
                    clauses.push(CypherClause::Where(wc));
                }
                TokenKind::Return => {
                    self.advance();
                    let rc = self.parse_return_clause()?;
                    clauses.push(CypherClause::Return(rc));
                }
                TokenKind::Order => {
                    self.advance();
                    self.expect(&TokenKind::By)?;
                    let ob = self.parse_order_by_clause()?;
                    clauses.push(CypherClause::OrderBy(ob));
                }
                TokenKind::Limit => {
                    self.advance();
                    let lc = self.parse_limit_clause()?;
                    clauses.push(CypherClause::Limit(lc));
                }
                TokenKind::Skip => {
                    self.advance();
                    let sc = self.parse_skip_clause()?;
                    clauses.push(CypherClause::Skip(sc));
                }
                TokenKind::Create => {
                    self.advance();
                    let cc = self.parse_create_clause()?;
                    clauses.push(CypherClause::Create(cc));
                }
                TokenKind::Set => {
                    self.advance();
                    let sc = self.parse_set_clause()?;
                    clauses.push(CypherClause::Set(sc));
                }
                TokenKind::Detach => {
                    self.advance();
                    self.expect(&TokenKind::Delete)?;
                    let dc = self.parse_delete_clause(true)?;
                    clauses.push(CypherClause::Delete(dc));
                }
                TokenKind::Delete => {
                    self.advance();
                    let dc = self.parse_delete_clause(false)?;
                    clauses.push(CypherClause::Delete(dc));
                }
                TokenKind::With => {
                    self.advance();
                    let wc = self.parse_with_clause()?;
                    clauses.push(CypherClause::With(wc));
                }
                TokenKind::Unwind => {
                    self.advance();
                    let uc = self.parse_unwind_clause()?;
                    clauses.push(CypherClause::Unwind(uc));
                }
                TokenKind::Union => {
                    self.advance();
                    let all = self.at(&TokenKind::All);
                    if all {
                        self.advance();
                    }
                    clauses.push(CypherClause::Union(UnionClause { all }));
                }
                other => {
                    bail!(
                        "Unexpected token {:?} at position {}",
                        other,
                        self.tokens[self.cursor].pos
                    );
                }
            }
        }

        if clauses.is_empty() {
            bail!("Empty query");
        }

        Ok(CypherStatement { clauses })
    }

    // -- MATCH ------------------------------------------------------------

    fn parse_match_clause(&mut self) -> Result<MatchClause> {
        let mut patterns = Vec::new();
        patterns.push(self.parse_pattern_path()?);
        while self.at(&TokenKind::Comma) {
            self.advance();
            patterns.push(self.parse_pattern_path()?);
        }
        Ok(MatchClause { patterns })
    }

    fn parse_pattern_path(&mut self) -> Result<PatternPath> {
        let mut elements = Vec::new();
        elements.push(PatternElement::Node(self.parse_node_pattern()?));

        #[expect(clippy::never_loop)] // Pattern with or-patterns makes while let awkward
        loop {
            // Check for relationship: -, <-
            match self.peek() {
                TokenKind::Minus | TokenKind::LArrow => {
                    let rel = self.parse_rel_pattern()?;
                    elements.push(PatternElement::Relationship(rel));
                    elements.push(PatternElement::Node(self.parse_node_pattern()?));
                }
                _ => break,
            }
        }

        Ok(PatternPath { elements })
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern> {
        self.expect(&TokenKind::LParen)?;

        let mut variable = None;
        let mut labels = Vec::new();
        let mut properties = Vec::new();

        // Variable name (optional) — must come before colon/brace/rparen
        if let TokenKind::Ident(_) = self.peek() {
            variable = Some(self.expect_ident()?);
        }

        // Labels
        while self.at(&TokenKind::Colon) {
            self.advance();
            labels.push(self.consume_name()?);
        }

        // Properties
        if self.at(&TokenKind::LBrace) {
            properties = self.parse_property_map()?;
        }

        self.expect(&TokenKind::RParen)?;

        Ok(NodePattern {
            variable,
            labels,
            properties,
        })
    }

    fn parse_rel_pattern(&mut self) -> Result<RelPattern> {
        // Determine direction and parse bracket contents
        // Patterns: -[...]->  <-[...]-  -[...]-
        let left_arrow = self.at(&TokenKind::LArrow);
        if left_arrow {
            self.advance(); // consume <-
        } else {
            self.expect(&TokenKind::Minus)?;
        }

        let mut variable = None;
        let mut rel_types = Vec::new();
        let mut properties = Vec::new();
        let mut range = None;

        // Optional bracket section
        if self.at(&TokenKind::LBracket) {
            self.advance();

            // Variable
            if let TokenKind::Ident(_) = self.peek() {
                variable = Some(self.expect_ident()?);
            }

            // Relationship types
            if self.at(&TokenKind::Colon) {
                self.advance();
                rel_types.push(self.consume_name()?);
                while self.at(&TokenKind::Pipe) {
                    self.advance();
                    rel_types.push(self.consume_name()?);
                }
            }

            // Properties
            if self.at(&TokenKind::LBrace) {
                properties = self.parse_property_map()?;
            }

            // Variable-length: *min..max
            if self.at(&TokenKind::Star) {
                self.advance();
                let min = if let TokenKind::Integer(n) = self.peek() {
                    let v = *n as u32;
                    self.advance();
                    Some(v)
                } else {
                    None
                };
                let max = if self.at(&TokenKind::DotDot) {
                    self.advance();
                    if let TokenKind::Integer(n) = self.peek() {
                        let v = *n as u32;
                        self.advance();
                        Some(v)
                    } else {
                        None
                    }
                } else {
                    // If no .., max = min (exact length)
                    min
                };
                range = Some((min, max));
            }

            self.expect(&TokenKind::RBracket)?;
        }

        // Determine direction based on arrows
        let direction = if left_arrow {
            // <-[...]- consumed so far. Check for -> to make it <-[]->  (Both)
            if self.at(&TokenKind::Minus) {
                self.advance();
                Direction::Left
            } else {
                Direction::Left
            }
        } else {
            // -[...]  now need -> or -
            if self.at(&TokenKind::Arrow) {
                self.advance();
                Direction::Right
            } else if self.at(&TokenKind::Minus) {
                self.advance();
                Direction::Both
            } else {
                bail!(
                    "Expected -> or - after relationship pattern at position {}",
                    self.tokens[self.cursor].pos
                );
            }
        };

        Ok(RelPattern {
            variable,
            rel_types,
            direction,
            properties,
            range,
        })
    }

    fn parse_property_map(&mut self) -> Result<Vec<(String, CypherValue)>> {
        self.expect(&TokenKind::LBrace)?;
        let mut props = Vec::new();

        if !self.at(&TokenKind::RBrace) {
            loop {
                let key = self.consume_name()?;
                self.expect(&TokenKind::Colon)?;
                let value = self.parse_cypher_value()?;
                props.push((key, value));
                if !self.at(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(props)
    }

    fn parse_cypher_value(&mut self) -> Result<CypherValue> {
        match self.peek().clone() {
            TokenKind::Integer(n) => {
                self.advance();
                Ok(CypherValue::Integer(n))
            }
            TokenKind::Float(f) => {
                self.advance();
                Ok(CypherValue::Float(f))
            }
            TokenKind::StringLit(s) => {
                self.advance();
                Ok(CypherValue::String(s))
            }
            TokenKind::BoolTrue => {
                self.advance();
                Ok(CypherValue::Boolean(true))
            }
            TokenKind::BoolFalse => {
                self.advance();
                Ok(CypherValue::Boolean(false))
            }
            TokenKind::Null => {
                self.advance();
                Ok(CypherValue::Null)
            }
            TokenKind::LBracket => {
                self.advance();
                let mut items = Vec::new();
                if !self.at(&TokenKind::RBracket) {
                    loop {
                        items.push(self.parse_cypher_value()?);
                        if !self.at(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(CypherValue::List(items))
            }
            TokenKind::LBrace => {
                let props = self.parse_property_map()?;
                Ok(CypherValue::Map(props))
            }
            other => bail!(
                "Expected value but found {:?} at position {}",
                other,
                self.tokens[self.cursor].pos
            ),
        }
    }

    // -- WHERE ------------------------------------------------------------

    fn parse_where_clause(&mut self) -> Result<WhereClause> {
        let expression = self.parse_expression()?;
        Ok(WhereClause { expression })
    }

    // -- RETURN -----------------------------------------------------------

    fn parse_return_clause(&mut self) -> Result<ReturnClause> {
        let distinct = self.at(&TokenKind::Distinct);
        if distinct {
            self.advance();
        }
        let items = self.parse_return_items()?;
        Ok(ReturnClause { distinct, items })
    }

    fn parse_return_items(&mut self) -> Result<Vec<ReturnItem>> {
        let mut items = Vec::new();
        loop {
            let expression = self.parse_expression()?;
            let alias = if self.at(&TokenKind::As) {
                self.advance();
                Some(self.consume_name()?)
            } else {
                None
            };
            items.push(ReturnItem { expression, alias });
            if !self.at(&TokenKind::Comma) {
                break;
            }
            self.advance();
        }
        Ok(items)
    }

    // -- ORDER BY ---------------------------------------------------------

    fn parse_order_by_clause(&mut self) -> Result<OrderByClause> {
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expression()?;
            let dir = match self.peek() {
                TokenKind::Desc | TokenKind::Descending => {
                    self.advance();
                    SortDirection::Desc
                }
                TokenKind::Asc | TokenKind::Ascending => {
                    self.advance();
                    SortDirection::Asc
                }
                _ => SortDirection::Asc,
            };
            items.push((expr, dir));
            if !self.at(&TokenKind::Comma) {
                break;
            }
            self.advance();
        }
        Ok(OrderByClause { items })
    }

    // -- LIMIT / SKIP -----------------------------------------------------

    fn parse_limit_clause(&mut self) -> Result<LimitClause> {
        match self.peek().clone() {
            TokenKind::Integer(n) if n >= 0 => {
                self.advance();
                Ok(LimitClause { count: n as u64 })
            }
            other => bail!("Expected positive integer for LIMIT but found {:?}", other),
        }
    }

    fn parse_skip_clause(&mut self) -> Result<SkipClause> {
        match self.peek().clone() {
            TokenKind::Integer(n) if n >= 0 => {
                self.advance();
                Ok(SkipClause { count: n as u64 })
            }
            other => bail!("Expected positive integer for SKIP but found {:?}", other),
        }
    }

    // -- CREATE -----------------------------------------------------------

    fn parse_create_clause(&mut self) -> Result<CreateClause> {
        let mut patterns = Vec::new();
        patterns.push(self.parse_pattern_path()?);
        while self.at(&TokenKind::Comma) {
            self.advance();
            patterns.push(self.parse_pattern_path()?);
        }
        Ok(CreateClause { patterns })
    }

    // -- SET --------------------------------------------------------------

    fn parse_set_clause(&mut self) -> Result<SetClause> {
        let mut items = Vec::new();
        loop {
            let var = self.consume_name()?;
            self.expect(&TokenKind::Dot)?;
            let prop = self.consume_name()?;
            self.expect(&TokenKind::Eq)?;
            let value = self.parse_expression()?;
            items.push(SetItem::Property {
                variable: var,
                property: prop,
                value,
            });
            if !self.at(&TokenKind::Comma) {
                break;
            }
            self.advance();
        }
        Ok(SetClause { items })
    }

    // -- DELETE -----------------------------------------------------------

    fn parse_delete_clause(&mut self, detach: bool) -> Result<DeleteClause> {
        let mut expressions = Vec::new();
        loop {
            expressions.push(self.parse_expression()?);
            if !self.at(&TokenKind::Comma) {
                break;
            }
            self.advance();
        }
        Ok(DeleteClause {
            detach,
            expressions,
        })
    }

    // -- WITH -------------------------------------------------------------

    fn parse_with_clause(&mut self) -> Result<WithClause> {
        let items = self.parse_return_items()?;
        let where_clause = if self.at(&TokenKind::Where) {
            self.advance();
            Some(self.parse_where_clause()?)
        } else {
            None
        };
        Ok(WithClause {
            items,
            where_clause,
        })
    }

    // -- UNWIND -----------------------------------------------------------

    fn parse_unwind_clause(&mut self) -> Result<UnwindClause> {
        // UNWIND expression AS variable
        let expression = self.parse_expression()?;

        if !self.at(&TokenKind::As) {
            bail!("Expected AS after UNWIND expression");
        }
        self.advance();

        let variable = self.consume_name()?;

        Ok(UnwindClause {
            expression,
            variable,
        })
    }

    // -- Expressions (precedence climbing) --------------------------------

    fn parse_expression(&mut self) -> Result<Expression> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expression> {
        let mut left = self.parse_xor_expr()?;
        while self.at(&TokenKind::Or) {
            self.advance();
            let right = self.parse_xor_expr()?;
            left = Expression::BinaryOp(Box::new(left), BinaryOperator::Or, Box::new(right));
        }
        Ok(left)
    }

    fn parse_xor_expr(&mut self) -> Result<Expression> {
        let mut left = self.parse_and_expr()?;
        while self.at(&TokenKind::Xor) {
            self.advance();
            let right = self.parse_and_expr()?;
            left = Expression::BinaryOp(Box::new(left), BinaryOperator::Xor, Box::new(right));
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expression> {
        let mut left = self.parse_not_expr()?;
        while self.at(&TokenKind::And) {
            self.advance();
            let right = self.parse_not_expr()?;
            left = Expression::BinaryOp(Box::new(left), BinaryOperator::And, Box::new(right));
        }
        Ok(left)
    }

    fn parse_not_expr(&mut self) -> Result<Expression> {
        if self.at(&TokenKind::Not) {
            self.advance();
            let expr = self.parse_not_expr()?;
            return Ok(Expression::UnaryOp(UnaryOperator::Not, Box::new(expr)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expression> {
        let left = self.parse_addition()?;

        let op = match self.peek() {
            TokenKind::Eq => Some(CompOp::Eq),
            TokenKind::Neq => Some(CompOp::Neq),
            TokenKind::Lt => Some(CompOp::Lt),
            TokenKind::Gt => Some(CompOp::Gt),
            TokenKind::Lte => Some(CompOp::Lte),
            TokenKind::Gte => Some(CompOp::Gte),
            TokenKind::RegexOp => Some(CompOp::RegexMatch),
            TokenKind::In => Some(CompOp::In),
            TokenKind::Contains => Some(CompOp::Contains),
            TokenKind::StartsWith => {
                // STARTS WITH is two tokens
                if self.cursor + 1 < self.tokens.len()
                    && self.tokens[self.cursor + 1].kind == TokenKind::With
                {
                    Some(CompOp::StartsWith)
                } else {
                    None
                }
            }
            TokenKind::EndsWith => {
                // ENDS WITH is two tokens
                if self.cursor + 1 < self.tokens.len()
                    && self.tokens[self.cursor + 1].kind == TokenKind::With
                {
                    Some(CompOp::EndsWith)
                } else {
                    None
                }
            }
            TokenKind::Is => {
                // IS NULL / IS NOT NULL
                if self.cursor + 1 < self.tokens.len() {
                    if self.tokens[self.cursor + 1].kind == TokenKind::Null {
                        Some(CompOp::IsNull)
                    } else if self.tokens[self.cursor + 1].kind == TokenKind::Not
                        && self.cursor + 2 < self.tokens.len()
                        && self.tokens[self.cursor + 2].kind == TokenKind::Null
                    {
                        Some(CompOp::IsNotNull)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(comp_op) = op {
            // Advance past the operator token(s)
            match comp_op {
                CompOp::StartsWith | CompOp::EndsWith => {
                    self.advance(); // STARTS/ENDS
                    self.advance(); // WITH
                }
                CompOp::IsNull => {
                    self.advance(); // IS
                    self.advance(); // NULL
                    return Ok(Expression::Comparison(
                        Box::new(left),
                        comp_op,
                        Box::new(Expression::Literal(CypherValue::Null)),
                    ));
                }
                CompOp::IsNotNull => {
                    self.advance(); // IS
                    self.advance(); // NOT
                    self.advance(); // NULL
                    return Ok(Expression::Comparison(
                        Box::new(left),
                        comp_op,
                        Box::new(Expression::Literal(CypherValue::Null)),
                    ));
                }
                _ => {
                    self.advance();
                }
            }
            let right = self.parse_addition()?;
            Ok(Expression::Comparison(
                Box::new(left),
                comp_op,
                Box::new(right),
            ))
        } else {
            Ok(left)
        }
    }

    fn parse_addition(&mut self) -> Result<Expression> {
        let mut left = self.parse_multiplication()?;
        loop {
            match self.peek() {
                TokenKind::Plus => {
                    self.advance();
                    let right = self.parse_multiplication()?;
                    left =
                        Expression::BinaryOp(Box::new(left), BinaryOperator::Plus, Box::new(right));
                }
                TokenKind::Minus => {
                    self.advance();
                    let right = self.parse_multiplication()?;
                    left = Expression::BinaryOp(
                        Box::new(left),
                        BinaryOperator::Minus,
                        Box::new(right),
                    );
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_multiplication(&mut self) -> Result<Expression> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek() {
                TokenKind::Star => {
                    self.advance();
                    let right = self.parse_unary()?;
                    left = Expression::BinaryOp(
                        Box::new(left),
                        BinaryOperator::Multiply,
                        Box::new(right),
                    );
                }
                TokenKind::Slash => {
                    self.advance();
                    let right = self.parse_unary()?;
                    left = Expression::BinaryOp(
                        Box::new(left),
                        BinaryOperator::Divide,
                        Box::new(right),
                    );
                }
                TokenKind::Percent => {
                    self.advance();
                    let right = self.parse_unary()?;
                    left = Expression::BinaryOp(
                        Box::new(left),
                        BinaryOperator::Modulo,
                        Box::new(right),
                    );
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expression> {
        if self.at(&TokenKind::Minus) {
            self.advance();
            let expr = self.parse_unary()?;
            return Ok(Expression::UnaryOp(UnaryOperator::Negate, Box::new(expr)));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expression> {
        let mut expr = self.parse_primary()?;

        // Property access chains: n.name, n.address.city
        while self.at(&TokenKind::Dot) {
            self.advance();
            let prop = self.consume_name()?;
            expr = Expression::Property(Box::new(expr), prop);
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expression> {
        match self.peek().clone() {
            TokenKind::Integer(n) => {
                self.advance();
                Ok(Expression::Literal(CypherValue::Integer(n)))
            }
            TokenKind::Float(f) => {
                self.advance();
                Ok(Expression::Literal(CypherValue::Float(f)))
            }
            TokenKind::StringLit(s) => {
                self.advance();
                Ok(Expression::Literal(CypherValue::String(s)))
            }
            TokenKind::BoolTrue => {
                self.advance();
                Ok(Expression::Literal(CypherValue::Boolean(true)))
            }
            TokenKind::BoolFalse => {
                self.advance();
                Ok(Expression::Literal(CypherValue::Boolean(false)))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expression::Literal(CypherValue::Null))
            }

            TokenKind::Dollar => {
                self.advance();
                let name = self.expect_ident()?;
                Ok(Expression::Parameter(name))
            }

            TokenKind::LBracket => {
                self.advance();
                // Check for list comprehension: [x IN list WHERE pred | projection]
                // or pattern comprehension: [(a)-->(b) WHERE pred | b.name]
                if self.at(&TokenKind::LParen) {
                    // This is a pattern comprehension
                    return self.parse_pattern_comprehension();
                } else {
                    // Check if this might be a list comprehension: [x IN list ...]
                    // We need to look ahead to see if there's an IN keyword
                    let save_cursor = self.cursor;
                    let is_comprehension = matches!(self.peek(), TokenKind::Ident(_)) && {
                        // Peek ahead to check for IN
                        let temp_cursor = save_cursor + 1;
                        if temp_cursor < self.tokens.len() {
                            matches!(self.tokens[temp_cursor].kind, TokenKind::In)
                        } else {
                            false
                        }
                    };
                    self.cursor = save_cursor; // Restore cursor

                    if is_comprehension {
                        return self.parse_list_comprehension();
                    }
                }

                // Regular list literal
                let mut items = Vec::new();
                if !self.at(&TokenKind::RBracket) {
                    loop {
                        items.push(self.parse_expression()?);
                        if !self.at(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Expression::List(items))
            }

            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }

            TokenKind::Reduce => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                return self.parse_reduce_expression();
            }

            TokenKind::Ident(name) => {
                self.advance();
                // Check for function call
                if self.at(&TokenKind::LParen) {
                    self.advance();
                    let name_upper = name.to_ascii_uppercase();

                    // Check for REDUCE function
                    if name_upper == "REDUCE" {
                        return self.parse_reduce_expression();
                    }

                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        // Handle COUNT(*) specially
                        if self.at(&TokenKind::Star) {
                            self.advance();
                            // COUNT(*) is represented as FunctionCall("COUNT", [])
                        } else {
                            loop {
                                args.push(self.parse_expression()?);
                                if !self.at(&TokenKind::Comma) {
                                    break;
                                }
                                self.advance();
                            }
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expression::FunctionCall(name_upper, args))
                } else {
                    Ok(Expression::Variable(name))
                }
            }

            other => bail!(
                "Expected expression but found {:?} at position {}",
                other,
                self.tokens[self.cursor].pos
            ),
        }
    }

    // -- REDUCE expression --------------------------------------------------

    fn parse_reduce_expression(&mut self) -> Result<Expression> {
        // REDUCE(accumulator = initial, variable IN list | update)
        // Parse accumulator variable
        let accumulator = self.expect_ident()?;

        // Expect '='
        if !self.at(&TokenKind::Eq) {
            bail!("Expected '=' after accumulator name in REDUCE");
        }
        self.advance();

        // Parse initial value
        let initial = self.parse_expression()?;

        // Expect ','
        if !self.at(&TokenKind::Comma) {
            bail!("Expected ',' after initial value in REDUCE");
        }
        self.advance();

        // Parse iteration variable
        let variable = self.expect_ident()?;

        // Expect IN
        if !self.at(&TokenKind::In) {
            bail!("Expected IN after variable name in REDUCE");
        }
        self.advance();

        // Parse list expression
        let list = self.parse_expression()?;

        // Expect '|'
        if !self.at(&TokenKind::Pipe) {
            bail!("Expected '|' after list expression in REDUCE");
        }
        self.advance();

        // Parse update expression
        let update = self.parse_expression()?;

        // Expect closing ')'
        self.expect(&TokenKind::RParen)?;

        Ok(Expression::Reduce {
            accumulator,
            initial: Box::new(initial),
            variable,
            list: Box::new(list),
            update: Box::new(update),
        })
    }

    // -- List comprehension -------------------------------------------------

    fn parse_list_comprehension(&mut self) -> Result<Expression> {
        // [variable IN list WHERE filter | projection]
        // [variable IN list | projection]
        // [variable IN list]

        // Parse variable name
        let variable = self.expect_ident()?;

        // Expect IN
        if !self.at(&TokenKind::In) {
            bail!("Expected IN in list comprehension");
        }
        self.advance();

        // Parse list expression
        let list = self.parse_expression()?;

        // Optional WHERE clause
        let filter = if self.at(&TokenKind::Where) {
            self.advance();
            Some(Box::new(self.parse_expression()?))
        } else {
            None
        };

        // Optional projection (after '|')
        let projection = if self.at(&TokenKind::Pipe) {
            self.advance();
            Some(Box::new(self.parse_expression()?))
        } else {
            None
        };

        // Expect closing ']'
        self.expect(&TokenKind::RBracket)?;

        Ok(Expression::ListComprehension {
            variable,
            list: Box::new(list),
            filter,
            projection,
        })
    }

    // -- Pattern comprehension ----------------------------------------------

    fn parse_pattern_comprehension(&mut self) -> Result<Expression> {
        // [(a)-->(b) WHERE filter | projection]
        // [(a:Person)-->(b) | b.name]

        // Parse pattern
        let pattern = self.parse_pattern_path()?;

        // Optional WHERE clause
        let filter = if self.at(&TokenKind::Where) {
            self.advance();
            Some(Box::new(self.parse_expression()?))
        } else {
            None
        };

        // Expect '|' before projection
        if !self.at(&TokenKind::Pipe) {
            bail!("Expected '|' in pattern comprehension");
        }
        self.advance();

        // Parse projection expression
        let projection = self.parse_expression()?;

        // Expect closing ']'
        self.expect(&TokenKind::RBracket)?;

        Ok(Expression::PatternComprehension {
            pattern,
            filter,
            projection: Box::new(projection),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_match() {
        let stmt = CypherParser::parse("MATCH (n:Person) RETURN n").unwrap();
        assert_eq!(stmt.clauses.len(), 2);

        // First clause should be Match
        match &stmt.clauses[0] {
            CypherClause::Match(mc) => {
                assert_eq!(mc.patterns.len(), 1);
                assert_eq!(mc.patterns[0].elements.len(), 1);
                match &mc.patterns[0].elements[0] {
                    PatternElement::Node(np) => {
                        assert_eq!(np.variable.as_deref(), Some("n"));
                        assert_eq!(np.labels, vec!["Person"]);
                    }
                    other => panic!("Expected Node, got {:?}", other),
                }
            }
            other => panic!("Expected Match, got {:?}", other),
        }

        // Second clause should be Return
        match &stmt.clauses[1] {
            CypherClause::Return(rc) => {
                assert!(!rc.distinct);
                assert_eq!(rc.items.len(), 1);
                match &rc.items[0].expression {
                    Expression::Variable(name) => assert_eq!(name, "n"),
                    other => panic!("Expected Variable, got {:?}", other),
                }
            }
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_multi_hop() {
        let stmt =
            CypherParser::parse("MATCH (a)-[:KNOWS]->(b)-[:WORKS_AT]->(c) RETURN c").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Match(mc) => {
                // Node-Rel-Node-Rel-Node = 5 elements
                assert_eq!(mc.patterns[0].elements.len(), 5);

                // Check first relationship type
                match &mc.patterns[0].elements[1] {
                    PatternElement::Relationship(rp) => {
                        assert_eq!(rp.rel_types, vec!["KNOWS"]);
                        assert_eq!(rp.direction, Direction::Right);
                    }
                    other => panic!("Expected Relationship, got {:?}", other),
                }

                // Check second relationship type
                match &mc.patterns[0].elements[3] {
                    PatternElement::Relationship(rp) => {
                        assert_eq!(rp.rel_types, vec!["WORKS_AT"]);
                        assert_eq!(rp.direction, Direction::Right);
                    }
                    other => panic!("Expected Relationship, got {:?}", other),
                }
            }
            other => panic!("Expected Match, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_optional_match() {
        let stmt =
            CypherParser::parse("MATCH (a) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a, b").unwrap();

        assert_eq!(stmt.clauses.len(), 3);

        match &stmt.clauses[0] {
            CypherClause::Match(_) => {}
            other => panic!("Expected Match, got {:?}", other),
        }
        match &stmt.clauses[1] {
            CypherClause::OptionalMatch(mc) => {
                assert_eq!(mc.patterns[0].elements.len(), 3);
            }
            other => panic!("Expected OptionalMatch, got {:?}", other),
        }
        match &stmt.clauses[2] {
            CypherClause::Return(rc) => {
                assert_eq!(rc.items.len(), 2);
            }
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_aggregation() {
        let stmt =
            CypherParser::parse("MATCH (n:Person) RETURN n.city, COUNT(n) AS count").unwrap();

        match &stmt.clauses[1] {
            CypherClause::Return(rc) => {
                assert_eq!(rc.items.len(), 2);

                // n.city
                match &rc.items[0].expression {
                    Expression::Property(base, prop) => {
                        assert!(matches!(base.as_ref(), Expression::Variable(v) if v == "n"));
                        assert_eq!(prop, "city");
                    }
                    other => panic!("Expected Property access, got {:?}", other),
                }

                // COUNT(n) AS count
                match &rc.items[1].expression {
                    Expression::FunctionCall(name, args) => {
                        assert_eq!(name, "COUNT");
                        assert_eq!(args.len(), 1);
                    }
                    other => panic!("Expected FunctionCall, got {:?}", other),
                }
                assert_eq!(rc.items[1].alias.as_deref(), Some("count"));
            }
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_create() {
        let stmt = CypherParser::parse("CREATE (n:Person {name: 'Alice', age: 30})").unwrap();

        assert_eq!(stmt.clauses.len(), 1);
        match &stmt.clauses[0] {
            CypherClause::Create(cc) => {
                assert_eq!(cc.patterns.len(), 1);
                match &cc.patterns[0].elements[0] {
                    PatternElement::Node(np) => {
                        assert_eq!(np.variable.as_deref(), Some("n"));
                        assert_eq!(np.labels, vec!["Person"]);
                        assert_eq!(np.properties.len(), 2);
                        assert_eq!(np.properties[0].0, "name");
                        assert!(
                            matches!(&np.properties[0].1, CypherValue::String(s) if s == "Alice")
                        );
                        assert_eq!(np.properties[1].0, "age");
                        assert!(matches!(&np.properties[1].1, CypherValue::Integer(30)));
                    }
                    other => panic!("Expected Node, got {:?}", other),
                }
            }
            other => panic!("Expected Create, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_where_complex() {
        let stmt =
            CypherParser::parse("MATCH (n:Person) WHERE n.age > 25 AND n.city = 'NYC' RETURN n")
                .unwrap();

        assert_eq!(stmt.clauses.len(), 3);

        match &stmt.clauses[1] {
            CypherClause::Where(wc) => {
                // Should be AND of two comparisons
                match &wc.expression {
                    Expression::BinaryOp(left, BinaryOperator::And, right) => {
                        // left: n.age > 25
                        match left.as_ref() {
                            Expression::Comparison(_, CompOp::Gt, _) => {}
                            other => panic!("Expected Gt comparison, got {:?}", other),
                        }
                        // right: n.city = 'NYC'
                        match right.as_ref() {
                            Expression::Comparison(_, CompOp::Eq, _) => {}
                            other => panic!("Expected Eq comparison, got {:?}", other),
                        }
                    }
                    other => panic!("Expected AND expression, got {:?}", other),
                }
            }
            other => panic!("Expected Where, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_order_limit() {
        let stmt =
            CypherParser::parse("MATCH (n:Person) RETURN n.name ORDER BY n.name DESC LIMIT 10")
                .unwrap();

        // Clauses: MATCH, RETURN, ORDER BY, LIMIT
        assert_eq!(stmt.clauses.len(), 4);

        match &stmt.clauses[2] {
            CypherClause::OrderBy(ob) => {
                assert_eq!(ob.items.len(), 1);
                assert_eq!(ob.items[0].1, SortDirection::Desc);
                match &ob.items[0].0 {
                    Expression::Property(_, prop) => assert_eq!(prop, "name"),
                    other => panic!("Expected Property, got {:?}", other),
                }
            }
            other => panic!("Expected OrderBy, got {:?}", other),
        }

        match &stmt.clauses[3] {
            CypherClause::Limit(lc) => assert_eq!(lc.count, 10),
            other => panic!("Expected Limit, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_union() {
        let stmt = CypherParser::parse(
            "MATCH (n:Person) RETURN n.name UNION MATCH (n:Company) RETURN n.name",
        )
        .unwrap();

        // MATCH, RETURN, UNION, MATCH, RETURN
        assert_eq!(stmt.clauses.len(), 5);

        match &stmt.clauses[2] {
            CypherClause::Union(uc) => assert!(!uc.all),
            other => panic!("Expected Union, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_union_all() {
        let stmt =
            CypherParser::parse("MATCH (n:Person) RETURN n UNION ALL MATCH (n:Company) RETURN n")
                .unwrap();

        match &stmt.clauses[2] {
            CypherClause::Union(uc) => assert!(uc.all),
            other => panic!("Expected Union, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_variable_length_path() {
        let stmt = CypherParser::parse("MATCH (a)-[*1..3]->(b) RETURN b").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Match(mc) => match &mc.patterns[0].elements[1] {
                PatternElement::Relationship(rp) => {
                    assert_eq!(rp.range, Some((Some(1), Some(3))));
                    assert_eq!(rp.direction, Direction::Right);
                }
                other => panic!("Expected Relationship, got {:?}", other),
            },
            other => panic!("Expected Match, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_clause() {
        let stmt = CypherParser::parse(
            "MATCH (n:Person) WITH n.city AS city, COUNT(n) AS cnt WHERE cnt > 5 RETURN city",
        )
        .unwrap();

        match &stmt.clauses[1] {
            CypherClause::With(wc) => {
                assert_eq!(wc.items.len(), 2);
                assert_eq!(wc.items[0].alias.as_deref(), Some("city"));
                assert_eq!(wc.items[1].alias.as_deref(), Some("cnt"));
                assert!(wc.where_clause.is_some());
            }
            other => panic!("Expected With, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_set_clause() {
        let stmt = CypherParser::parse("MATCH (n:Person {name: 'Alice'}) SET n.age = 31 RETURN n")
            .unwrap();

        match &stmt.clauses[1] {
            CypherClause::Set(sc) => {
                assert_eq!(sc.items.len(), 1);
                match &sc.items[0] {
                    SetItem::Property {
                        variable, property, ..
                    } => {
                        assert_eq!(variable, "n");
                        assert_eq!(property, "age");
                    }
                }
            }
            other => panic!("Expected Set, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_delete_clause() {
        let stmt = CypherParser::parse("MATCH (n:Person {name: 'Alice'}) DETACH DELETE n").unwrap();

        match &stmt.clauses[1] {
            CypherClause::Delete(dc) => {
                assert!(dc.detach);
                assert_eq!(dc.expressions.len(), 1);
            }
            other => panic!("Expected Delete, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_left_directed_relationship() {
        let stmt = CypherParser::parse("MATCH (a)<-[:KNOWS]-(b) RETURN a, b").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Match(mc) => match &mc.patterns[0].elements[1] {
                PatternElement::Relationship(rp) => {
                    assert_eq!(rp.direction, Direction::Left);
                    assert_eq!(rp.rel_types, vec!["KNOWS"]);
                }
                other => panic!("Expected Relationship, got {:?}", other),
            },
            other => panic!("Expected Match, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_function_calls() {
        let stmt = CypherParser::parse(
            "MATCH (n:Person) RETURN COUNT(*), SUM(n.age), AVG(n.age), MIN(n.age), MAX(n.age), COLLECT(n.name)"
        ).unwrap();

        match &stmt.clauses[1] {
            CypherClause::Return(rc) => {
                assert_eq!(rc.items.len(), 6);

                // COUNT(*)
                match &rc.items[0].expression {
                    Expression::FunctionCall(name, args) => {
                        assert_eq!(name, "COUNT");
                        assert!(args.is_empty()); // COUNT(*) has no args
                    }
                    other => panic!("Expected FunctionCall, got {:?}", other),
                }

                // SUM(n.age)
                match &rc.items[1].expression {
                    Expression::FunctionCall(name, args) => {
                        assert_eq!(name, "SUM");
                        assert_eq!(args.len(), 1);
                    }
                    other => panic!("Expected FunctionCall, got {:?}", other),
                }
            }
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_error_unterminated_string() {
        let result = CypherParser::parse("MATCH (n:Person {name: 'Alice}) RETURN n");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_empty_query() {
        let result = CypherParser::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_case_insensitive() {
        // Keywords should be case-insensitive
        let stmt = CypherParser::parse("match (n:Person) where n.age > 25 return n").unwrap();
        assert_eq!(stmt.clauses.len(), 3);
    }

    #[test]
    fn test_parse_multiple_labels() {
        let stmt = CypherParser::parse("MATCH (n:Person:Employee) RETURN n").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Match(mc) => match &mc.patterns[0].elements[0] {
                PatternElement::Node(np) => {
                    assert_eq!(np.labels, vec!["Person", "Employee"]);
                }
                other => panic!("Expected Node, got {:?}", other),
            },
            other => panic!("Expected Match, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_return_distinct() {
        let stmt = CypherParser::parse("MATCH (n:Person) RETURN DISTINCT n.city").unwrap();

        match &stmt.clauses[1] {
            CypherClause::Return(rc) => {
                assert!(rc.distinct);
                assert_eq!(rc.items.len(), 1);
            }
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_parameter() {
        let stmt = CypherParser::parse("MATCH (n:Person) WHERE n.name = $name RETURN n").unwrap();

        match &stmt.clauses[1] {
            CypherClause::Where(wc) => match &wc.expression {
                Expression::Comparison(_, CompOp::Eq, right) => match right.as_ref() {
                    Expression::Parameter(name) => assert_eq!(name, "name"),
                    other => panic!("Expected Parameter, got {:?}", other),
                },
                other => panic!("Expected Comparison, got {:?}", other),
            },
            other => panic!("Expected Where, got {:?}", other),
        }
    }

    // UNWIND clause tests

    #[test]
    fn test_parse_unwind() {
        let stmt = CypherParser::parse("UNWIND [1, 2, 3] AS x RETURN x").unwrap();
        assert_eq!(stmt.clauses.len(), 2);

        match &stmt.clauses[0] {
            CypherClause::Unwind(uw) => {
                match &uw.expression {
                    Expression::List(items) => {
                        assert_eq!(items.len(), 3);
                    }
                    other => panic!("Expected List, got {:?}", other),
                }
                assert_eq!(uw.variable, "x");
            }
            other => panic!("Expected Unwind, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unwind_with_variable() {
        let stmt = CypherParser::parse("UNWIND $list AS item RETURN item").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Unwind(uw) => {
                match &uw.expression {
                    Expression::Parameter(name) => {
                        assert_eq!(name, "list");
                    }
                    other => panic!("Expected Parameter, got {:?}", other),
                }
                assert_eq!(uw.variable, "item");
            }
            other => panic!("Expected Unwind, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unwind_after_match() {
        let stmt = CypherParser::parse("MATCH (n) UNWIND n.items AS item RETURN item").unwrap();
        assert_eq!(stmt.clauses.len(), 3);

        match &stmt.clauses[1] {
            CypherClause::Unwind(uw) => {
                assert_eq!(uw.variable, "item");
            }
            other => panic!("Expected Unwind, got {:?}", other),
        }
    }

    // REDUCE expression tests

    #[test]
    fn test_parse_reduce() {
        let stmt =
            CypherParser::parse("RETURN REDUCE(total = 0, x IN [1, 2, 3] | total + x)").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::Reduce {
                    accumulator,
                    variable,
                    ..
                } => {
                    assert_eq!(accumulator, "total");
                    assert_eq!(variable, "x");
                }
                other => panic!("Expected Reduce, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_reduce_complex() {
        let stmt =
            CypherParser::parse("RETURN REDUCE(sum = 0, n IN collect(n.price) | sum + n)").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::Reduce {
                    accumulator,
                    variable,
                    ..
                } => {
                    assert_eq!(accumulator, "sum");
                    assert_eq!(variable, "n");
                }
                other => panic!("Expected Reduce, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    // List comprehension tests

    #[test]
    fn test_parse_list_comprehension_simple() {
        let stmt = CypherParser::parse("RETURN [x IN [1, 2, 3] | x * 2]").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::ListComprehension {
                    variable,
                    list: _,
                    filter,
                    projection,
                } => {
                    assert_eq!(variable, "x");
                    assert!(filter.is_none());
                    assert!(projection.is_some());
                }
                other => panic!("Expected ListComprehension, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_list_comprehension_with_filter() {
        let stmt =
            CypherParser::parse("RETURN [x IN [1, 2, 3, 4, 5] WHERE x > 2 | x * 2]").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::ListComprehension {
                    variable,
                    list: _,
                    filter,
                    projection,
                } => {
                    assert_eq!(variable, "x");
                    assert!(filter.is_some());
                    assert!(projection.is_some());
                }
                other => panic!("Expected ListComprehension, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_list_comprehension_no_projection() {
        let stmt = CypherParser::parse("RETURN [x IN [1, 2, 3] WHERE x > 1]").unwrap();

        match &stmt.clauses[0] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::ListComprehension {
                    variable,
                    list: _,
                    filter,
                    projection,
                } => {
                    assert_eq!(variable, "x");
                    assert!(filter.is_some());
                    assert!(projection.is_none());
                }
                other => panic!("Expected ListComprehension, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    // Pattern comprehension tests

    #[test]
    fn test_parse_pattern_comprehension() {
        let stmt =
            CypherParser::parse("MATCH (a:Person) RETURN [(a)-->(b:Friend) | b.name]").unwrap();

        match &stmt.clauses[1] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::PatternComprehension { filter, .. } => {
                    assert!(filter.is_none());
                }
                other => panic!("Expected PatternComprehension, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_pattern_comprehension_with_filter() {
        let stmt = CypherParser::parse(
            "MATCH (a:Person) RETURN [(a)-->(b:Friend) WHERE b.age > 25 | b.name]",
        )
        .unwrap();

        match &stmt.clauses[1] {
            CypherClause::Return(rc) => match &rc.items[0].expression {
                Expression::PatternComprehension { filter, .. } => {
                    assert!(filter.is_some());
                }
                other => panic!("Expected PatternComprehension, got {:?}", other),
            },
            other => panic!("Expected Return, got {:?}", other),
        }
    }

    // Combined feature tests

    #[test]
    fn test_parse_unwind_with_comprehension() {
        let stmt =
            CypherParser::parse("UNWIND [1, 2, 3] AS x RETURN [y IN [x, x*2] | y * 3]").unwrap();
        assert_eq!(stmt.clauses.len(), 2);
    }

    #[test]
    fn test_parse_reduce_with_unwind() {
        let stmt = CypherParser::parse(
            "UNWIND [[1, 2], [3, 4]] AS nested RETURN REDUCE(sum = 0, x IN nested | sum + x)",
        )
        .unwrap();
        assert_eq!(stmt.clauses.len(), 2);
    }
}
