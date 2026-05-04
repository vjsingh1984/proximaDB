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

//! # Arrow Bridge for Graph Query Results
//!
//! This module provides conversion between graph query results (HashMap-based)
//! and Arrow columnar format (RecordBatch). This enables:
//!
//! - Vectorized processing of graph results in federated queries
//! - Efficient cross-model joins (graph + vector + document)
//! - Zero-copy data sharing between query components
//! - Arrow Flight streaming for large result sets
//!
//! # Architecture
//!
//! Graph results typically come from `GraphQueryExecutor` as:
//! ```text
//! Vec<HashMap<String, serde_json::Value>>
//! ```
//!
//! This bridge converts them to Arrow RecordBatch:
//! ```text
//! RecordBatch {
//!   schema: [node_id, label, properties, edge_id, edge_type, ...],
//!   columns: [StringArray, StringArray, ListArray, ...]
//! }
//! ```
//!
//! # Performance Benefits
//!
//! - **Columnar Layout**: Efficient filtering and projection
//! - **Zero-Copy**: Arrow data can be shared across query components
//! - **SIMD**: Vectorized operations on numeric properties
//! - **Compression**: Better memory footprint for large result sets

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::*;
use arrow::datatypes::*;
use arrow::record_batch::RecordBatch;
use serde_json::Value as JsonValue;

use crate::core::error::VectorDBError;

/// Graph query result converted to Arrow format
#[derive(Debug, Clone)]
pub struct GraphArrowResult {
    /// Arrow RecordBatch containing graph results
    pub batch: RecordBatch,
    /// Schema for graph results
    pub schema: GraphSchema,
}

/// Schema definition for graph query results
#[derive(Debug, Clone)]
pub struct GraphSchema {
    /// Node columns
    pub node_columns: Vec<GraphColumn>,
    /// Edge columns (if traversal results)
    pub edge_columns: Vec<GraphColumn>,
    /// Metadata columns (depth, path, score)
    pub metadata_columns: Vec<GraphColumn>,
}

/// Column definition for graph results
#[derive(Debug, Clone)]
pub struct GraphColumn {
    /// Column name
    pub name: String,
    /// Data type
    pub data_type: DataType,
    /// Is nullable
    pub nullable: bool,
}

/// Bridge for converting graph query results to/from Arrow format
pub struct GraphArrowBridge;

impl GraphArrowBridge {
    /// Convert graph query results to Arrow RecordBatch
    ///
    /// # Arguments
    ///
    /// * `results` - Graph query results from executor (HashMap format)
    /// * `include_edges` - Whether to include edge information (for traversals)
    ///
    /// # Returns
    ///
    /// Arrow RecordBatch with schema:
    /// - `node_id`: String (UTF8)
    /// - `label`: String (UTF8)
    /// - `properties`: Struct<V> (dynamic properties)
    /// - `edge_id`: String (optional, if include_edges)
    /// - `edge_type`: String (optional, if include_edges)
    /// - `depth`: Int32 (optional, for traversals)
    /// - `score`: Float64 (optional, for relevance scoring)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let results = vec![
    ///     HashMap::from([
    ///         ("node_id".to_string(), json!("node1")),
    ///         ("label".to_string(), json!("Person")),
    ///         ("properties".to_string(), json!({"name": "Alice", "age": 30}))
    ///     ])
    /// ];
    ///
    /// let batch = GraphArrowBridge::graph_results_to_arrow(&results, false)?;
    /// ```
    pub fn graph_results_to_arrow(
        results: &[HashMap<String, JsonValue>],
        include_edges: bool,
    ) -> Result<RecordBatch, VectorDBError> {
        if results.is_empty() {
            // Return empty batch with minimal schema
            return Self::empty_batch(include_edges);
        }

        // Determine schema from first result
        let schema = Self::infer_schema(&results[0], include_edges)?;

        // Build arrays column by column
        let mut arrays: Vec<(String, ArrayRef)> = Vec::new();

        // Extract node_id column
        if let Some(node_ids) = Self::extract_string_array(results, "node_id") {
            arrays.push(("node_id".to_string(), Arc::new(node_ids)));
        }

        // Extract label column
        if let Some(labels) = Self::extract_string_array(results, "label") {
            arrays.push(("label".to_string(), Arc::new(labels)));
        }

        // Extract properties as StructArray
        if let Some(properties) = Self::extract_properties_array(results) {
            arrays.push(("properties".to_string(), Arc::new(properties)));
        }

        // Extract edge columns if requested
        if include_edges {
            if let Some(edge_ids) = Self::extract_string_array(results, "edge_id") {
                arrays.push(("edge_id".to_string(), Arc::new(edge_ids)));
            }

            if let Some(edge_types) = Self::extract_string_array(results, "edge_type") {
                arrays.push(("edge_type".to_string(), Arc::new(edge_types)));
            }

            if let Some(depths) = Self::extract_i32_array(results, "depth") {
                arrays.push(("depth".to_string(), Arc::new(depths)));
            }
        }

        // Extract optional score column
        if let Some(scores) = Self::extract_f64_array(results, "score") {
            arrays.push(("score".to_string(), Arc::new(scores)));
        }

        // Build RecordBatch
        let schema = Schema::new(
            arrays
                .iter()
                .map(|(name, array)| {
                    Field::new(name, array.data_type().clone(), array.is_nullable())
                })
                .collect::<Vec<_>>(),
        );

        RecordBatch::try_new(
            Arc::new(schema),
            arrays.into_iter().map(|(_, arr)| arr).collect(),
        )
        .map_err(|e| VectorDBError::Internal(format!("Failed to create RecordBatch: {}", e)))
    }

