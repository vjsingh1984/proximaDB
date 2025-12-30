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

//! # Schema Module TDD Tests (WS5: ProximaSchema Migration + VectorRecord Compatibility)
//!
//! Comprehensive tests for:
//! - ProximaSchema creation from Arrow Schema
//! - Schema evolution (adding/removing columns)
//! - VectorRecord <-> RecordBatch roundtrip
//! - Nested metadata handling
//! - Avro-style schema serialization

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray, Float32Array, Int64Array};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};

use proximadb::proto::proximadb_v1::{SqlValue, VectorRecord, SqlArray, SqlObject};
use proximadb::proto::proximadb_v1::sql_value::Value as ProtoSqlValueInner;
use proximadb::storage::schema::{
    ProximaSchema, ProximaColumn, ProximaDataType, VectorElementType, TimeUnit,
    DefaultValue, AutoGenerateType,
    VectorRecordBridge, DefaultVectorRecordBridge, MetadataMode,
    infer_schema_from_vector_records,
    AvroStyleSchema, AvroStyleField, AvroStyleType,
    SchemaEvolution, SchemaEvolutionOp, DefaultSchemaEvolution,
    SchemaRegistry, InMemorySchemaRegistry,
};

// ============================================================================
// Test Fixtures
// ============================================================================

/// Create a test VectorRecord with various metadata types.
fn create_test_record(id: &str, dimension: usize) -> VectorRecord {
    let mut metadata = HashMap::new();

    // String metadata
    metadata.insert(
        "category".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::StringValue("electronics".to_string())),
        },
    );

    // Numeric metadata
    metadata.insert(
        "price".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::NumberValue(99.99)),
        },
    );

    // Integer metadata
    metadata.insert(
        "quantity".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::Int64Value(42)),
        },
    );

    // Boolean metadata
    metadata.insert(
        "in_stock".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::BoolValue(true)),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: (0..dimension).map(|i| (i as f32) * 0.01).collect(),
        metadata,
        timestamp: Some(chrono::Utc::now().timestamp_millis()),
        version: Some(1),
        ..Default::default()
    }
}

/// Create a VectorRecord with nested metadata (arrays and objects).
fn create_record_with_nested_metadata(id: &str, dimension: usize) -> VectorRecord {
    let mut metadata = HashMap::new();

    // Array metadata
    metadata.insert(
        "tags".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::ArrayValue(SqlArray {
                values: vec![
                    SqlValue {
                        value: Some(ProtoSqlValueInner::StringValue("tag1".to_string())),
                    },
                    SqlValue {
                        value: Some(ProtoSqlValueInner::StringValue("tag2".to_string())),
                    },
                ],
            })),
        },
    );

    // Object metadata
    let mut nested_fields = HashMap::new();
    nested_fields.insert(
        "name".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::StringValue("nested_value".to_string())),
        },
    );
    nested_fields.insert(
        "count".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::Int64Value(10)),
        },
    );

    metadata.insert(
        "nested_obj".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::ObjectValue(SqlObject {
                fields: nested_fields,
            })),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: (0..dimension).map(|i| (i as f32) * 0.1).collect(),
        metadata,
        timestamp: Some(1234567890000),
        version: Some(2),
        ..Default::default()
    }
}

// ============================================================================
// ProximaSchema Creation Tests
// ============================================================================

#[test]
fn test_proxima_schema_from_arrow_schema() {
    let arrow_schema = ArrowSchema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("embedding", DataType::FixedSizeBinary(512), false),
        Field::new("title", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
    ]);

    let proxima_schema = ProximaSchema::from_arrow_schema(&arrow_schema, "test_schema".to_string());

    assert_eq!(proxima_schema.schema_id, "test_schema");
    assert_eq!(proxima_schema.active_column_count(), 4);

    // Check column mapping
    let id_col = proxima_schema.column_by_name("id").unwrap();
    assert!(!id_col.nullable);
    assert!(matches!(id_col.data_type, ProximaDataType::String));

    let embedding_col = proxima_schema.column_by_name("embedding").unwrap();
    assert!(!embedding_col.nullable);
    // FixedSizeBinary(512) should be interpreted as Vector(128) since 512 bytes = 128 floats
    if let ProximaDataType::Vector { dimension, .. } = &embedding_col.data_type {
        assert_eq!(*dimension, 128);
    } else {
        panic!("Expected Vector type for embedding column");
    }
}

