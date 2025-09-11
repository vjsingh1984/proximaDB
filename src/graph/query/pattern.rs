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

//! # Pattern Matching Engine for Cypher-like Queries
//!
//! This module implements a pattern matching engine that supports Cypher-like syntax
//! for graph queries. It provides a simplified but powerful query language for
//! expressing graph patterns and relationships.
//!
//! ## Supported Patterns
//!
//! - **Node Patterns**: `(n:Label {property: value})`
//! - **Edge Patterns**: `-[r:TYPE {property: value}]->`
//! - **Path Patterns**: `(a)-[*1..3]->(b)` (variable length paths)
//! - **Complex Patterns**: `(a:Person)-[:KNOWS]->(b:Person)-[:WORKS_AT]->(c:Company)`
//!
//! ## Pattern Syntax Examples
//!
//! ```cypher
//! // Find all persons
//! MATCH (n:Person) RETURN n
//!
//! // Find persons with specific name
//! MATCH (n:Person {name: "Alice"}) RETURN n
//!
//! // Find friends of Alice
//! MATCH (alice:Person {name: "Alice"})-[:KNOWS]->(friend:Person) RETURN friend
//!
//! // Find paths of length 1-3
//! MATCH (a:Person)-[*1..3]->(b:Person) RETURN a, b
//!
//! // Complex pattern with multiple conditions
//! MATCH (p:Person)-[:WORKS_AT]->(c:Company {industry: "Tech"})
//! WHERE p.age > 25
//! RETURN p.name, c.name
//! ```

use super::ast::{
    CompiledPattern, EdgeDirection, EdgePattern, FoundPath, MatchResult,
    NodePattern, PathElement, PathPattern, PropertyConstraint, PropertyProjection,
    ReturnSpec, VariableBinding, WhereClause,
};
use super::{QueryContext, QueryResult};
use crate::core::error::ProximaDBError;
use crate::graph::{Edge, GraphMemoryPool, Node, NodeId};
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Pattern matcher for Cypher-like graph queries
pub struct PatternMatcher {
    /// Compiled patterns cache
    pattern_cache: HashMap<String, CompiledPattern>,
    /// Pattern compiler
    compiler: PatternCompiler,
}

/// Pattern compiler for parsing Cypher-like patterns
pub struct PatternCompiler {
    /// Regular expressions for pattern parsing
    node_pattern_regex: Regex,
    edge_pattern_regex: Regex,
    path_pattern_regex: Regex,
    property_pattern_regex: Regex,
}

impl PatternMatcher {
    /// Create a new pattern matcher
    pub fn new() -> QueryResult<Self> {
        Ok(Self {
            pattern_cache: HashMap::new(),
            compiler: PatternCompiler::new()?,
        })
    }

    /// Compile a pattern string into an executable pattern
    pub fn compile_pattern(&mut self, pattern_str: &str) -> QueryResult<CompiledPattern> {
        if let Some(cached) = self.pattern_cache.get(pattern_str) {
            return Ok(cached.clone());
        }

        let compiled = self.compiler.compile(pattern_str)?;
        self.pattern_cache
            .insert(pattern_str.to_string(), compiled.clone());

        Ok(compiled)
    }

    /// Execute a compiled pattern against the graph
    pub fn execute_pattern(
        &self,
        pattern: &CompiledPattern,
        memory_pool: &Arc<GraphMemoryPool>,
        context: &QueryContext,
    ) -> QueryResult<Vec<MatchResult>> {
        let mut results = Vec::new();

        // Start with node patterns to establish initial bindings
        let initial_candidates = self.find_initial_candidates(pattern, memory_pool)?;

        // For each initial candidate, try to extend the match
        for candidate in initial_candidates {
            if let Ok(matches) = self.extend_match(pattern, candidate, memory_pool, context) {
                results.extend(matches);
            }
        }

        // Apply WHERE clauses
        let filtered_results = self.apply_where_clauses(pattern, results, memory_pool)?;

        // Apply ordering and limits
        let ordered_results = self.apply_ordering_and_limits(pattern, filtered_results)?;

        Ok(ordered_results)
    }

