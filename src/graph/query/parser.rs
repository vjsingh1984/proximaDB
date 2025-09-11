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

//! # Query Parser for Cypher-like Queries
//!
//! This module implements a parser for Cypher-like graph query patterns,
//! transforming a string representation into an Abstract Syntax Tree (AST).
//!
//! It uses the `nom` parser-combinator library for robust and efficient parsing.

use super::ast::{
    CompiledPattern, EdgeDirection, EdgePattern, LogicalOperator, NodePattern, OrderBy,
    PathElement, PathPattern, PropertyConstraint, PropertyProjection, ReturnSpec, VariableBinding,
    WhereClause,
};
use crate::core::error::ProximaDBError;
use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{alpha1, alphanumeric1, char, multispace0, multispace1},
    combinator::{map, opt, recognize},
    multi::{many0, separated_list0},
    sequence::{delimited, preceded, separated_pair, tuple},
};
use std::collections::HashMap;

type QueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Main query parser struct
pub struct QueryParser;

impl QueryParser {
    pub fn new() -> Self {
        QueryParser
    }

    /// Parse a Cypher-like query string into a CompiledPattern (AST)
    pub fn parse(&self, input: &str) -> QueryResult<CompiledPattern> {
        match parse_query(input) {
            Ok((_, compiled_pattern)) => Ok(compiled_pattern),
            Err(e) => Err(ProximaDBError::invalid_argument(&format!(
                "Failed to parse query: {}",
                e
            ))),
        }
    }
}

// --- Nom Parser Combinators ---

// Helper to parse identifiers (variable names, labels, property keys)
fn identifier(input: &str) -> IResult<&str, String> {
    map(
        recognize(tuple((
            alt((alpha1, tag("_"))),
            many0(alt((alphanumeric1, tag("_")))),
        ))),
        String::from,
    )(input)
}

// Helper to parse string literals (e.g., "value" or 'value')
fn string_literal(input: &str) -> IResult<&str, String> {
    alt((
        delimited(char('"'), take_while1(|c| c != '"'), char('"')),
        delimited(char('\''), take_while1(|c| c != '\''), char('\'')),
    ))
    .map(|s| s.to_string())
    .parse(input)
}

// Helper to parse integer literals
fn integer_literal(input: &str) -> IResult<&str, i64> {
    map(take_while1(|c: char| c.is_ascii_digit()), |s: &str| {
        s.parse::<i64>().unwrap()
    })(input)
}

// Helper to parse boolean literals
fn boolean_literal(input: &str) -> IResult<&str, bool> {
    alt((map(tag("true"), |_| true), map(tag("false"), |_| false)))(input)
}

// Helper to parse property values (simplified to string, int, bool for now)
fn property_value(input: &str) -> IResult<&str, serde_json::Value> {
    alt((
        map(string_literal, serde_json::Value::String),
        map(integer_literal, |i| {
            serde_json::Value::Number(serde_json::Number::from(i))
        }),
        map(boolean_literal, serde_json::Value::Bool),
    ))(input)
}

// Helper to parse property assignments (e.g., key: value)
fn property_assignment(input: &str) -> IResult<&str, (String, PropertyConstraint)> {
    map(
        separated_pair(
            identifier,
            delimited(multispace0, char(':'), multispace0),
            property_value,
        ),
        |(key, value)| (key, PropertyConstraint::Equals(value)),
    )(input)
}

// Helper to parse property map (e.g., {key: value, key2: value2})
fn property_map(input: &str) -> IResult<&str, HashMap<String, PropertyConstraint>> {
    delimited(
        char('{'),
        separated_list0(
            delimited(multispace0, char(','), multispace0),
            property_assignment,
        ),
        char('}'),
    )
    .map(|assignments| assignments.into_iter().collect())
    .parse(input)
}

// Parse a node pattern (e.g., (n:Label {prop: "val"}))
fn parse_node_pattern(input: &str) -> IResult<&str, NodePattern> {
    map(
        delimited(
            char('('),
            tuple((
                identifier,                               // Variable
                opt(preceded(char(':'), identifier)),     // Optional Label
                opt(preceded(multispace0, property_map)), // Optional Properties
            )),
            char(')'),
        ),
        |(variable, label_opt, properties_opt)| NodePattern {
            variable,
            labels: label_opt.map(|l| vec![l]).unwrap_or_default(),
            properties: properties_opt.unwrap_or_default(),
            optional: false, // Not handling OPTIONAL MATCH yet
        },
    )(input)
}