#[test]
fn test_proxima_schema_to_arrow_roundtrip() {
    let schema = ProximaSchema::vector_record_schema(768);
    let arrow_schema = schema.to_arrow_schema();

    // Verify fields are present
    assert!(arrow_schema.field_with_name("id").is_ok());
    assert!(arrow_schema.field_with_name("vector").is_ok());
    assert!(arrow_schema.field_with_name("metadata").is_ok());
    assert!(arrow_schema.field_with_name("timestamp").is_ok());
    assert!(arrow_schema.field_with_name("version").is_ok());

    // Roundtrip test
    let recovered = ProximaSchema::from_arrow_schema(&arrow_schema, "recovered".to_string());
    assert_eq!(recovered.active_column_count(), schema.active_column_count());
}

#[test]
fn test_proxima_schema_with_metadata_columns() {
    let metadata_fields = vec![
        ("category".to_string(), ProximaDataType::String),
        ("price".to_string(), ProximaDataType::Float64),
        ("quantity".to_string(), ProximaDataType::Int64),
        ("tags".to_string(), ProximaDataType::Json),
    ];

    let schema = ProximaSchema::with_metadata_columns(
        "custom_schema".to_string(),
        512,
        metadata_fields,
    );

    assert_eq!(schema.schema_id, "custom_schema");
    assert_eq!(schema.vector_dimension(), Some(512));

    // Check metadata columns are present
    assert!(schema.column_by_name("category").is_some());
    assert!(schema.column_by_name("price").is_some());
    assert!(schema.column_by_name("quantity").is_some());
    assert!(schema.column_by_name("tags").is_some());
}

// ============================================================================
// Schema Evolution Tests
// ============================================================================

#[test]
fn test_schema_add_column() {
    let schema = ProximaSchema::vector_record_schema(256);
    let evolution = DefaultSchemaEvolution::new();

    let new_column = ProximaColumn {
        id: schema.next_column_id(),
        name: "new_field".to_string(),
        data_type: ProximaDataType::String,
        nullable: true,
        default_value: Some(DefaultValue::Literal("\"default\"".to_string())),
        comment: Some("A new field".to_string()),
        metadata: HashMap::new(),
        is_deleted: false,
        original_id: None,
    };

    let evolved = evolution.apply_operation(
        schema.clone(),
        SchemaEvolutionOp::AddColumn { column: new_column },
    ).unwrap();

    assert_eq!(evolved.active_column_count(), schema.active_column_count() + 1);
    assert!(evolved.column_by_name("new_field").is_some());
    assert!(evolved.version > schema.version);
}

#[test]
fn test_schema_drop_column() {
    let metadata_fields = vec![
        ("field_to_drop".to_string(), ProximaDataType::String),
        ("field_to_keep".to_string(), ProximaDataType::Int64),
    ];

    let schema = ProximaSchema::with_metadata_columns(
        "drop_test".to_string(),
        128,
        metadata_fields,
    );

    let evolution = DefaultSchemaEvolution::new();
    let column_id = schema.column_by_name("field_to_drop").unwrap().id;

    let evolved = evolution.apply_operation(
        schema.clone(),
        SchemaEvolutionOp::DropColumn { column_id },
    ).unwrap();

    // Column should be soft-deleted (is_deleted = true)
    assert!(evolved.column_by_name("field_to_drop").is_none());
    assert!(evolved.column_by_name("field_to_keep").is_some());
}

#[test]
fn test_schema_rename_column() {
    let metadata_fields = vec![
        ("old_name".to_string(), ProximaDataType::String),
    ];

    let schema = ProximaSchema::with_metadata_columns(
        "rename_test".to_string(),
        64,
        metadata_fields,
    );

    let evolution = DefaultSchemaEvolution::new();
    let column_id = schema.column_by_name("old_name").unwrap().id;

    let evolved = evolution.apply_operation(
        schema.clone(),
        SchemaEvolutionOp::RenameColumn {
            column_id,
            new_name: "new_name".to_string(),
        },
    ).unwrap();

    assert!(evolved.column_by_name("old_name").is_none());
    assert!(evolved.column_by_name("new_name").is_some());

    // Verify original_id is tracked
    let renamed_col = evolved.column_by_name("new_name").unwrap();
    assert_eq!(renamed_col.original_id, Some(column_id));
}

// ============================================================================
// VectorRecord <-> RecordBatch Roundtrip Tests
// ============================================================================