    /// Find initial candidates based on node patterns
    fn find_initial_candidates(
        &self,
        pattern: &CompiledPattern,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<MatchResult>> {
        let mut candidates = Vec::new();

        // Find the most selective node pattern to start with
        let starting_pattern = pattern
            .nodes
            .iter()
            .min_by_key(|node| self.estimate_node_selectivity(node, memory_pool))
            .ok_or_else(|| {
                ProximaDBError::InvalidInput("Pattern must contain at least one node".to_string())
            })?;

        // Find matching nodes
        let matching_nodes = self.find_matching_nodes(starting_pattern, memory_pool)?;

        // Create initial candidates
        for node in matching_nodes {
            let mut bindings = HashMap::new();
            bindings.insert(
                starting_pattern.variable.clone(),
                VariableBinding::Node(node),
            );

            candidates.push(MatchResult {
                bindings,
                score: 1.0,
            });
        }

        Ok(candidates)
    }

    /// Extend a partial match with remaining patterns
    fn extend_match(
        &self,
        pattern: &CompiledPattern,
        candidate: MatchResult,
        memory_pool: &Arc<GraphMemoryPool>,
        _context: &QueryContext,
    ) -> QueryResult<Vec<MatchResult>> {
        let mut results = vec![candidate.clone()];

        // Process edge patterns
        for edge_pattern in &pattern.edges {
            let mut new_results = Vec::new();

            for result in results {
                if let Ok(extended) =
                    self.extend_with_edge_pattern(edge_pattern, result, memory_pool)
                {
                    new_results.extend(extended);
                }
            }

            results = new_results;
            if results.is_empty() {
                break; // No more matches possible
            }
        }

        // Process path patterns
        for path_pattern in &pattern.paths {
            let mut new_results = Vec::new();

            for result in results {
                if let Ok(extended) =
                    self.extend_with_path_pattern(path_pattern, result, memory_pool)
                {
                    new_results.extend(extended);
                }
            }

            results = new_results;
            if results.is_empty() {
                break;
            }
        }

        // Process remaining node patterns
        for node_pattern in &pattern.nodes {
            if candidate.bindings.contains_key(&node_pattern.variable) {
                continue; // Already bound
            }

            let mut new_results = Vec::new();

            for result in results {
                if let Ok(extended) =
                    self.extend_with_node_pattern(node_pattern, result, memory_pool)
                {
                    new_results.extend(extended);
                }
            }

            results = new_results;
            if results.is_empty() {
                break;
            }
        }

        Ok(results)
    }

    /// Extend match with an edge pattern
    fn extend_with_edge_pattern(
        &self,
        edge_pattern: &EdgePattern,
        candidate: MatchResult,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<MatchResult>> {
        let mut results = Vec::new();

        // Get source node
        let source_node = match candidate.bindings.get(&edge_pattern.from_variable) {
            Some(VariableBinding::Node(node)) => node.clone(),
            _ => return Ok(vec![]), // Source not bound or wrong type
        };

        // Find matching edges from this source
        let matching_edges =
            self.find_matching_edges_from_node(&source_node.id, edge_pattern, memory_pool)?;

        for (edge, target_node) in matching_edges {
            let mut new_bindings = candidate.bindings.clone();

            // Bind edge variable if specified
            if let Some(edge_var) = &edge_pattern.variable {
                new_bindings.insert(edge_var.clone(), VariableBinding::Edge(edge));
            }

            // Bind target node
            new_bindings.insert(
                edge_pattern.to_variable.clone(),
                VariableBinding::Node(target_node),
            );

            results.push(MatchResult {
                bindings: new_bindings,
                score: candidate.score,
            });
        }

        Ok(results)
    }

    /// Extend match with a path pattern
    fn extend_with_path_pattern(
        &self,
        path_pattern: &PathPattern,
        candidate: MatchResult,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<MatchResult>> {
        let mut results = Vec::new();

        // Get source node
        let source_node = match candidate.bindings.get(&path_pattern.from_variable) {
            Some(VariableBinding::Node(node)) => node.clone(),
            _ => return Ok(vec![]),
        };

        // Find paths of specified length
        let paths = self.find_variable_length_paths(&source_node.id, path_pattern, memory_pool)?;

        for path in paths {
            let mut new_bindings = candidate.bindings.clone();

            // Bind the path variable
            new_bindings.insert(
                path_pattern.variable.clone(),
                VariableBinding::Path(path.elements.clone()),
            );

            // Bind the target node
            if let Some(PathElement::Node(target_node)) = path.elements.last() {
                new_bindings.insert(
                    path_pattern.to_variable.clone(),
                    VariableBinding::Node(target_node.clone()),
                );
            }

            results.push(MatchResult {
                bindings: new_bindings,
                score: candidate.score * (1.0 / path.elements.len() as f64), // Prefer shorter paths
            });
        }

        Ok(results)
    }

    /// Extend match with a node pattern
    fn extend_with_node_pattern(
        &self,
        node_pattern: &NodePattern,
        candidate: MatchResult,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<MatchResult>> {
        let matching_nodes = self.find_matching_nodes(node_pattern, memory_pool)?;

        let mut results = Vec::new();
        for node in matching_nodes {
            let mut new_bindings = candidate.bindings.clone();
            new_bindings.insert(node_pattern.variable.clone(), VariableBinding::Node(node));

            results.push(MatchResult {
                bindings: new_bindings,
                score: candidate.score,
            });
        }

        Ok(results)
    }

    /// Find nodes matching a node pattern
    fn find_matching_nodes(
        &self,
        node_pattern: &NodePattern,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<Arc<Node>>> {
        let mut candidates = HashSet::new();

        // Start with label-based filtering if labels are specified
        if !node_pattern.labels.is_empty() {
            for label in &node_pattern.labels {
                if let Some(label_nodes) = memory_pool.label_indexes.get(label) {
                    for node_id in &*label_nodes {
                        candidates.insert(node_id.clone());
                    }
                } else {
                    return Ok(vec![]); // No nodes with this label
                }
            }
        } else {
            // No label filter - consider all nodes
            for entry in memory_pool.nodes.iter() {
                candidates.insert(entry.key().clone());
            }
        }

        // Apply property constraints
        let mut matching_nodes = Vec::new();
        for node_id in candidates {
            if let Some(node) = memory_pool.get_node(&node_id) {
                if self.node_matches_properties(&node, &node_pattern.properties)? {
                    matching_nodes.push(node);
                }
            }
        }

        Ok(matching_nodes)
    }

    /// Find edges matching an edge pattern from a specific node
    fn find_matching_edges_from_node(
        &self,
        from_node_id: &NodeId,
        edge_pattern: &EdgePattern,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<(Arc<Edge>, Arc<Node>)>> {
        // This is a simplified implementation
        // In a real CSR implementation, we would use adjacency lists
        let mut matching_edges = Vec::new();

        for edge_entry in memory_pool.edges.iter() {
            let edge = edge_entry.value();

            // Check if edge originates from our source node
            let is_valid_direction = match edge_pattern.direction {
                EdgeDirection::Outgoing => edge.from_node_id == *from_node_id,
                EdgeDirection::Incoming => edge.to_node_id == *from_node_id,
                EdgeDirection::Bidirectional => {
                    edge.from_node_id == *from_node_id || edge.to_node_id == *from_node_id
                }
            };

            if !is_valid_direction {
                continue;
            }

            // Check edge type
            if !edge_pattern.edge_types.is_empty() {
                if !edge_pattern.edge_types.contains(&edge.edge_type) {
                    continue;
                }
            }

            // Check edge properties
            if !self.edge_matches_properties(edge, &edge_pattern.properties)? {
                continue;
            }

            // Get target node
            let target_node_id = match edge_pattern.direction {
                EdgeDirection::Outgoing => &edge.to_node_id,
                EdgeDirection::Incoming => &edge.from_node_id,
                EdgeDirection::Bidirectional => {
                    if edge.from_node_id == *from_node_id {
                        &edge.to_node_id
                    } else {
                        &edge.from_node_id
                    }
                }
            };

            if let Some(target_node) = memory_pool.get_node(target_node_id) {
                matching_edges.push((edge.clone(), target_node));
            }
        }

        Ok(matching_edges)
    }

    /// Find variable length paths
    fn find_variable_length_paths(
        &self,
        from_node_id: &NodeId,
        path_pattern: &PathPattern,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<FoundPath>> {
        let mut paths = Vec::new();
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();

        // Start BFS from source node
        if let Some(start_node) = memory_pool.get_node(from_node_id) {
            queue.push_back(FoundPath {
                elements: vec![PathElement::Node(start_node)],
                length: 0,
            });
        }

        while let Some(current_path) = queue.pop_front() {
            // Skip if we've exceeded max length
            if current_path.length >= path_pattern.max_length {
                continue;
            }

            // Get current node
            let current_node = match current_path.elements.last() {
                Some(PathElement::Node(node)) => node,
                _ => continue,
            };

            // Prevent cycles
            let visited_key = format!("{}:{}", current_node.id, current_path.length);
            if visited.contains(&visited_key) {
                continue;
            }
            visited.insert(visited_key);

            // If we've reached minimum length, add to results
            if current_path.length >= path_pattern.min_length {
                paths.push(current_path.clone());
            }

            // Find outgoing edges
            for edge_entry in memory_pool.edges.iter() {
                let edge = edge_entry.value();

                // Check if edge starts from current node and matches pattern
                if edge.from_node_id != current_node.id {
                    continue;
                }

                if !path_pattern.edge_types.is_empty() {
                    if !path_pattern.edge_types.contains(&edge.edge_type) {
                        continue;
                    }
                }

                // Get target node
                if let Some(target_node) = memory_pool.get_node(&edge.to_node_id) {
                    let mut new_path = current_path.clone();
                    new_path.elements.push(PathElement::Edge(edge.clone()));
                    new_path.elements.push(PathElement::Node(target_node));
                    new_path.length += 1;

                    queue.push_back(new_path);
                }
            }
        }

        Ok(paths)
    }

    /// Check if node matches property constraints
    fn node_matches_properties(
        &self,
        node: &Node,
        constraints: &HashMap<String, PropertyConstraint>,
    ) -> QueryResult<bool> {
        for (prop_name, constraint) in constraints {
            if let Some(prop_value) = node.properties.get(prop_name) {
                if !self.evaluate_property_constraint(prop_value, constraint)? {
                    return Ok(false);
                }
            } else {
                // Property doesn't exist
                match constraint {
                    PropertyConstraint::NotExists => {} // This is what we want
                    PropertyConstraint::Exists => return Ok(false),
                    _ => return Ok(false), // Other constraints fail if property doesn't exist
                }
            }
        }

        Ok(true)
    }

    /// Check if edge matches property constraints
    fn edge_matches_properties(
        &self,
        edge: &Edge,
        constraints: &HashMap<String, PropertyConstraint>,
    ) -> QueryResult<bool> {
        for (prop_name, constraint) in constraints {
            if let Some(prop_value) = edge.properties.get(prop_name) {
                if !self.evaluate_property_constraint(prop_value, constraint)? {
                    return Ok(false);
                }
            } else {
                match constraint {
                    PropertyConstraint::NotExists => {}
                    PropertyConstraint::Exists => return Ok(false),
                    _ => return Ok(false),
                }
            }
        }

        Ok(true)
    }

    /// Evaluate a property constraint
    fn evaluate_property_constraint(
        &self,
        value: &crate::proto::proximadb_v1::PropertyValue,
        constraint: &PropertyConstraint,
    ) -> QueryResult<bool> {
        // Convert PropertyValue to JSON for easier comparison
        let json_value = self.property_value_to_json(value);

        match constraint {
            PropertyConstraint::Equals(expected) => Ok(json_value == *expected),
            PropertyConstraint::NotEquals(expected) => Ok(json_value != *expected),
            PropertyConstraint::GreaterThan(expected) => self
                .compare_values(&json_value, expected)
                .map(|cmp| cmp > 0),
            PropertyConstraint::GreaterThanOrEqual(expected) => self
                .compare_values(&json_value, expected)
                .map(|cmp| cmp >= 0),
            PropertyConstraint::LessThan(expected) => self
                .compare_values(&json_value, expected)
                .map(|cmp| cmp < 0),
            PropertyConstraint::LessThanOrEqual(expected) => self
                .compare_values(&json_value, expected)
                .map(|cmp| cmp <= 0),
            PropertyConstraint::In(values) => Ok(values.contains(&json_value)),
            PropertyConstraint::NotIn(values) => Ok(!values.contains(&json_value)),
            PropertyConstraint::Contains(substring) => {
                if let serde_json::Value::String(s) = &json_value {
                    Ok(s.contains(substring))
                } else {
                    Ok(false)
                }
            }
            PropertyConstraint::StartsWith(prefix) => {
                if let serde_json::Value::String(s) = &json_value {
                    Ok(s.starts_with(prefix))
                } else {
                    Ok(false)
                }
            }
            PropertyConstraint::EndsWith(suffix) => {
                if let serde_json::Value::String(s) = &json_value {
                    Ok(s.ends_with(suffix))
                } else {
                    Ok(false)
                }
            }
            PropertyConstraint::Regex(pattern) => {
                if let serde_json::Value::String(s) = &json_value {
                    let regex = Regex::new(pattern).map_err(|e| {
                        ProximaDBError::InvalidInput(format!("Invalid regex: {}", e))
                    })?;
                    Ok(regex.is_match(s))
                } else {
                    Ok(false)
                }
            }
            PropertyConstraint::Exists => Ok(true), // We already know it exists
            PropertyConstraint::NotExists => Ok(false), // We already know it exists
        }
    }

    /// Compare two JSON values
    fn compare_values(&self, a: &serde_json::Value, b: &serde_json::Value) -> QueryResult<i32> {
        use serde_json::Value;

        match (a, b) {
            (Value::Number(n1), Value::Number(n2)) => {
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
                Ok(f1.partial_cmp(&f2).unwrap_or(std::cmp::Ordering::Equal) as i32)
            }
            (Value::String(s1), Value::String(s2)) => Ok(s1.cmp(s2) as i32),
            (Value::Bool(b1), Value::Bool(b2)) => Ok(b1.cmp(b2) as i32),
            _ => Err(ProximaDBError::InvalidInput(
                "Cannot compare values of different types".to_string(),
            )),
        }
    }

    /// Convert PropertyValue to JSON
    fn property_value_to_json(
        &self,
        value: &crate::proto::proximadb_v1::PropertyValue,
    ) -> serde_json::Value {
        match &value.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                serde_json::Value::String(s.clone())
            }
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
                serde_json::Value::Number(serde_json::Number::from(*i))
            }
            Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                serde_json::Value::Number(serde_json::Number::from_f64(*d).unwrap_or(serde_json::Number::from(0)))
            }
            Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
                serde_json::Value::Bool(*b)
            }
            Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(_)) => {
                serde_json::Value::String("BYTES".to_string()) // Simplified
            }
            Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(_)) => {
                serde_json::Value::Array(vec![]) // Simplified
            }
            Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_)) => {
                serde_json::Value::Object(serde_json::Map::new()) // Simplified
            }
            None => serde_json::Value::Null,
        }
    }

    /// Apply WHERE clauses to filter results
    fn apply_where_clauses(
        &self,
        pattern: &CompiledPattern,
        results: Vec<MatchResult>,
        _memory_pool: &Arc<GraphMemoryPool>,
    ) -> QueryResult<Vec<MatchResult>> {
        if pattern.where_clauses.is_empty() {
            return Ok(results);
        }

        let mut filtered = Vec::new();

        for result in results {
            if self.evaluate_where_clauses(&pattern.where_clauses, &result.bindings)? {
                filtered.push(result);
            }
        }

        Ok(filtered)
    }

    /// Evaluate WHERE clauses for a binding set
    fn evaluate_where_clauses(
        &self,
        where_clauses: &[WhereClause],
        bindings: &HashMap<String, VariableBinding>,
    ) -> QueryResult<bool> {
        // Simplified evaluation - just check first clause for now
        if let Some(clause) = where_clauses.first() {
            if let Some(VariableBinding::Node(node)) = bindings.get(&clause.variable) {
                if let Some(prop_value) = node.properties.get(&clause.property) {
                    return self.evaluate_property_constraint(prop_value, &clause.constraint);
                }
            }
        }

        Ok(true)
    }

    /// Apply ordering and limits to results
    fn apply_ordering_and_limits(
        &self,
        pattern: &CompiledPattern,
        mut results: Vec<MatchResult>,
    ) -> QueryResult<Vec<MatchResult>> {
        // Sort by score (descending) for now
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply limit if specified
        if let Some(limit) = pattern.return_spec.limit {
            results.truncate(limit as usize);
        }

        Ok(results)
    }

    /// Estimate selectivity of a node pattern
    fn estimate_node_selectivity(
        &self,
        node_pattern: &NodePattern,
        memory_pool: &Arc<GraphMemoryPool>,
    ) -> usize {
        if !node_pattern.labels.is_empty() {
            // Use first label for estimation
            memory_pool
                .label_indexes
                .get(&node_pattern.labels[0])
                .map(|nodes| nodes.len())
                .unwrap_or(memory_pool.node_count())
        } else {
            memory_pool.node_count()
        }
    }
}

