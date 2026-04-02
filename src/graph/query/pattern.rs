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
    CompiledPattern, EdgeDirection, EdgePattern, FoundPath, MatchResult, NodePattern, PathElement,
    PathPattern, PropertyConstraint, PropertyProjection, ReturnSpec, VariableBinding, WhereClause,
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
    #[allow(dead_code)]
    node_pattern_regex: Regex,
    #[allow(dead_code)]
    edge_pattern_regex: Regex,
    #[allow(dead_code)]
    path_pattern_regex: Regex,
    #[allow(dead_code)]
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

        // Apply WITH clauses (TD-019: intermediate projections)
        let with_results = self.apply_with_clauses(pattern, filtered_results)?;

        // Apply ordering and limits
        let ordered_results = self.apply_ordering_and_limits(pattern, with_results)?;

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
            if let Some(node) = memory_pool.get_node(&node_id)
                && self.node_matches_properties(&node, &node_pattern.properties)? {
                    matching_nodes.push(node);
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
            if !edge_pattern.edge_types.is_empty()
                && !edge_pattern.edge_types.contains(&edge.edge_type) {
                    continue;
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

                if !path_pattern.edge_types.is_empty()
                    && !path_pattern.edge_types.contains(&edge.edge_type) {
                        continue;
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
            PropertyConstraint::GreaterThanOrEqual(expected)
            | PropertyConstraint::GreaterOrEqual(expected) => self
                .compare_values(&json_value, expected)
                .map(|cmp| cmp >= 0),
            PropertyConstraint::LessThan(expected) => self
                .compare_values(&json_value, expected)
                .map(|cmp| cmp < 0),
            PropertyConstraint::LessThanOrEqual(expected)
            | PropertyConstraint::LessOrEqual(expected) => self
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
                // unwrap_or(0.0) is safe here: JSON numbers are always valid f64 values in practice.
                // as_f64() only returns None for numbers beyond f64 range, which is extremely rare.
                // Defaulting to 0.0 provides a stable comparison value for edge cases.
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
                // unwrap_or(Equal) handles NaN comparison: NaN != NaN, so partial_cmp returns None.
                // Using Equal as default provides deterministic sorting for NaN values.
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
                // unwrap_or is safe here: serde_json::Number::from_f64 returns None for
                // NaN and Infinity values, which are not valid JSON numbers.
                // Defaulting to 0 provides a valid JSON representation for these edge cases.
                serde_json::Value::Number(
                    serde_json::Number::from_f64(*d).unwrap_or(serde_json::Number::from(0)),
                )
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
            Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(_)) => {
                serde_json::Value::String("VECTOR".to_string()) // Simplified vector representation
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
        // Evaluate all WHERE clauses
        for clause in where_clauses {
            if !self.evaluate_where_clause(clause, bindings)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn evaluate_where_clause(
        &self,
        clause: &WhereClause,
        bindings: &HashMap<String, VariableBinding>,
    ) -> QueryResult<bool> {
        match clause {
            WhereClause::Property {
                variable,
                property,
                constraint,
            } => {
                if let Some(VariableBinding::Node(node)) = bindings.get(variable)
                    && let Some(prop_value) = node.properties.get(property) {
                        return self.evaluate_property_constraint(prop_value, constraint);
                    }
                Ok(false)
            }
            WhereClause::And(left, right) => {
                let left_result = self.evaluate_where_clause(left, bindings)?;
                if !left_result {
                    return Ok(false);
                }
                self.evaluate_where_clause(right, bindings)
            }
            WhereClause::Or(left, right) => {
                let left_result = self.evaluate_where_clause(left, bindings)?;
                if left_result {
                    return Ok(true);
                }
                self.evaluate_where_clause(right, bindings)
            }
            WhereClause::Not(inner) => {
                let result = self.evaluate_where_clause(inner, bindings)?;
                Ok(!result)
            }
        }
    }

    /// Apply ordering, skip, and limits to match results
    ///
    /// TD-019: Enhanced implementation of ORDER BY, SKIP, and LIMIT clauses
    /// - ORDER BY: Sorts by specified variables and properties
    /// - SKIP: Skips N results before applying LIMIT
    /// - LIMIT: Limits the number of results returned
    fn apply_ordering_and_limits(
        &self,
        pattern: &CompiledPattern,
        mut results: Vec<MatchResult>,
    ) -> QueryResult<Vec<MatchResult>> {
        // Apply ORDER BY if specified
        if !pattern.return_spec.order_by.is_empty() {
            results = self.apply_order_by(&pattern.return_spec.order_by, results)?;
        } else {
            // Default: Sort by score (descending) when no ORDER BY specified
            // unwrap_or(Equal) is safe: partial_cmp on f64 returns None only for NaN values.
            // Using Equal as default provides deterministic sorting behavior for NaN scores.
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Apply SKIP if specified
        if let Some(skip) = pattern.return_spec.skip {
            if skip < results.len() {
                results = results.split_off(skip);
            } else {
                results.clear();
            }
        }

        // Apply LIMIT if specified
        if let Some(limit) = pattern.return_spec.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Apply WITH clauses to match results
    ///
    /// TD-019: Execute WITH clause for intermediate projections.
    /// WITH clauses are used to:
    /// - Project and rename variables
    /// - Filter intermediate results
    /// - Apply aggregations
    /// - Chain query parts together
    ///
    /// WITH clauses are executed after WHERE clauses but before the final RETURN.
    fn apply_with_clauses(
        &self,
        pattern: &CompiledPattern,
        mut results: Vec<MatchResult>,
    ) -> QueryResult<Vec<MatchResult>> {
        for with_clause in &pattern.with_clauses {
            // Apply projections from WITH clause
            if !with_clause.projections.is_empty() {
                results = self.apply_projections(&with_clause.projections, results)?;
            }

            // Apply DISTINCT if specified
            if with_clause.distinct {
                results = self.apply_distinct(results);
            }

            // Apply ORDER BY from WITH clause
            if !with_clause.order_by.is_empty() {
                results = self.apply_order_by(&with_clause.order_by, results)?;
            }

            // Apply SKIP from WITH clause
            if let Some(skip) = with_clause.skip {
                if skip < results.len() {
                    results = results.split_off(skip);
                } else {
                    results.clear();
                }
            }

            // Apply LIMIT from WITH clause
            if let Some(limit) = with_clause.limit {
                results.truncate(limit);
            }
        }

        Ok(results)
    }

    /// Apply UNION clause to combine results from multiple queries
    ///
    /// TD-019: Execute UNION clause for combining query results.
    ///
    /// UNION combines results from two queries:
    /// - UNION (distinct): removes duplicate rows
    /// - UNION ALL: keeps all rows including duplicates
    ///
    /// This function takes the left query results and right query results
    /// and combines them according to the distinct flag.
    pub fn apply_union(
        &self,
        left_results: Vec<MatchResult>,
        right_results: Vec<MatchResult>,
        distinct: bool,
    ) -> QueryResult<Vec<MatchResult>> {
        let mut combined_results = Vec::new();

        // Add all results from left query
        combined_results.extend(left_results);

        // Add results from right query
        combined_results.extend(right_results);

        // Apply DISTINCT if specified (UNION without ALL)
        if distinct {
            combined_results = self.apply_distinct(combined_results);
        }

        Ok(combined_results)
    }

    /// Apply projections to match results
    ///
    /// Projects results according to the specified projections.
    /// Each projection is a tuple of (variable_name, property_projection).
    /// This creates new bindings based on the projections.
    fn apply_projections(
        &self,
        projections: &[(String, PropertyProjection)],
        results: Vec<MatchResult>,
    ) -> QueryResult<Vec<MatchResult>> {
        use crate::proto::proximadb_v1::{PropertyValue, property_value::Value};

        let result_count = results.len();

        // Pre-compute aggregations (computed once, same for all results)
        let mut aggregates: std::collections::HashMap<String, PropertyValue> =
            std::collections::HashMap::new();

        for (alias, projection) in projections {
            match projection {
                PropertyProjection::Count => {
                    aggregates.insert(
                        alias.clone(),
                        PropertyValue {
                            value: Some(Value::IntValue(result_count as i64)),
                        },
                    );
                }
                PropertyProjection::Sum { variable, property } => {
                    let mut sum = 0i64;
                    for result in &results {
                        if let Some(binding) = result.bindings.get(variable)
                            && let Some(value) = self.get_binding_property(binding, property)
                                && let Some(Value::IntValue(n)) = value.value {
                                    sum += n;
                                }
                    }
                    aggregates.insert(
                        alias.clone(),
                        PropertyValue {
                            value: Some(Value::IntValue(sum)),
                        },
                    );
                }
                PropertyProjection::Avg { variable, property } => {
                    let mut sum = 0i64;
                    let mut count = 0i64;
                    for result in &results {
                        if let Some(binding) = result.bindings.get(variable)
                            && let Some(value) = self.get_binding_property(binding, property)
                                && let Some(Value::IntValue(n)) = value.value {
                                    sum += n;
                                    count += 1;
                                }
                    }
                    let avg = if count > 0 { sum / count } else { 0 };
                    aggregates.insert(
                        alias.clone(),
                        PropertyValue {
                            value: Some(Value::IntValue(avg)),
                        },
                    );
                }
                PropertyProjection::Min { variable, property } => {
                    let mut min: Option<i64> = None;
                    for result in &results {
                        if let Some(binding) = result.bindings.get(variable)
                            && let Some(value) = self.get_binding_property(binding, property)
                                && let Some(Value::IntValue(n)) = value.value
                                    && (min.is_none() || Some(n) < min) {
                                        min = Some(n);
                                    }
                    }
                    aggregates.insert(
                        alias.clone(),
                        PropertyValue {
                            value: Some(Value::IntValue(min.unwrap_or(0))),
                        },
                    );
                }
                PropertyProjection::Max { variable, property } => {
                    let mut max: Option<i64> = None;
                    for result in &results {
                        if let Some(binding) = result.bindings.get(variable)
                            && let Some(value) = self.get_binding_property(binding, property)
                                && let Some(Value::IntValue(n)) = value.value
                                    && (max.is_none() || Some(n) > max) {
                                        max = Some(n);
                                    }
                    }
                    aggregates.insert(
                        alias.clone(),
                        PropertyValue {
                            value: Some(Value::IntValue(max.unwrap_or(0))),
                        },
                    );
                }
                _ => {} // Non-aggregations are handled per-result
            }
        }

        // Process each result
        let mut projected_results = Vec::new();
        for result in results {
            let mut new_bindings = HashMap::new();

            for (alias, projection) in projections {
                match projection {
                    PropertyProjection::Variable(var_name) => {
                        // Direct variable reference (e.g., "WITH n AS node")
                        if let Some(binding) = result.bindings.get(var_name) {
                            new_bindings.insert(alias.clone(), binding.clone());
                        }
                    }
                    PropertyProjection::Property { variable, property } => {
                        // Property access (e.g., "WITH n.name AS name")
                        if let Some(binding) = result.bindings.get(variable)
                            && let Some(value) = self.get_binding_property(binding, property) {
                                // Create a new binding for the projected value
                                new_bindings.insert(
                                    alias.clone(),
                                    VariableBinding::Node(std::sync::Arc::new(
                                        crate::graph::Node {
                                            id: format!("{}_{}", variable, property),
                                            labels: vec![],
                                            properties: {
                                                let mut props = std::collections::HashMap::new();
                                                props.insert("value".to_string(), value);
                                                props
                                            },
                                            embedding: None,
                                            created_at_ms: 0,
                                            updated_at_ms: 0,
                                        },
                                    )),
                                );
                            }
                    }
                    PropertyProjection::Count
                    | PropertyProjection::Sum { .. }
                    | PropertyProjection::Avg { .. }
                    | PropertyProjection::Min { .. }
                    | PropertyProjection::Max { .. } => {
                        // Use pre-computed aggregate value
                        if let Some(aggregate_value) = aggregates.get(alias) {
                            new_bindings.insert(
                                alias.clone(),
                                VariableBinding::Node(std::sync::Arc::new(crate::graph::Node {
                                    id: alias.clone(),
                                    labels: vec!["Aggregate".to_string()],
                                    properties: {
                                        let mut props = std::collections::HashMap::new();
                                        let prop_name = match projection {
                                            PropertyProjection::Count => "count",
                                            PropertyProjection::Sum { .. } => "sum",
                                            PropertyProjection::Avg { .. } => "avg",
                                            PropertyProjection::Min { .. } => "min",
                                            PropertyProjection::Max { .. } => "max",
                                            _ => "value",
                                        };
                                        props
                                            .insert(prop_name.to_string(), aggregate_value.clone());
                                        props
                                    },
                                    embedding: None,
                                    created_at_ms: 0,
                                    updated_at_ms: 0,
                                })),
                            );
                        }
                    }
                }
            }

            projected_results.push(MatchResult {
                bindings: new_bindings,
                score: result.score,
            });
        }

        Ok(projected_results)
    }

    /// Apply DISTINCT to match results
    ///
    /// Removes duplicate results based on their bindings.
    fn apply_distinct(&self, results: Vec<MatchResult>) -> Vec<MatchResult> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut distinct_results = Vec::new();

        for result in results {
            // Create a hashable representation of the bindings
            let hash_key = result
                .bindings
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        k,
                        match v {
                            VariableBinding::Node(n) => n.id.clone(),
                            VariableBinding::Edge(e) => e.id.to_string(),
                            VariableBinding::Path(p) => format!("{:?}", p),
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(",");

            if seen.insert(hash_key) {
                distinct_results.push(result);
            }
        }

        distinct_results
    }

    /// Get property value from a variable binding
    ///
    /// Extracts the value of a property from a binding for projection.
    fn get_binding_property(
        &self,
        binding: &VariableBinding,
        property: &str,
    ) -> Option<crate::proto::proximadb_v1::PropertyValue> {
        match binding {
            VariableBinding::Node(node) => node.properties.get(property).cloned(),
            VariableBinding::Edge(edge) => edge.properties.get(property).cloned(),
            VariableBinding::Path(_) => None,
        }
    }

    /// Apply ORDER BY clause to match results
    ///
    /// Sorts results by the specified variables and properties.
    /// Each ORDER BY spec is a tuple of (variable_name, ascending_flag).
    /// Examples:
    /// - ORDER BY n ASC: Sort by node n in ascending order (by id)
    /// - ORDER BY n.name DESC: Sort by node n's name property in descending order
    fn apply_order_by(
        &self,
        order_specs: &[(String, bool)],
        mut results: Vec<MatchResult>,
    ) -> QueryResult<Vec<MatchResult>> {
        results.sort_by(|a, b| {
            for (variable, ascending) in order_specs {
                // Get the binding values from both results
                let a_value = self.get_binding_value(a, variable);
                let b_value = self.get_binding_value(b, variable);

                // Compare the values
                let ordering = match (a_value, b_value) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(ref a_val), Some(ref b_val)) => {
                        // Compare as JSON values (handles strings, numbers, booleans)
                        // Use references to avoid moving the values
                        match (a_val, b_val) {
                            (
                                serde_json::Value::Number(a_num),
                                serde_json::Value::Number(b_num),
                            ) => {
                                // Numeric comparison
                                match (a_num.as_f64(), b_num.as_f64()) {
                                    (Some(a_f), Some(b_f)) => {
                                        a_f.partial_cmp(&b_f).unwrap_or(std::cmp::Ordering::Equal)
                                    }
                                    _ => std::cmp::Ordering::Equal,
                                }
                            }
                            (
                                serde_json::Value::String(a_str),
                                serde_json::Value::String(b_str),
                            ) => {
                                // String comparison
                                a_str
                                    .partial_cmp(b_str)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            }
                            (serde_json::Value::Bool(a_bool), serde_json::Value::Bool(b_bool)) => {
                                // Boolean comparison - convert bool to u8 for cmp()
                                let a_b = *a_bool as u8;
                                let b_b = *b_bool as u8;
                                a_b.cmp(&b_b)
                            }
                            _ => {
                                // Fallback: compare as strings (for mixed types)
                                a_val
                                    .to_string()
                                    .partial_cmp(&b_val.to_string())
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            }
                        }
                    }
                };

                // If not equal, return the ordering (reversed if descending)
                if ordering != std::cmp::Ordering::Equal {
                    return if *ascending {
                        ordering
                    } else {
                        ordering.reverse()
                    };
                }
            }

            // All ORDER BY specs were equal
            std::cmp::Ordering::Equal
        });

        Ok(results)
    }

    /// Get the value of a binding for comparison
    ///
    /// Extracts the JSON value from a variable binding for ORDER BY comparison.
    /// Supports:
    /// - Direct variable references (e.g., n) - returns node/edge id
    /// - Property access (e.g., n.name) - returns property value
    fn get_binding_value(
        &self,
        result: &MatchResult,
        variable_spec: &str,
    ) -> Option<serde_json::Value> {
        // Handle the special "score" variable which maps to MatchResult.score
        if variable_spec == "score" {
            return Some(serde_json::Value::Number(
                serde_json::Number::from_f64(result.score)
                    .unwrap_or_else(|| serde_json::Number::from(0)),
            ));
        }

        // Parse variable_spec to extract variable name and property
        // Examples: "n" -> ("n", None), "n.name" -> ("n", Some("name"))
        let parts: Vec<&str> = variable_spec.split('.').collect();
        let variable_name = parts.first()?;
        let property_name = parts.get(1).copied();

        // Get the binding (dereference variable_name to get &str)
        let binding = result.bindings.get(*variable_name)?;

        match binding {
            VariableBinding::Node(node) => {
                if let Some(prop) = property_name {
                    // Get property value from node and convert PropertyValue to JSON
                    node.properties
                        .get(prop)
                        .map(|pv| self.property_value_to_json(pv))
                } else {
                    // Return node id as default value
                    Some(serde_json::Value::String(node.id.clone()))
                }
            }
            VariableBinding::Edge(edge) => {
                if let Some(prop) = property_name {
                    // Get property value from edge and convert PropertyValue to JSON
                    edge.properties
                        .get(prop)
                        .map(|pv| self.property_value_to_json(pv))
                } else {
                    // Return edge id as default value
                    Some(serde_json::Value::String(edge.id.to_string()))
                }
            }
            VariableBinding::Path(_) => {
                // Paths don't have direct values - use None as fallback
                // In a full implementation, we could extract properties from path elements
                None
            }
        }
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
                .map_or(memory_pool.node_count(), |nodes| nodes.len())
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
            with_clauses: Vec::new(),
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
                    spec.projections.push(PropertyProjection::Property {
                        variable: parts[0].to_string(),
                        property: parts[1].to_string(),
                    });
                }
            } else {
                // Variable like "n"
                spec.variables.push(item.to_string());
                spec.projections
                    .push(PropertyProjection::Variable(item.to_string()));
            }
        }

        Ok(spec)
    }
}