#[test]
fn test_vector_record_to_batch_roundtrip() {
    let schema = ProximaSchema::vector_record_schema(128);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![
        create_test_record("vec_001", 128),
        create_test_record("vec_002", 128),
        create_test_record("vec_003", 128),
    ];

    // Convert to batch
    let batch = bridge.records_to_batch(&records).unwrap();
    assert_eq!(batch.num_rows(), 3);

    // Convert back to records
    let recovered = bridge.batch_to_records(&batch).unwrap();
    assert_eq!(recovered.len(), 3);

    // Verify IDs preserved
    assert_eq!(recovered[0].id, "vec_001");
    assert_eq!(recovered[1].id, "vec_002");
    assert_eq!(recovered[2].id, "vec_003");

    // Verify vector dimensions preserved
    assert_eq!(recovered[0].vector.len(), 128);
    assert_eq!(recovered[1].vector.len(), 128);
    assert_eq!(recovered[2].vector.len(), 128);

    // Verify metadata preserved
    assert!(recovered[0].metadata.contains_key("category"));
    assert!(recovered[0].metadata.contains_key("price"));
}

#[test]
fn test_vector_record_batch_with_json_metadata() {
    let schema = ProximaSchema::vector_record_schema(64);
    let bridge = DefaultVectorRecordBridge::new(schema)
        .with_metadata_mode(MetadataMode::JsonString);

    let records = vec![create_test_record("json_test", 64)];
    let batch = bridge.records_to_batch(&records).unwrap();

    // Verify metadata is stored as JSON string
    let metadata_col = batch.column_by_name("metadata").unwrap();
    let string_arr = metadata_col.as_any().downcast_ref::<StringArray>().unwrap();
    let json_str = string_arr.value(0);

    // Should be valid JSON
    let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
    assert!(parsed.is_object());
    assert!(parsed.get("category").is_some());
}

#[test]
fn test_vector_record_batch_with_struct_metadata() {
    let schema = ProximaSchema::vector_record_schema(64);
    let bridge = DefaultVectorRecordBridge::new(schema)
        .with_metadata_mode(MetadataMode::ArrowStruct);

    let records = vec![create_test_record("struct_test", 64)];
    let batch = bridge.records_to_batch(&records).unwrap();

    // Verify metadata is stored as struct
    let metadata_col = batch.column_by_name("metadata").unwrap();
    assert!(metadata_col.as_any().downcast_ref::<arrow_array::StructArray>().is_some());
}

#[test]
fn test_vector_record_empty_metadata() {
    let schema = ProximaSchema::vector_record_schema(32);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![VectorRecord {
        id: "empty_meta".to_string(),
        vector: vec![0.5; 32],
        metadata: HashMap::new(),
        timestamp: Some(1000000),
        version: Some(1),
        ..Default::default()
    }];

    let batch = bridge.records_to_batch(&records).unwrap();
    let recovered = bridge.batch_to_records(&batch).unwrap();

    assert_eq!(recovered[0].id, "empty_meta");
    assert!(recovered[0].metadata.is_empty());
}

// ============================================================================
// Nested Metadata Handling Tests
// ============================================================================

#[test]
fn test_nested_metadata_array() {
    let schema = ProximaSchema::vector_record_schema(16);
    let bridge = DefaultVectorRecordBridge::new(schema)
        .with_metadata_mode(MetadataMode::JsonString);

    let records = vec![create_record_with_nested_metadata("nested", 16)];
    let batch = bridge.records_to_batch(&records).unwrap();
    let recovered = bridge.batch_to_records(&batch).unwrap();

    // Verify nested array is preserved
    assert!(recovered[0].metadata.contains_key("tags"));
    let tags = &recovered[0].metadata["tags"];

    if let Some(ProtoSqlValueInner::ArrayValue(arr)) = &tags.value {
        assert_eq!(arr.values.len(), 2);
    } else if let Some(ProtoSqlValueInner::StringValue(s)) = &tags.value {
        // JSON mode serializes arrays as strings
        let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
        assert!(parsed.is_array());
    } else {
        // In JSON mode, it may be parsed back differently
        // Just check it's not empty
        assert!(tags.value.is_some());
    }
}

#[test]
fn test_nested_metadata_object() {
    let schema = ProximaSchema::vector_record_schema(16);
    let bridge = DefaultVectorRecordBridge::new(schema)
        .with_metadata_mode(MetadataMode::JsonString);

    let records = vec![create_record_with_nested_metadata("nested", 16)];
    let batch = bridge.records_to_batch(&records).unwrap();
    let recovered = bridge.batch_to_records(&batch).unwrap();

    // Verify nested object is preserved
    assert!(recovered[0].metadata.contains_key("nested_obj"));
}