impl PatternCompiler {
    /// Create a new pattern compiler
    pub fn new() -> QueryResult<Self> {
        Ok(Self {
            node_pattern_regex: Regex::new(
                r"\(([a-zA-Z_][a-zA-Z0-9_]*):?([a-zA-Z_][a-zA-Z0-9_]*)?\s*(\{[^}]*\})?\)",
            )
            .map_err(|e| {
                ProximaDBError::Internal(format!("Failed to compile node regex: {}", e))
            })?,
            edge_pattern_regex: Regex::new(
                r"-\[([a-zA-Z_][a-zA-Z0-9_]*)?:?([a-zA-Z_][a-zA-Z0-9_]*)?\s*(\{[^}]*\})?\]->?",
            )
            .map_err(|e| {
                ProximaDBError::Internal(format!("Failed to compile edge regex: {}", e))
            })?,
            path_pattern_regex: Regex::new(r"-\[\*([0-9]+)\.\.([0-9]+)\]->?").map_err(|e| {
                ProximaDBError::Internal(format!("Failed to compile path regex: {}", e))
            })?,
            property_pattern_regex: Regex::new(r"([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*([^,}]+)")
                .map_err(|e| {
                    ProximaDBError::Internal(format!("Failed to compile property regex: {}", e))
                })?,
        })
    }