    /// Infer Arrow schema from a single graph result
    fn infer_schema(
        result: &HashMap<String, JsonValue>,
        include_edges: bool,
    ) -> Result<Schema, VectorDBError> {
        let mut fields = vec![
            Field::new("node_id", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, true),
        ];

        // Properties are stored as a struct with dynamic fields
        if let Some(JsonValue::Object(props)) = result.get("properties") {
            let prop_fields: Vec<Field> = props
                .iter()
                .map(|(key, value)| Field::new(key, Self::json_type_to_arrow(value), true))
                .collect();

            if !prop_fields.is_empty() {
                fields.push(Field::new(
                    "properties",
                    DataType::Struct(prop_fields.clone().into()),
                    true,
                ));
            }
        }

        if include_edges {
            fields.push(Field::new("edge_id", DataType::Utf8, true));
            fields.push(Field::new("edge_type", DataType::Utf8, true));
            fields.push(Field::new("depth", DataType::Int32, true));
        }

        if result.contains_key("score") {
            fields.push(Field::new("score", DataType::Float64, true));
        }

        Ok(Schema::new(fields))
    }

    /// Convert JSON type to Arrow DataType
    fn json_type_to_arrow(value: &JsonValue) -> DataType {
        match value {
            JsonValue::String(_) => DataType::Utf8,
            JsonValue::Number(n) => {
                if n.is_i64() {
                    DataType::Int64
                } else {
                    DataType::Float64
                }
            }
            JsonValue::Bool(_) => DataType::Boolean,
            JsonValue::Null => DataType::Null,
            JsonValue::Array(_) => {
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)))
            }
            JsonValue::Object(_) => DataType::Struct(Vec::<Field>::new().into()), // Will be inferred later
        }
    }

    /// Extract a StringArray from results for a given key
    fn extract_string_array(
        results: &[HashMap<String, JsonValue>],
        key: &str,
    ) -> Option<StringArray> {
        let values: Vec<Option<&str>> = results
            .iter()
            .map(|row| row.get(key).and_then(|v| v.as_str()))
            .collect();

        if values.iter().all(|v| v.is_none()) {
            return None; // All nulls, skip column
        }

        Some(StringArray::from(values))
    }

    /// Extract an Int32Array from results for a given key
    fn extract_i32_array(results: &[HashMap<String, JsonValue>], key: &str) -> Option<Int32Array> {
        let values: Vec<Option<i32>> = results
            .iter()
            .map(|row| {
                row.get(key)
                    .and_then(|v| v.as_i64())
                    .and_then(|i| i32::try_from(i).ok())
            })
            .collect();

        if values.iter().all(|v| v.is_none()) {
            return None;
        }

        Some(Int32Array::from(values))
    }

    /// Extract a Float64Array from results for a given key
    fn extract_f64_array(
        results: &[HashMap<String, JsonValue>],
        key: &str,
    ) -> Option<Float64Array> {
        let values: Vec<Option<f64>> = results
            .iter()
            .map(|row| row.get(key).and_then(|v| v.as_f64()))
            .collect();

        if values.iter().all(|v| v.is_none()) {
            return None;
        }

        Some(Float64Array::from(values))
    }

    /// Extract properties as a StructArray
    fn extract_properties_array(results: &[HashMap<String, JsonValue>]) -> Option<StructArray> {
        // For now, store properties as JSON string
        // TODO: Parse into proper struct with inferred schema
        let values: Vec<Option<String>> = results
            .iter()
            .map(|row| {
                row.get("properties")
                    .and_then(|v| serde_json::to_string(v).ok())
            })
            .collect();

        if values.iter().all(|v| v.is_none()) {
            return None;
        }

        // Store as JSON string for simplicity
        let string_array = StringArray::from(
            values
                .iter()
                .map(|value| value.as_deref())
                .collect::<Vec<_>>(),
        );
        Some(StructArray::new(
            vec![Field::new("json", DataType::Utf8, true)]
                .into_iter()
                .collect(),
            vec![Arc::new(string_array) as ArrayRef],
            None,
        ))
    }

    /// Create an empty RecordBatch with appropriate schema
    fn empty_batch(include_edges: bool) -> Result<RecordBatch, VectorDBError> {
        let mut fields = vec![
            Field::new("node_id", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, true),
        ];

        if include_edges {
            fields.push(Field::new("edge_id", DataType::Utf8, true));
            fields.push(Field::new("edge_type", DataType::Utf8, true));
            fields.push(Field::new("depth", DataType::Int32, true));
        }

        let schema = Schema::new(fields);
        let arrays: Vec<ArrayRef> = schema
            .fields()
            .iter()
            .map(|field| new_null_array(field.data_type(), 0))
            .collect();

        RecordBatch::try_new(Arc::new(schema), arrays).map_err(|e| {
            VectorDBError::Internal(format!("Failed to create empty RecordBatch: {}", e))
        })
    }

    /// Enable Arrow-native streaming for large graph results
    ///
    /// This is useful for:
    /// - Arrow Flight API responses
    /// - Large graph traversals that don't fit in memory
    /// - Real-time streaming of query results
    ///
    /// # Arguments
    ///
    /// * `executor` - Graph query executor
    /// * `query` - Graph query to execute
    /// * `batch_size` - Number of rows per batch
    ///
    /// # Returns
    ///
    /// Stream of RecordBatch results
    ///
    /// # Example
    ///
    /// ```ignore
    /// use futures::stream::StreamExt;
    ///
    /// let mut stream = GraphArrowBridge::stream_graph_results(
    ///     &executor,
    ///     &query,
    ///     1000,
    /// ).await?;
    ///
    /// while let Some(batch) = stream.next().await {
    ///     // Process batch
    /// }
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub async fn stream_graph_results<'a>(
        executor: &'a crate::graph::query::executor::QueryExecutor,
        query: &'a crate::graph::query::planner::QueryPlan,
        batch_size: usize,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<RecordBatch, VectorDBError>> + Send + 'a>>,
        VectorDBError,
    > {
        use futures::stream::{self, StreamExt};

        // Execute query to get all results
        let context = crate::graph::query::QueryContext::default();
        let results = executor.execute(query, &context).await?;

        // Convert to stream of batches
        let stream = stream::iter(results.into_iter())
            .chunks(batch_size)
            .map(move |batch| {
                GraphArrowBridge::graph_results_to_arrow(&batch, true)
                    .map_err(|e| VectorDBError::Internal(format!("Batch conversion failed: {}", e)))
            });

        Ok(Box::pin(stream))
    }
}

