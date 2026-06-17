// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Multi-model RecordBatch schemas and codecs for Arrow Flight
//!
//! Provides Arrow schemas for all ProximaDB data models:
//! - Documents (JSON document store)
//! - Graph nodes and edges
//! - Metrics (time-series)
//! - Logs (observability)
//! - Traces (distributed tracing)
//! - Relational (dynamic SQL tables)
//!
//! These schemas enable DoPut/DoGet routing across all models,
//! complementing the vector-only codec in `codec.rs`.

use arrow_schema::{DataType, Field, Schema};
use proximadb_catalog::CatalogTableSchema;
use std::sync::Arc;

// --- Document ---

/// Arrow schema for document model records.
///
/// Fields:
/// - `id`: unique document identifier
/// - `document`: JSON-serialized document body
/// - `version`: monotonic version counter
/// - `collection_id`: owning collection
/// - `updated_at_ns`: last-modified timestamp in nanoseconds
pub fn document_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("document", DataType::Utf8, false),
        Field::new("version", DataType::Int64, false),
        Field::new("collection_id", DataType::Utf8, false),
        Field::new("updated_at_ns", DataType::Int64, false),
    ]))
}

// --- Graph Nodes ---

/// Arrow schema for graph node records.
///
/// Fields:
/// - `id`: node identifier
/// - `labels`: JSON array of node labels
/// - `properties`: JSON object of node properties
pub fn node_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("labels", DataType::Utf8, false),
        Field::new("properties", DataType::Utf8, false),
    ]))
}

// --- Graph Edges ---

/// Arrow schema for graph edge records.
///
/// Fields:
/// - `id`: edge identifier
/// - `source_id`: source node id
/// - `target_id`: target node id
/// - `edge_type`: relationship type label
/// - `properties`: JSON object of edge properties
pub fn edge_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("target_id", DataType::Utf8, false),
        Field::new("edge_type", DataType::Utf8, false),
        Field::new("properties", DataType::Utf8, false),
    ]))
}

// --- Metrics ---

/// Arrow schema for time-series metric records.
///
/// Fields:
/// - `name`: metric name (e.g. `cpu_usage`)
/// - `timestamp_ns`: sample timestamp in nanoseconds since epoch
/// - `value`: metric value as f64
/// - `labels`: JSON-encoded label set
pub fn metric_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("name", DataType::Utf8, false),
        Field::new("timestamp_ns", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
        Field::new("labels", DataType::Utf8, false),
    ]))
}

// --- Logs ---

/// Arrow schema for log records.
///
/// Fields:
/// - `timestamp_ns`: log timestamp in nanoseconds since epoch
/// - `severity`: severity level as int32 (0=TRACE .. 5=FATAL)
/// - `message`: log message body
/// - `source`: optional originating source
/// - `service`: optional service name
pub fn log_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ns", DataType::Int64, false),
        Field::new("severity", DataType::Int32, false),
        Field::new("message", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, true),
        Field::new("service", DataType::Utf8, true),
    ]))
}

// --- Traces ---

/// Arrow schema for distributed trace span records.
///
/// Fields:
/// - `trace_id`: trace-wide unique identifier
/// - `span_id`: span-level unique identifier
/// - `parent_span_id`: parent span (nullable for root spans)
/// - `name`: operation name
/// - `start_time_ns`: span start in nanoseconds since epoch
/// - `end_time_ns`: span end in nanoseconds since epoch
pub fn trace_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("span_id", DataType::Utf8, false),
        Field::new("parent_span_id", DataType::Utf8, true),
        Field::new("name", DataType::Utf8, false),
        Field::new("start_time_ns", DataType::Int64, false),
        Field::new("end_time_ns", DataType::Int64, false),
    ]))
}

// --- Relational ---