    /// Compile a pattern string into a CompiledPattern
    pub fn compile(&self, pattern_str: &str) -> QueryResult<CompiledPattern> {
        // This is a simplified compiler - a real implementation would need
        // a proper parser for the full Cypher syntax

        let mut compiled = CompiledPattern {
            nodes: Vec::new(),
            edges: Vec::new(),
            paths: Vec::new(),
            where_clauses: Vec::new(),
            return_spec: ReturnSpec {
                variables: Vec::new(),
                projections: Vec::new(),
                distinct: false,
                order_by: Vec::new(),
                limit: None,
                skip: None,
            },
            variables: HashMap::new(),
        };

        // Extract MATCH clause
        let match_clause = self.extract_match_clause(pattern_str)?;

        // Parse nodes
        for cap in self.node_pattern_regex.captures_iter(&match_clause) {
            let variable = cap
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let label = cap.get(2).map(|m| m.as_str().to_string());
            let properties = cap
                .get(3)
                .map(|m| self.parse_property_map(m.as_str()))
                .transpose()?
                .unwrap_or_default();

            compiled.nodes.push(NodePattern {
                variable,
                labels: label.map(|l| vec![l]).unwrap_or_default(),
                properties,
                optional: false,
            });
        }

        // Parse RETURN clause
        if let Some(return_clause) = self.extract_return_clause(pattern_str) {
            compiled.return_spec = self.parse_return_clause(&return_clause)?;
        }

        Ok(compiled)
    }