// ============================================================================
// Avro-Style Schema Serialization Tests
// ============================================================================

#[test]
fn test_avro_style_serialization() {
    let schema = ProximaSchema::vector_record_schema(512);
    let avro = schema.to_avro_style();

    assert_eq!(avro.schema_type, "record");
    assert_eq!(avro.namespace, "com.proximadb");
    assert_eq!(avro.name, "vector_record_v0");
    assert!(!avro.fields.is_empty());

    // Check field types
    let id_field = avro.fields.iter().find(|f| f.name == "id").unwrap();
    assert!(matches!(&id_field.field_type,
        AvroStyleType::Simple(t) | AvroStyleType::Union(t) if t.contains(&"string".to_string()) || t == "string"
    ));
}

#[test]
fn test_avro_json_roundtrip() {
    let schema = ProximaSchema::vector_record_schema(768);

    // Serialize to JSON
    let json = schema.to_avro_json().unwrap();
    assert!(!json.is_empty());

    // Parse and verify it's valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["type"], "record");

    // Roundtrip
    let recovered = ProximaSchema::from_avro_json(&json).unwrap();
    assert_eq!(recovered.vector_dimension(), Some(768));
}

#[test]
fn test_avro_style_with_custom_columns() {
    let metadata_fields = vec![
        ("name".to_string(), ProximaDataType::String),
        ("score".to_string(), ProximaDataType::Float64),
        ("count".to_string(), ProximaDataType::Int64),
        ("tags".to_string(), ProximaDataType::List {
            element: Box::new(ProximaDataType::String)
        }),
    ];

    let schema = ProximaSchema::with_metadata_columns(
        "custom_avro".to_string(),
        256,
        metadata_fields,
    );

    let avro = schema.to_avro_style();

    // Verify all fields present
    let field_names: Vec<&str> = avro.fields.iter().map(|f| f.name.as_str()).collect();
    assert!(field_names.contains(&"name"));
    assert!(field_names.contains(&"score"));
    assert!(field_names.contains(&"count"));
    assert!(field_names.contains(&"tags"));
}

// ============================================================================
// Schema Inference Tests
// ============================================================================

#[test]
fn test_infer_schema_from_records() {
    let records = vec![
        create_test_record("r1", 384),
        create_test_record("r2", 384),
        create_test_record("r3", 384),
    ];

    let schema = infer_schema_from_vector_records(&records, "inferred".to_string()).unwrap();

    assert_eq!(schema.vector_dimension(), Some(384));
    assert_eq!(schema.schema_id, "inferred");

    // Should have inferred metadata fields
    // Note: The exact column count depends on implementation
    assert!(schema.active_column_count() >= 3); // At least id, vector, timestamp
}

#[test]
fn test_infer_schema_empty_records() {
    let result = infer_schema_from_vector_records(&[], "empty".to_string());
    assert!(result.is_err());
}

#[test]
fn test_infer_schema_varying_metadata() {
    let mut record1 = create_test_record("r1", 64);
    record1.metadata.insert(
        "extra_field".to_string(),
        SqlValue {
            value: Some(ProtoSqlValueInner::StringValue("extra".to_string())),
        },
    );

    let record2 = create_test_record("r2", 64);

    let records = vec![record1, record2];
    let schema = infer_schema_from_vector_records(&records, "varied".to_string()).unwrap();

    // Should include union of all fields
    assert!(schema.active_column_count() >= 3);
}

// ============================================================================
// Validation Tests
// ============================================================================

#[test]
fn test_validate_records_success() {
    let schema = ProximaSchema::vector_record_schema(128);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![
        create_test_record("valid_1", 128),
        create_test_record("valid_2", 128),
    ];

    assert!(bridge.validate_records(&records).is_ok());
}

#[test]
fn test_validate_records_empty_id() {
    let schema = ProximaSchema::vector_record_schema(64);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![VectorRecord {
        id: "".to_string(),
        vector: vec![0.1; 64],
        ..Default::default()
    }];

    assert!(bridge.validate_records(&records).is_err());
}

#[test]
fn test_validate_records_wrong_dimension() {
    let schema = ProximaSchema::vector_record_schema(128);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![VectorRecord {
        id: "wrong_dim".to_string(),
        vector: vec![0.1; 64], // Wrong dimension
        ..Default::default()
    }];

    assert!(bridge.validate_records(&records).is_err());
}