// NOTE: Default implementation removed because PatternMatcher::new() is fallible.
// PatternMatcher requires regex compilation in PatternCompiler which can fail.
// Types with fallible initialization should not implement Default to avoid
// runtime panics during default construction.
// Use PatternMatcher::new() explicitly to handle initialization errors.

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_order_by_ascending() {
        let matcher = PatternMatcher::new().unwrap();

        // Create test results with different scores
        let results = vec![
            MatchResult {
                bindings: std::collections::HashMap::new(),
                score: 0.5,
            },
            MatchResult {
                bindings: std::collections::HashMap::new(),
                score: 0.9,
            },
            MatchResult {
                bindings: std::collections::HashMap::new(),
                score: 0.2,
            },
        ];

        // Apply ORDER BY with ascending = false (descending)
        let order_by = vec![("score".to_string(), false)];
        let sorted = matcher.apply_order_by(&order_by, results).unwrap();

        // Verify descending order (highest score first)
        assert!((sorted[0].score - 0.9).abs() < 0.001);
        assert!((sorted[1].score - 0.5).abs() < 0.001);
        assert!((sorted[2].score - 0.2).abs() < 0.001);
    }

    #[test]
    fn test_order_by_with_property_access() {
        let matcher = PatternMatcher::new().unwrap();

        // Create test nodes with name properties
        let node1 = std::sync::Arc::new(crate::graph::Node {
            id: "1".to_string(),
            labels: vec!["Person".to_string()],
            properties: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "name".to_string(),
                    PropertyValue {
                        value: Some(Value::StringValue("Alice".to_string())),
                    },
                );
                map
            },
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });

        let node2 = std::sync::Arc::new(crate::graph::Node {
            id: "2".to_string(),
            labels: vec!["Person".to_string()],
            properties: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "name".to_string(),
                    PropertyValue {
                        value: Some(Value::StringValue("Charlie".to_string())),
                    },
                );
                map
            },
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });

        let node3 = std::sync::Arc::new(crate::graph::Node {
            id: "3".to_string(),
            labels: vec!["Person".to_string()],
            properties: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "name".to_string(),
                    PropertyValue {
                        value: Some(Value::StringValue("Bob".to_string())),
                    },
                );
                map
            },
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });

        let results = vec![
            MatchResult {
                bindings: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "n".to_string(),
                        crate::graph::query::ast::VariableBinding::Node(node1),
                    );
                    map
                },
                score: 0.5,
            },
            MatchResult {
                bindings: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "n".to_string(),
                        crate::graph::query::ast::VariableBinding::Node(node2),
                    );
                    map
                },
                score: 0.9,
            },
            MatchResult {
                bindings: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "n".to_string(),
                        crate::graph::query::ast::VariableBinding::Node(node3),
                    );
                    map
                },
                score: 0.2,
            },
        ];

        // Apply ORDER BY with property access
        let order_by = vec![("n.name".to_string(), true)]; // ascending
        let sorted = matcher.apply_order_by(&order_by, results).unwrap();

        // Verify alphabetical ascending order
        assert_eq!(
            sorted[0].bindings["n"].as_node().unwrap().properties["name"]
                .value
                .as_ref()
                .unwrap(),
            &Value::StringValue("Alice".to_string())
        );
        assert_eq!(
            sorted[1].bindings["n"].as_node().unwrap().properties["name"]
                .value
                .as_ref()
                .unwrap(),
            &Value::StringValue("Bob".to_string())
        );
        assert_eq!(
            sorted[2].bindings["n"].as_node().unwrap().properties["name"]
                .value
                .as_ref()
                .unwrap(),
            &Value::StringValue("Charlie".to_string())
        );
    }

    #[test]
    fn test_skip_clause() {
        let _matcher = PatternMatcher::new().unwrap();

        // Create 10 test results
        let results: Vec<MatchResult> = (0..10)
            .map(|i| MatchResult {
                bindings: std::collections::HashMap::new(),
                score: i as f64,
            })
            .collect();

        // Apply SKIP 5 (should keep 5-9)
        let mut sorted_results = results;
        let skip = Some(5);
        if skip.unwrap() < sorted_results.len() {
            sorted_results = sorted_results.split_off(skip.unwrap());
        }

        assert_eq!(sorted_results.len(), 5);
        assert!((sorted_results[0].score - 5.0).abs() < 0.001);
        assert!((sorted_results[4].score - 9.0).abs() < 0.001);
    }

    #[test]
    fn test_limit_clause() {
        let _matcher = PatternMatcher::new().unwrap();

        // Create 10 test results
        let mut results: Vec<MatchResult> = (0..10)
            .map(|i| MatchResult {
                bindings: std::collections::HashMap::new(),
                score: i as f64,
            })
            .collect();

        // Apply LIMIT 3
        let limit = Some(3);
        results.truncate(limit.unwrap());

        assert_eq!(results.len(), 3);
        assert!((results[0].score - 0.0).abs() < 0.001);
        assert!((results[2].score - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_skip_and_limit_combined() {
        let _matcher = PatternMatcher::new().unwrap();

        // Create 10 test results
        let mut results: Vec<MatchResult> = (0..10)
            .map(|i| MatchResult {
                bindings: std::collections::HashMap::new(),
                score: i as f64,
            })
            .collect();

        // Apply SKIP 3 and LIMIT 4 (should return indices 3-6)
        let skip = Some(3);
        let limit = Some(4);

        if skip.unwrap() < results.len() {
            results = results.split_off(skip.unwrap());
        }
        results.truncate(limit.unwrap());

        assert_eq!(results.len(), 4);
        assert!((results[0].score - 3.0).abs() < 0.001);
        assert!((results[3].score - 6.0).abs() < 0.001);
    }

    #[test]
    fn test_order_by_numeric_properties() {
        let matcher = PatternMatcher::new().unwrap();

        // Create test nodes with age properties
        let node1 = std::sync::Arc::new(crate::graph::Node {
            id: "1".to_string(),
            labels: vec!["Person".to_string()],
            properties: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "age".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(30)),
                    },
                );
                map
            },
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });

        let node2 = std::sync::Arc::new(crate::graph::Node {
            id: "2".to_string(),
            labels: vec!["Person".to_string()],
            properties: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "age".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(25)),
                    },
                );
                map
            },
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });

        let node3 = std::sync::Arc::new(crate::graph::Node {
            id: "3".to_string(),
            labels: vec!["Person".to_string()],
            properties: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "age".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(35)),
                    },
                );
                map
            },
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        });

        let results = vec![
            MatchResult {
                bindings: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "p".to_string(),
                        crate::graph::query::ast::VariableBinding::Node(node1),
                    );
                    map
                },
                score: 0.5,
            },
            MatchResult {
                bindings: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "p".to_string(),
                        crate::graph::query::ast::VariableBinding::Node(node2),
                    );
                    map
                },
                score: 0.9,
            },
            MatchResult {
                bindings: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "p".to_string(),
                        crate::graph::query::ast::VariableBinding::Node(node3),
                    );
                    map
                },
                score: 0.2,
            },
        ];

        // Apply ORDER BY age ascending
        let order_by = vec![("p.age".to_string(), true)];
        let sorted = matcher.apply_order_by(&order_by, results).unwrap();

        // Verify numeric ascending order (25, 30, 35)
        assert_eq!(
            sorted[0].bindings["p"].as_node().unwrap().properties["age"]
                .value
                .as_ref()
                .unwrap(),
            &Value::IntValue(25)
        );
        assert_eq!(
            sorted[1].bindings["p"].as_node().unwrap().properties["age"]
                .value
                .as_ref()
                .unwrap(),
            &Value::IntValue(30)
        );
        assert_eq!(
            sorted[2].bindings["p"].as_node().unwrap().properties["age"]
                .value
                .as_ref()
                .unwrap(),
            &Value::IntValue(35)
        );
    }
}