    /// Extract MATCH clause from pattern
    fn extract_match_clause(&self, pattern: &str) -> QueryResult<String> {
        if let Some(start) = pattern.find("MATCH") {
            let after_match = &pattern[start + 5..];

            // Find end of MATCH clause (next keyword or end of string)
            let end_keywords = ["WHERE", "RETURN", "ORDER BY", "LIMIT"];
            let mut end_pos = after_match.len();

            for keyword in &end_keywords {
                if let Some(pos) = after_match.find(keyword) {
                    end_pos = end_pos.min(pos);
                }
            }

            Ok(after_match[..end_pos].trim().to_string())
        } else {
            // No explicit MATCH - treat entire string as match clause
            Ok(pattern.to_string())
        }
    }

    /// Extract RETURN clause from pattern
    fn extract_return_clause(&self, pattern: &str) -> Option<String> {
        if let Some(start) = pattern.find("RETURN") {
            let after_return = &pattern[start + 6..];

            // Find end of RETURN clause
            let end_keywords = ["ORDER BY", "LIMIT", "SKIP"];
            let mut end_pos = after_return.len();

            for keyword in &end_keywords {
                if let Some(pos) = after_return.find(keyword) {
                    end_pos = end_pos.min(pos);
                }
            }

            Some(after_return[..end_pos].trim().to_string())
        } else {
            None
        }
    }