// ============================================================================
// Legacy VectorRecord Compatibility Tests
// ============================================================================

#[test]
fn test_legacy_vector_record_schema() {
    let schema = ProximaSchema::vector_record_schema(1536);

    assert!(schema.is_legacy_vector_record);
    assert_eq!(schema.version, 0);
    assert_eq!(schema.schema_id, "vector_record_v0");
    assert_eq!(schema.vector_dimension(), Some(1536));

    // Should have the standard VectorRecord columns
    assert!(schema.column_by_name("id").is_some());
    assert!(schema.column_by_name("vector").is_some());
    assert!(schema.column_by_name("metadata").is_some());
    assert!(schema.column_by_name("timestamp").is_some());
    assert!(schema.column_by_name("version").is_some());
}

#[test]
fn test_legacy_bridge() {
    let bridge = DefaultVectorRecordBridge::legacy(512);

    assert!(bridge.schema().is_legacy_vector_record);
    assert_eq!(bridge.schema().vector_dimension(), Some(512));
}

// ============================================================================
// Schema Registry Integration Tests
// ============================================================================

#[tokio::test]
async fn test_schema_registry_store_and_retrieve() {
    let registry = InMemorySchemaRegistry::new();
    let schema = ProximaSchema::vector_record_schema(256);

    registry.register_schema("test_collection", schema.clone()).await.unwrap();

    let retrieved = registry.get_schema("test_collection", 0).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().vector_dimension(), Some(256));
}

#[tokio::test]
async fn test_schema_registry_latest() {
    let registry = InMemorySchemaRegistry::new();

    // Register multiple versions
    let mut schema_v0 = ProximaSchema::vector_record_schema(128);
    schema_v0.version = 0;
    schema_v0.schema_id = "v0".to_string();

    let mut schema_v1 = ProximaSchema::vector_record_schema(128);
    schema_v1.version = 1;
    schema_v1.schema_id = "v1".to_string();

    registry.register_schema("versioned", schema_v0).await.unwrap();
    registry.register_schema("versioned", schema_v1).await.unwrap();

    let latest = registry.get_latest_schema("versioned").await.unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().version, 1);
}

#[tokio::test]
async fn test_schema_registry_fingerprint_lookup() {
    let registry = InMemorySchemaRegistry::new();
    let schema = ProximaSchema::vector_record_schema(384);
    let fingerprint = schema.fingerprint;

    registry.register_schema("fingerprint_test", schema).await.unwrap();

    let found = registry.get_schema_by_fingerprint(fingerprint).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().vector_dimension(), Some(384));
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_very_large_vector() {
    let schema = ProximaSchema::vector_record_schema(4096);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![VectorRecord {
        id: "large_vec".to_string(),
        vector: vec![0.5; 4096],
        metadata: HashMap::new(),
        timestamp: Some(1234567890),
        ..Default::default()
    }];

    let batch = bridge.records_to_batch(&records).unwrap();
    let recovered = bridge.batch_to_records(&batch).unwrap();

    assert_eq!(recovered[0].vector.len(), 4096);
}

#[test]
fn test_single_record_batch() {
    let schema = ProximaSchema::vector_record_schema(32);
    let bridge = DefaultVectorRecordBridge::new(schema);

    let records = vec![create_test_record("single", 32)];
    let batch = bridge.records_to_batch(&records).unwrap();

    assert_eq!(batch.num_rows(), 1);
}

#[test]
fn test_schema_fingerprint_stability() {
    let schema1 = ProximaSchema::vector_record_schema(512);
    let schema2 = ProximaSchema::vector_record_schema(512);

    assert_eq!(schema1.fingerprint, schema2.fingerprint);

    let schema3 = ProximaSchema::vector_record_schema(768);
    assert_ne!(schema1.fingerprint, schema3.fingerprint);
}

#[test]
fn test_without_vectors_mode() {
    let schema = ProximaSchema::vector_record_schema(128);
    let bridge = DefaultVectorRecordBridge::new(schema).without_vectors();

    let records = vec![create_test_record("no_vec", 128)];
    let batch = bridge.records_to_batch(&records).unwrap();

    // Vector column should not be present
    assert!(batch.column_by_name("vector").is_none());

    // Other columns should be present
    assert!(batch.column_by_name("id").is_some());
    assert!(batch.column_by_name("metadata").is_some());
}