/// Build a dynamic Arrow schema for relational (SQL) tables.
///
/// Column types are mapped from SQL-style names:
/// - `INT32`, `INTEGER`, `INT` -> `DataType::Int32`
/// - `INT64`, `BIGINT` -> `DataType::Int64`
/// - `FLOAT32`, `FLOAT`, `REAL` -> `DataType::Float32`
/// - `FLOAT64`, `DOUBLE` -> `DataType::Float64`
/// - `BOOLEAN`, `BOOL` -> `DataType::Boolean`
/// - `TIMESTAMP` -> `DataType::Int64`
/// - Everything else (STRING, VARCHAR, TEXT, ...) -> `DataType::Utf8`
pub fn relational_schema(column_names: &[String], column_types: &[String]) -> Arc<Schema> {
    let fields: Vec<Field> = column_names
        .iter()
        .zip(column_types.iter())
        .map(|(name, type_name)| {
            let dt = match type_name.to_uppercase().as_str() {
                "INT32" | "INTEGER" | "INT" => DataType::Int32,
                "INT64" | "BIGINT" => DataType::Int64,
                "FLOAT32" | "FLOAT" | "REAL" => DataType::Float32,
                "FLOAT64" | "DOUBLE" => DataType::Float64,
                "BOOLEAN" | "BOOL" => DataType::Boolean,
                "TIMESTAMP" => DataType::Int64,
                _ => DataType::Utf8, // STRING, VARCHAR, TEXT, etc.
            };
            Field::new(name, dt, true)
        })
        .collect();
    Arc::new(Schema::new(fields))
}

/// Build an Arrow schema directly from xCatalog table metadata.
///
/// This is the canonical relational Arrow path: SQL, REST/gRPC, embedded, and
/// Arrow Flight should agree on the xCatalog `ProximaType` and nullability
/// instead of maintaining separate string-based type maps.
pub fn relational_schema_from_catalog(table_schema: &CatalogTableSchema) -> Arc<Schema> {
    let fields: Vec<Field> = table_schema
        .columns
        .iter()
        .map(|column| {
            Field::new(
                column.name.clone(),
                proximadb_catalog::catalog_arrow_type(&column.data_type),
                column.nullable,
            )
        })
        .collect();
    Arc::new(Schema::new(fields))
}