    /// Parse property map from string like "{name: 'Alice', age: 30}"
    fn parse_property_map(
        &self,
        prop_str: &str,
    ) -> QueryResult<HashMap<String, PropertyConstraint>> {
        let mut properties = HashMap::new();

        // Remove braces
        let inner = prop_str.trim_matches(|c| c == '{' || c == '}');

        // Parse each property
        for cap in self.property_pattern_regex.captures_iter(inner) {
            let prop_name = cap
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let prop_value_str = cap.get(2).map(|m| m.as_str().trim()).unwrap_or_default();

            // Parse value (simplified)
            let value = if prop_value_str.starts_with('\'') || prop_value_str.starts_with('"') {
                // String value
                let unquoted = prop_value_str.trim_matches(|c| c == '\'' || c == '"');
                serde_json::Value::String(unquoted.to_string())
            } else if let Ok(int_val) = prop_value_str.parse::<i64>() {
                // Integer value
                serde_json::Value::Number(serde_json::Number::from(int_val))
            } else if let Ok(float_val) = prop_value_str.parse::<f64>() {
                // Float value
                serde_json::Value::Number(
                    serde_json::Number::from_f64(float_val).unwrap_or(serde_json::Number::from(0)),
                )
            } else if prop_value_str == "true" || prop_value_str == "false" {
                // Boolean value
                serde_json::Value::Bool(prop_value_str == "true")
            } else {
                // Default to string
                serde_json::Value::String(prop_value_str.to_string())
            };

            properties.insert(prop_name, PropertyConstraint::Equals(value));
        }

        Ok(properties)
    }

