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
//!
//! ## Enhanced Features
//!
//! - **Edge Patterns**: `-[r:TYPE]->`, `<-[r:TYPE]-`, `-[r:TYPE]-`
//! - **WHERE Clauses**: Complex predicates with AND/OR/NOT
//! - **Aggregations**: COUNT, SUM, AVG, MIN, MAX
//! - **Variable-Length Paths**: `-[*1..5]->`

use super::ast::{
    CompiledPattern, EdgeDirection, EdgePattern, NodePattern, PropertyConstraint,
    PropertyProjection, ReturnSpec, WhereClause,
};
use crate::core::error::ProximaDBError;
use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while1},
    character::complete::{alpha1, alphanumeric1, char, digit1, multispace0, multispace1},
    combinator::{map, map_res, opt, recognize, value},
    multi::{many0, separated_list0, separated_list1},
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
            Err(e) => Err(ProximaDBError::InvalidInput(format!(
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
    map(
        alt((
            delimited(
                char::<&str, nom::error::Error<_>>('"'),
                take_while1(|c| c != '"'),
                char('"'),
            ),
            delimited(
                char::<&str, nom::error::Error<_>>('\''),
                take_while1(|c| c != '\''),
                char('\''),
            ),
        )),
        |s| s.to_string(),
    )(input)
}

// Helper to parse integer literals
fn integer_literal(input: &str) -> IResult<&str, i64> {
    map_res(take_while1(|c: char| c.is_ascii_digit()), |s: &str| {
        s.parse::<i64>()
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
    map(
        delimited(
            char('{'),
            separated_list0(
                delimited(multispace0, char(','), multispace0),
                property_assignment,
            ),
            char('}'),
        ),
        |assignments| assignments.into_iter().collect(),
    )(input)
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

// Parse edge direction arrows
fn parse_edge_direction_left(input: &str) -> IResult<&str, bool> {
    alt((
        map(tag("<-"), |_| true),  // Incoming
        map(tag("-"), |_| false),  // Outgoing or bidirectional
    ))(input)
}

fn parse_edge_direction_right(input: &str) -> IResult<&str, bool> {
    alt((
        map(tag("->"), |_| true),  // Outgoing
        map(tag("-"), |_| false),  // Incoming or bidirectional
    ))(input)
}

// Parse edge specification inside brackets [r:TYPE {prop: val}]
fn parse_edge_spec(input: &str) -> IResult<&str, (Option<String>, Vec<String>, HashMap<String, PropertyConstraint>)> {
    map(
        tuple((
            opt(identifier),                               // Optional variable
            opt(preceded(char(':'), identifier)),          // Optional edge type
            opt(preceded(multispace0, property_map)),      // Optional properties
        )),
        |(variable_opt, edge_type_opt, properties_opt)| {
            let edge_types = edge_type_opt.map(|t| vec![t]).unwrap_or_default();
            let properties = properties_opt.unwrap_or_default();
            (variable_opt, edge_types, properties)
        },
    )(input)
}

// Parse variable-length path specification *min..max
fn parse_variable_length(input: &str) -> IResult<&str, (u32, u32)> {
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
            // *max (equivalent to *1..max)
            map(map_res(digit1, |s: &str| s.parse::<u32>()), |max| (1, max)),
            // * (equivalent to *1..5, default range)
            value((1, 5), tag("")),
        )),
    )(input)
}

// Parse edge pattern (e.g., -[r:KNOWS]-> or <-[r:KNOWS]- or -[r:KNOWS]-)
fn parse_edge_pattern(
    input: &str,
) -> IResult<&str, (bool, Option<String>, Vec<String>, HashMap<String, PropertyConstraint>, bool, Option<(u32, u32)>)> {
    map(
        tuple((
            parse_edge_direction_left,
            delimited(
                char('['),
                tuple((
                    parse_edge_spec,
                    opt(parse_variable_length),
                )),
                char(']'),
            ),
            parse_edge_direction_right,
        )),
        |(left_arrow, ((variable, edge_types, properties), var_length_opt), right_arrow)| {
            // Determine direction from arrows
            let is_outgoing = right_arrow;
            let is_incoming = left_arrow;

            (is_incoming, variable, edge_types, properties, is_outgoing, var_length_opt)
        },
    )(input)
}

// Parse node-edge-node pattern (e.g., (a)-[r:KNOWS]->(b))
fn parse_path_segment(input: &str) -> IResult<&str, (NodePattern, EdgePattern, NodePattern)> {
    map(
        tuple((
            parse_node_pattern,
            preceded(multispace0, parse_edge_pattern),
            preceded(multispace0, parse_node_pattern),
        )),
        |(from_node, (is_incoming, edge_var, edge_types, edge_props, is_outgoing, _var_length), to_node)| {
            let direction = match (is_incoming, is_outgoing) {
                (false, true) => EdgeDirection::Outgoing,  // -[]->
                (true, false) => EdgeDirection::Incoming,  // <-[]-
                (false, false) => EdgeDirection::Bidirectional, // -[]-
                (true, true) => EdgeDirection::Bidirectional, // Invalid, treat as bidirectional
            };

            let edge_pattern = EdgePattern {
                variable: edge_var,
                from_variable: from_node.variable.clone(),
                to_variable: to_node.variable.clone(),
                edge_types,
                properties: edge_props,
                direction,
                optional: false,
            };

            (from_node.clone(), edge_pattern, to_node)
        },
    )(input)
}

// Parse WHERE clause conditions
fn parse_where_condition(input: &str) -> IResult<&str, WhereClause> {
    alt((
        // Logical AND
        map(
            tuple((
                parse_where_primary,
                preceded(
                    delimited(multispace1, tag("AND"), multispace1),
                    parse_where_condition,
                ),
            )),
            |(left, right)| WhereClause::And(Box::new(left), Box::new(right)),
        ),
        // Logical OR
        map(
            tuple((
                parse_where_primary,
                preceded(
                    delimited(multispace1, tag("OR"), multispace1),
                    parse_where_condition,
                ),
            )),
            |(left, right)| WhereClause::Or(Box::new(left), Box::new(right)),
        ),
        // Primary condition
        parse_where_primary,
    ))(input)
}

fn parse_where_primary(input: &str) -> IResult<&str, WhereClause> {
    alt((
        // NOT condition
        map(
            preceded(
                tuple((tag("NOT"), multispace1)),
                parse_where_primary,
            ),
            |cond| WhereClause::Not(Box::new(cond)),
        ),
        // Parenthesized condition
        delimited(
            char('('),
            delimited(multispace0, parse_where_condition, multispace0),
            char(')'),
        ),
        // Property comparison
        parse_where_property_condition,
    ))(input)
}

fn parse_where_property_condition(input: &str) -> IResult<&str, WhereClause> {
    map(
        tuple((
            identifier,                                        // variable
            char('.'),
            identifier,                                        // property
            delimited(multispace0, parse_comparison_op, multispace0),
            property_value,                                    // value
        )),
        |(variable, _, property, operator, value)| WhereClause::Property {
            variable,
            property,
            constraint: match operator {
                "=" => PropertyConstraint::Equals(value),
                ">" => PropertyConstraint::GreaterThan(value),
                "<" => PropertyConstraint::LessThan(value),
                ">=" => PropertyConstraint::GreaterOrEqual(value),
                "<=" => PropertyConstraint::LessOrEqual(value),
                "!=" => PropertyConstraint::NotEquals(value),
                _ => PropertyConstraint::Equals(value),
            },
        },
    )(input)
}

fn parse_comparison_op(input: &str) -> IResult<&str, &str> {
    alt((
        tag(">="),
        tag("<="),
        tag("!="),
        tag("="),
        tag(">"),
        tag("<"),
    ))(input)
}

// Parse WHERE clause
fn parse_where_clause(input: &str) -> IResult<&str, WhereClause> {
    preceded(
        tag("WHERE"),
        preceded(multispace1, parse_where_condition),
    )(input)
}

// Parse aggregation functions
fn parse_aggregation(input: &str) -> IResult<&str, PropertyProjection> {
    alt((
        // COUNT(*)
        map(
            tuple((
                tag_no_case("COUNT"),
                delimited(
                    char('('),
                    delimited(multispace0, char('*'), multispace0),
                    char(')'),
                ),
            )),
            |_| PropertyProjection::Count,
        ),
        // SUM(variable.property)
        map(
            tuple((
                tag_no_case("SUM"),
                delimited(
                    char('('),
                    delimited(
                        multispace0,
                        separated_pair(
                            identifier,
                            char('.'),
                            identifier,
                        ),
                        multispace0,
                    ),
                    char(')'),
                ),
            )),
            |(_, (var, prop))| PropertyProjection::Sum { variable: var, property: prop },
        ),
        // AVG(variable.property)
        map(
            tuple((
                tag_no_case("AVG"),
                delimited(
                    char('('),
                    delimited(
                        multispace0,
                        separated_pair(
                            identifier,
                            char('.'),
                            identifier,
                        ),
                        multispace0,
                    ),
                    char(')'),
                ),
            )),
            |(_, (var, prop))| PropertyProjection::Avg { variable: var, property: prop },
        ),
        // MIN(variable.property)
        map(
            tuple((
                tag_no_case("MIN"),
                delimited(
                    char('('),
                    delimited(
                        multispace0,
                        separated_pair(
                            identifier,
                            char('.'),
                            identifier,
                        ),
                        multispace0,
                    ),
                    char(')'),
                ),
            )),
            |(_, (var, prop))| PropertyProjection::Min { variable: var, property: prop },
        ),
        // MAX(variable.property)
        map(
            tuple((
                tag_no_case("MAX"),
                delimited(
                    char('('),
                    delimited(
                        multispace0,
                        separated_pair(
                            identifier,
                            char('.'),
                            identifier,
                        ),
                        multispace0,
                    ),
                    char(')'),
                ),
            )),
            |(_, (var, prop))| PropertyProjection::Max { variable: var, property: prop },
        ),
    ))(input)
}

// Parse a MATCH clause with support for both nodes and edge patterns
fn parse_match_clause(input: &str) -> IResult<&str, (Vec<NodePattern>, Vec<EdgePattern>)> {
    preceded(
        tag("MATCH"),
        preceded(
            multispace1,
            map(
                separated_list1(
                    delimited(multispace0, char(','), multispace0),
                    alt((
                        // Path segment: (a)-[r]->(b)
                        map(parse_path_segment, |(from, edge, to)| (vec![from, to], vec![edge])),
                        // Simple node: (n:Label)
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
        ),
    )(input)
}

// Parse return item (variable, property projection, or aggregation)
fn parse_return_item(input: &str) -> IResult<&str, (String, PropertyProjection)> {
    alt((
        // Aggregation function
        map(
            tuple((
                parse_aggregation,
                opt(preceded(
                    delimited(multispace1, tag("AS"), multispace1),
                    identifier,
                )),
            )),
            |(agg, alias_opt)| {
                let name = alias_opt.unwrap_or_else(|| match &agg {
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
        // Property projection (variable.property AS alias)
        map(
            tuple((
                identifier,
                char('.'),
                identifier,
                opt(preceded(
                    delimited(multispace1, tag("AS"), multispace1),
                    identifier,
                )),
            )),
            |(var, _, prop, alias_opt)| {
                let name = alias_opt.unwrap_or_else(|| format!("{}.{}", var, prop));
                (name, PropertyProjection::Property {
                    variable: var,
                    property: prop,
                })
            },
        ),
        // Simple variable
        map(identifier, |var| {
            (var.clone(), PropertyProjection::Variable(var))
        }),
    ))(input)
}

// Parse DISTINCT keyword
fn parse_distinct(input: &str) -> IResult<&str, bool> {
    map(opt(preceded(multispace1, tag("DISTINCT"))), |d| d.is_some())(input)
}

// Parse ORDER BY clause
fn parse_order_by(input: &str) -> IResult<&str, Vec<(String, bool)>> {
    preceded(
        delimited(multispace1, tag("ORDER"), multispace1),
        preceded(
            tag("BY"),
            preceded(
                multispace1,
                separated_list1(
                    delimited(multispace0, char(','), multispace0),
                    map(
                        tuple((
                            identifier,
                            opt(preceded(
                                multispace1,
                                alt((
                                    map(tag("ASC"), |_| true),
                                    map(tag("DESC"), |_| false),
                                )),
                            )),
                        )),
                        |(var, asc_opt)| (var, asc_opt.unwrap_or(true)),
                    ),
                ),
            ),
        ),
    )(input)
}

// Parse LIMIT clause
fn parse_limit(input: &str) -> IResult<&str, usize> {
    preceded(
        delimited(multispace1, tag("LIMIT"), multispace1),
        map_res(digit1, |s: &str| s.parse::<usize>()),
    )(input)
}

// Parse SKIP clause
fn parse_skip(input: &str) -> IResult<&str, usize> {
    preceded(
        delimited(multispace1, tag("SKIP"), multispace1),
        map_res(digit1, |s: &str| s.parse::<usize>()),
    )(input)
}

// Parse enhanced RETURN clause
fn parse_return_clause(input: &str) -> IResult<&str, ReturnSpec> {
    preceded(
        tag("RETURN"),
        map(
            tuple((
                parse_distinct,
                preceded(
                    multispace1,
                    separated_list1(
                        delimited(multispace0, char(','), multispace0),
                        parse_return_item,
                    ),
                ),
                opt(parse_order_by),
                opt(parse_skip),
                opt(parse_limit),
            )),
            |(distinct, items, order_by_opt, skip_opt, limit_opt)| {
                let (vars, projections): (Vec<_>, Vec<_>) = items.into_iter().unzip();
                ReturnSpec {
                    variables: vars,
                    projections,
                    distinct,
                    order_by: order_by_opt.unwrap_or_default(),
                    limit: limit_opt,
                    skip: skip_opt,
                }
            },
        ),
    )(input)
}

// Main query parser
fn parse_query(input: &str) -> IResult<&str, CompiledPattern> {
    map(
        tuple((
            parse_match_clause,
            multispace0,
            opt(parse_where_clause),
            multispace0,
            parse_return_clause,
            multispace0,
        )),
        |((nodes, edges), _, where_clause_opt, _, return_spec, _)| CompiledPattern {
            nodes,
            edges,
            paths: Vec::new(),
            where_clauses: where_clause_opt.map(|wc| vec![wc]).unwrap_or_default(),
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
        assert_eq!(compiled.edges.len(), 0);

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

    #[test]
    fn test_parse_edge_pattern() {
        let parser = QueryParser::new();
        let query = "MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a, r, b";
        let compiled = parser.parse(query).unwrap();

        // Should have 2 nodes (a and b)
        assert_eq!(compiled.nodes.len(), 2);
        assert_eq!(compiled.nodes[0].variable, "a");
        assert_eq!(compiled.nodes[1].variable, "b");

        // Should have 1 edge
        assert_eq!(compiled.edges.len(), 1);
        assert_eq!(compiled.edges[0].variable, Some("r".to_string()));
        assert_eq!(compiled.edges[0].edge_types, vec!["KNOWS"]);
        assert_eq!(compiled.edges[0].direction, EdgeDirection::Outgoing);
        assert_eq!(compiled.edges[0].from_variable, "a");
        assert_eq!(compiled.edges[0].to_variable, "b");

        // Should return 3 variables
        assert_eq!(compiled.return_spec.variables.len(), 3);
    }

    #[test]
    fn test_parse_where_clause() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person) WHERE p.age > 25 RETURN p";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.nodes.len(), 1);
        assert_eq!(compiled.where_clauses.len(), 1);

        match &compiled.where_clauses[0] {
            WhereClause::Property { variable, property, constraint } => {
                assert_eq!(variable, "p");
                assert_eq!(property, "age");
                match constraint {
                    PropertyConstraint::GreaterThan(val) => {
                        assert_eq!(val, &serde_json::Value::Number(serde_json::Number::from(25)));
                    }
                    _ => panic!("Expected GreaterThan constraint"),
                }
            }
            _ => panic!("Expected Property where clause"),
        }
    }

    #[test]
    fn test_parse_where_and_condition() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person) WHERE p.age > 25 AND p.name = \"Alice\" RETURN p";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.where_clauses.len(), 1);

        match &compiled.where_clauses[0] {
            WhereClause::And(left, right) => {
                // Verify left side (age > 25)
                match left.as_ref() {
                    WhereClause::Property { variable, property, .. } => {
                        assert_eq!(variable, "p");
                        assert_eq!(property, "age");
                    }
                    _ => panic!("Expected Property clause"),
                }

                // Verify right side (name = "Alice")
                match right.as_ref() {
                    WhereClause::Property { variable, property, .. } => {
                        assert_eq!(variable, "p");
                        assert_eq!(property, "name");
                    }
                    _ => panic!("Expected Property clause"),
                }
            }
            _ => panic!("Expected And clause"),
        }
    }

    #[test]
    fn test_parse_aggregation_count() {
        let parser = QueryParser::new();
        let query = "MATCH (n:Person) RETURN COUNT(*)";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.projections.len(), 1);
        match &compiled.return_spec.projections[0] {
            PropertyProjection::Count => {} // Success
            _ => panic!("Expected Count projection"),
        }
    }

    #[test]
    fn test_parse_aggregation_sum() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person) RETURN SUM(p.salary)";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.projections.len(), 1);
        match &compiled.return_spec.projections[0] {
            PropertyProjection::Sum { variable, property } => {
                assert_eq!(variable, "p");
                assert_eq!(property, "salary");
            }
            _ => panic!("Expected Sum projection"),
        }
    }

    #[test]
    fn test_parse_property_projection() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person) RETURN p.name, p.age";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.projections.len(), 2);

        match &compiled.return_spec.projections[0] {
            PropertyProjection::Property { variable, property } => {
                assert_eq!(variable, "p");
                assert_eq!(property, "name");
            }
            _ => panic!("Expected Property projection"),
        }

        match &compiled.return_spec.projections[1] {
            PropertyProjection::Property { variable, property } => {
                assert_eq!(variable, "p");
                assert_eq!(property, "age");
            }
            _ => panic!("Expected Property projection"),
        }
    }

    #[test]
    fn test_parse_property_projection_with_alias() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person) RETURN p.name AS person_name";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.variables.len(), 1);
        assert_eq!(compiled.return_spec.variables[0], "person_name");

        match &compiled.return_spec.projections[0] {
            PropertyProjection::Property { variable, property } => {
                assert_eq!(variable, "p");
                assert_eq!(property, "name");
            }
            _ => panic!("Expected Property projection"),
        }
    }

    #[test]
    fn test_parse_return_with_limit() {
        let parser = QueryParser::new();
        let query = "MATCH (n:Person) RETURN n LIMIT 10";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.limit, Some(10));
    }

    #[test]
    fn test_parse_return_with_skip_and_limit() {
        let parser = QueryParser::new();
        let query = "MATCH (n:Person) RETURN n SKIP 5 LIMIT 10";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.skip, Some(5));
        assert_eq!(compiled.return_spec.limit, Some(10));
    }

    #[test]
    fn test_parse_return_with_order_by() {
        let parser = QueryParser::new();
        let query = "MATCH (p:Person) RETURN p ORDER BY p ASC";
        let compiled = parser.parse(query).unwrap();

        assert_eq!(compiled.return_spec.order_by.len(), 1);
        assert_eq!(compiled.return_spec.order_by[0].0, "p");
        assert_eq!(compiled.return_spec.order_by[0].1, true); // ASC
    }

    #[test]
    fn test_parse_return_distinct() {
        let parser = QueryParser::new();
        let query = "MATCH (n:Person) RETURN DISTINCT n";
        let compiled = parser.parse(query).unwrap();

        assert!(compiled.return_spec.distinct);
    }

    #[test]
    fn test_parse_complex_query() {
        let parser = QueryParser::new();
        let query = "MATCH (a:Person)-[r:KNOWS]->(b:Person) WHERE a.age > 25 AND b.age < 40 RETURN a.name, b.name, r ORDER BY a ASC LIMIT 20";
        let compiled = parser.parse(query).unwrap();

        // Verify nodes and edges
        assert_eq!(compiled.nodes.len(), 2);
        assert_eq!(compiled.edges.len(), 1);

        // Verify WHERE clause
        assert_eq!(compiled.where_clauses.len(), 1);

        // Verify RETURN spec
        assert_eq!(compiled.return_spec.projections.len(), 3);
        assert_eq!(compiled.return_spec.order_by.len(), 1);
        assert_eq!(compiled.return_spec.limit, Some(20));
    }
}