// Parse a simple MATCH clause
fn parse_match_clause(input: &str) -> IResult<&str, Vec<NodePattern>> {
    preceded(
        tag("MATCH"),
        preceded(
            multispace1,
            separated_list0(
                delimited(multispace0, char(','), multispace0),
                parse_node_pattern,
            ),
        ),
    )(input)
}

// Parse a simple RETURN clause
fn parse_return_clause(input: &str) -> IResult<&str, ReturnSpec> {
    preceded(
        tag("RETURN"),
        preceded(
            multispace1,
            map(
                separated_list0(delimited(multispace0, char(','), multispace0), identifier),
                |vars| ReturnSpec {
                    variables: vars,
                    projections: Vec::new(), // Not handling projections yet
                    distinct: false,
                    order_by: Vec::new(),
                    limit: None,
                    skip: None,
                },
            ),
        ),
    )(input)
}

// Main query parser
fn parse_query(input: &str) -> IResult<&str, CompiledPattern> {
    map(
        tuple((
            parse_match_clause,
            multispace0,
            parse_return_clause,
            multispace0,
        )),
        |(nodes, _, return_spec, _)| CompiledPattern {
            nodes,
            edges: Vec::new(),
            paths: Vec::new(),
            where_clauses: Vec::new(),
            return_spec,
            variables: HashMap::new(), // Populated during planning/execution
        },
    )(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_match_return() {
        let parser = QueryParser::new();
        let query = "MATCH (n:Person) RETURN n";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.nodes.len(), 1);
        assert_eq!(compiled.nodes[0].variable, "n");
        assert_eq!(compiled.nodes[0].labels, vec!["Person"]);
        assert!(compiled.nodes[0].properties.is_empty());

        assert_eq!(compiled.return_spec.variables.len(), 1);
        assert_eq!(compiled.return_spec.variables[0], "n");
    }

    #[test]
    fn test_parse_match_with_properties() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person {name: \"Alice\", age: 30, active: true}) RETURN p";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.nodes.len(), 1);
        let node_pattern = &compiled.nodes[0];
        assert_eq!(node_pattern.variable, "p");
        assert_eq!(node_pattern.labels, vec!["Person"]);
        assert_eq!(node_pattern.properties.len(), 3);
        assert_eq!(
            node_pattern.properties["name"],
            PropertyConstraint::Equals(serde_json::Value::String("Alice".to_string()))
        );
        assert_eq!(
            node_pattern.properties["age"],
            PropertyConstraint::Equals(serde_json::Value::Number(serde_json::Number::from(30)))
        );
        assert_eq!(
            node_pattern.properties["active"],
            PropertyConstraint::Equals(serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn test_parse_multiple_nodes() {
        let parser = QueryParser::new();
        let query = "MATCH (a:Person), (b:Company) RETURN a, b";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.nodes.len(), 2);
        assert_eq!(compiled.nodes[0].variable, "a");
        assert_eq!(compiled.nodes[0].labels, vec!["Person"]);
        assert_eq!(compiled.nodes[1].variable, "b");
        assert_eq!(compiled.nodes[1].labels, vec!["Company"]);

        assert_eq!(compiled.return_spec.variables.len(), 2);
        assert_eq!(compiled.return_spec.variables[0], "a");
        assert_eq!(compiled.return_spec.variables[1], "b");
    }

    #[test]
    fn test_parse_no_label_node() {
        let parser = QueryParser::new();
        let query = "MATCH (n) RETURN n";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.nodes.len(), 1);
        assert_eq!(compiled.nodes[0].variable, "n");
        assert!(compiled.nodes[0].labels.is_empty());
    }

    #[test]
    fn test_parse_error() {
        let parser = QueryParser::new();
        let query = "MATCH (n:Person RETURN n"; // Missing closing parenthesis
        let result = parser.parse(query);
        assert!(result.is_err());
    }
}