    /// Parse RETURN clause
    fn parse_return_clause(&self, return_str: &str) -> QueryResult<ReturnSpec> {
        let mut spec = ReturnSpec {
            variables: Vec::new(),
            projections: Vec::new(),
            distinct: false,
            order_by: Vec::new(),
            limit: None,
            skip: None,
        };

        // Check for DISTINCT
        let cleaned = if return_str
            .trim_start()
            .to_uppercase()
            .starts_with("DISTINCT")
        {
            spec.distinct = true;
            return_str.trim_start()[8..].trim()
        } else {
            return_str.trim()
        };

        // Split by comma and parse each item
        for item in cleaned.split(',') {
            let item = item.trim();

            if item.contains('.') {
                // Property projection like "n.name"
                let parts: Vec<&str> = item.split('.').collect();
                if parts.len() == 2 {
                    spec.projections.push(PropertyProjection {
                        variable: parts[0].to_string(),
                        property: parts[1].to_string(),
                        alias: None,
                    });
                }
            } else {
                // Variable like "n"
                spec.variables.push(item.to_string());
            }
        }

        Ok(spec)
    }
}

impl Default for PatternMatcher {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphMemoryPool;
    use crate::proto::proximadb_v1::{PropertyValue, property_value::Value};

    #[test]
    fn test_pattern_compiler_creation() {
        let compiler = PatternCompiler::new().unwrap();
        assert!(compiler.node_pattern_regex.is_match("(n:Person)"));
        assert!(
            compiler
                .node_pattern_regex
                .is_match("(alice:Person {name: 'Alice'})")
        );
    }