// Re-export Stream trait
use futures::Stream;
use std::pin::Pin;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_simple_node_to_arrow() {
        let results = vec![HashMap::from([
            ("node_id".to_string(), json!("node1")),
            ("label".to_string(), json!("Person")),
            ("properties".to_string(), json!({"name": "Alice"})),
        ])];

        let batch = GraphArrowBridge::graph_results_to_arrow(&results, false).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 3); // node_id, label, properties
    }

    #[test]
    fn test_convert_traversal_result_to_arrow() {
        let results = vec![HashMap::from([
            ("node_id".to_string(), json!("node1")),
            ("label".to_string(), json!("Person")),
            ("edge_id".to_string(), json!("edge1")),
            ("edge_type".to_string(), json!("KNOWS")),
            ("depth".to_string(), json!(1)),
        ])];

        let batch = GraphArrowBridge::graph_results_to_arrow(&results, true).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 5); // node_id, label, edge_id, edge_type, depth
    }

    #[test]
    fn test_empty_results() {
        let results: Vec<HashMap<String, JsonValue>> = vec![];
        let batch = GraphArrowBridge::graph_results_to_arrow(&results, false).unwrap();
        assert_eq!(batch.num_rows(), 0);
    }

    #[test]
    fn test_extract_string_array() {
        let results = vec![
            HashMap::from([("node_id".to_string(), json!("node1"))]),
            HashMap::from([("node_id".to_string(), json!("node2"))]),
        ];

        let array = GraphArrowBridge::extract_string_array(&results, "node_id").unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array.value(0), "node1");
        assert_eq!(array.value(1), "node2");
    }

    #[test]
    fn test_extract_i32_array() {
        let results = vec![
            HashMap::from([("depth".to_string(), json!(1))]),
            HashMap::from([("depth".to_string(), json!(2))]),
        ];

        let array = GraphArrowBridge::extract_i32_array(&results, "depth").unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array.value(0), 1);
        assert_eq!(array.value(1), 2);
    }

    #[test]
    fn test_extract_f64_array() {
        let results = vec![
            HashMap::from([("score".to_string(), json!(0.95))]),
            HashMap::from([("score".to_string(), json!(0.87))]),
        ];

        let array = GraphArrowBridge::extract_f64_array(&results, "score").unwrap();
        assert_eq!(array.len(), 2);
        assert_eq!(array.value(0), 0.95);
        assert_eq!(array.value(1), 0.87);
    }

    #[test]
    fn test_json_type_to_arrow() {
        assert_eq!(
            GraphArrowBridge::json_type_to_arrow(&json!("string")),
            DataType::Utf8
        );
        assert_eq!(
            GraphArrowBridge::json_type_to_arrow(&json!(42)),
            DataType::Int64
        );
        assert_eq!(
            GraphArrowBridge::json_type_to_arrow(&json!(3.14)),
            DataType::Float64
        );
        assert_eq!(
            GraphArrowBridge::json_type_to_arrow(&json!(true)),
            DataType::Boolean
        );
    }
}