/// Detect model type from a FlightDescriptor path.
///
/// Convention: `path[0]` is the model type, `path[1]` is the collection/table name.
/// Returns `"vector"` as the default when the path is empty or unrecognized.
pub fn detect_model_from_descriptor(path: &[String]) -> &str {
    path.first().map(|s| s.as_str()).unwrap_or("vector")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_schema_fields() {
        let schema = document_schema();
        assert_eq!(schema.fields().len(), 5);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(*schema.field(0).data_type(), DataType::Utf8);
        assert_eq!(schema.field(1).name(), "document");
        assert_eq!(*schema.field(1).data_type(), DataType::Utf8);
        assert_eq!(schema.field(2).name(), "version");
        assert_eq!(*schema.field(2).data_type(), DataType::Int64);
        assert_eq!(schema.field(3).name(), "collection_id");
        assert_eq!(*schema.field(3).data_type(), DataType::Utf8);
        assert_eq!(schema.field(4).name(), "updated_at_ns");
        assert_eq!(*schema.field(4).data_type(), DataType::Int64);
    }

    #[test]
    fn test_node_schema_fields() {
        let schema = node_schema();
        assert_eq!(schema.fields().len(), 3);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(*schema.field(0).data_type(), DataType::Utf8);
        assert_eq!(schema.field(1).name(), "labels");
        assert_eq!(*schema.field(1).data_type(), DataType::Utf8);
        assert_eq!(schema.field(2).name(), "properties");
        assert_eq!(*schema.field(2).data_type(), DataType::Utf8);
    }

    #[test]
    fn test_edge_schema_fields() {
        let schema = edge_schema();
        assert_eq!(schema.fields().len(), 5);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(schema.field(1).name(), "source_id");
        assert_eq!(schema.field(2).name(), "target_id");
        assert_eq!(schema.field(3).name(), "edge_type");
        assert_eq!(schema.field(4).name(), "properties");
    }

    #[test]
    fn test_metric_schema_fields() {
        let schema = metric_schema();
        assert_eq!(schema.fields().len(), 4);
        assert_eq!(schema.field(0).name(), "name");
        assert_eq!(*schema.field(0).data_type(), DataType::Utf8);
        assert_eq!(schema.field(1).name(), "timestamp_ns");
        assert_eq!(*schema.field(1).data_type(), DataType::Int64);
        assert_eq!(schema.field(2).name(), "value");
        assert_eq!(*schema.field(2).data_type(), DataType::Float64);
        assert_eq!(schema.field(3).name(), "labels");
        assert_eq!(*schema.field(3).data_type(), DataType::Utf8);
    }

    #[test]
    fn test_log_schema_fields() {
        let schema = log_schema();
        assert_eq!(schema.fields().len(), 5);
        assert_eq!(schema.field(0).name(), "timestamp_ns");
        assert_eq!(schema.field(1).name(), "severity");
        assert_eq!(*schema.field(1).data_type(), DataType::Int32);
        assert_eq!(schema.field(2).name(), "message");
        // source and service are nullable
        assert!(schema.field(3).is_nullable());
        assert!(schema.field(4).is_nullable());
    }

    #[test]
    fn test_trace_schema_fields() {
        let schema = trace_schema();
        assert_eq!(schema.fields().len(), 6);
        assert_eq!(schema.field(0).name(), "trace_id");
        assert_eq!(schema.field(1).name(), "span_id");
        assert_eq!(schema.field(2).name(), "parent_span_id");
        assert!(schema.field(2).is_nullable());
        assert_eq!(schema.field(3).name(), "name");
        assert_eq!(schema.field(4).name(), "start_time_ns");
        assert_eq!(schema.field(5).name(), "end_time_ns");
    }

    #[test]
    fn test_relational_schema_type_mapping() {
        let names = vec![
            "id".to_string(),
            "count".to_string(),
            "score".to_string(),
            "active".to_string(),
            "name".to_string(),
            "created".to_string(),
        ];
        let types = vec![
            "INT32".to_string(),
            "BIGINT".to_string(),
            "DOUBLE".to_string(),
            "BOOL".to_string(),
            "VARCHAR".to_string(),
            "TIMESTAMP".to_string(),
        ];

        let schema = relational_schema(&names, &types);
        assert_eq!(schema.fields().len(), 6);
        assert_eq!(*schema.field(0).data_type(), DataType::Int32);
        assert_eq!(*schema.field(1).data_type(), DataType::Int64);
        assert_eq!(*schema.field(2).data_type(), DataType::Float64);
        assert_eq!(*schema.field(3).data_type(), DataType::Boolean);
        assert_eq!(*schema.field(4).data_type(), DataType::Utf8);
        assert_eq!(*schema.field(5).data_type(), DataType::Int64);
        // All relational columns are nullable
        for field in schema.fields() {
            assert!(field.is_nullable());
        }
    }

    #[test]
    fn test_relational_schema_from_catalog_uses_catalog_types_and_nullability() {
        use proximadb_catalog::{CatalogColumn, CatalogTableSchema};
        use proximadb_data_model::ProximaType;

        let table = CatalogTableSchema::new("events")
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "payload", ProximaType::Json))
            .with_column(CatalogColumn::new(
                3,
                "embedding",
                ProximaType::DenseVector {
                    element: proximadb_data_model::VectorElement::Float32,
                    dim: 0,
                },
            ))
            .with_column(CatalogColumn::new(
                4,
                "created_at",
                ProximaType::TimestampTz(proximadb_data_model::TimeUnit::Nanosecond),
            ));

        let schema = relational_schema_from_catalog(&table);
        assert_eq!(schema.fields().len(), 4);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(*schema.field(0).data_type(), DataType::Int64);
        assert!(!schema.field(0).is_nullable());
        assert_eq!(schema.field(1).name(), "payload");
        assert_eq!(*schema.field(1).data_type(), DataType::Utf8);
        assert_eq!(
            *schema.field(2).data_type(),
            DataType::List(Box::new(Field::new("item", DataType::Float32, true)).into())
        );
        assert_eq!(
            *schema.field(3).data_type(),
            DataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, Some("UTC".into()))
        );
    }

    #[test]
    fn test_detect_model_vector() {
        let path = vec!["vector".to_string(), "col".to_string()];
        assert_eq!(detect_model_from_descriptor(&path), "vector");
    }

    #[test]
    fn test_detect_model_document() {
        let path = vec!["document".to_string(), "col".to_string()];
        assert_eq!(detect_model_from_descriptor(&path), "document");
    }

    #[test]
    fn test_detect_model_graph() {
        let path = vec!["graph".to_string(), "social".to_string()];
        assert_eq!(detect_model_from_descriptor(&path), "graph");
    }

    #[test]
    fn test_detect_model_default() {
        let path: Vec<String> = vec![];
        assert_eq!(detect_model_from_descriptor(&path), "vector");
    }
}