    #[test]
    fn test_pattern_matcher_creation() {
        let matcher = PatternMatcher::new().unwrap();
        assert_eq!(matcher.pattern_cache.len(), 0);
    }

    #[test]
    fn test_simple_pattern_compilation() {
        let mut matcher = PatternMatcher::new().unwrap();

        let pattern = matcher
            .compile_pattern("MATCH (n:Person) RETURN n")
            .unwrap();

        assert_eq!(pattern.nodes.len(), 1);
        assert_eq!(pattern.nodes[0].variable, "n");
        assert_eq!(pattern.nodes[0].labels[0], "Person");
        assert_eq!(pattern.return_spec.variables.len(), 1);
        assert_eq!(pattern.return_spec.variables[0], "n");
    }

    #[test]
    fn test_property_constraint_evaluation() {
        let matcher = PatternMatcher::new().unwrap();

        let prop_value = PropertyValue {
            value: Some(Value::StringValue("Alice".to_string())),
        };

        let constraint = PropertyConstraint::Equals(serde_json::Value::String("Alice".to_string()));

        assert!(
            matcher
                .evaluate_property_constraint(&prop_value, &constraint)
                .unwrap()
        );

        let different_constraint =
            PropertyConstraint::Equals(serde_json::Value::String("Bob".to_string()));
        assert!(
            !matcher
                .evaluate_property_constraint(&prop_value, &different_constraint)
                .unwrap()
        );
    }

    #[test]
    fn test_property_value_to_json() {
        let matcher = PatternMatcher::new().unwrap();

        let string_prop = PropertyValue {
            value: Some(Value::StringValue("test".to_string())),
        };
        assert_eq!(
            matcher.property_value_to_json(&string_prop),
            serde_json::Value::String("test".to_string())
        );

        let int_prop = PropertyValue {
            value: Some(Value::IntValue(42)),
        };
        assert_eq!(
            matcher.property_value_to_json(&int_prop),
            serde_json::Value::Number(serde_json::Number::from(42))
        );

        let bool_prop = PropertyValue {
            value: Some(Value::BoolValue(true)),
        };
        assert_eq!(
            matcher.property_value_to_json(&bool_prop),
            serde_json::Value::Bool(true)
        );
    }
}
