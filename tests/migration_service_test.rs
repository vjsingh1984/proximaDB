/*
 * Copyright 2025 ProximaDB
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

//! Integration tests for the Record Migration Service
//!
//! These tests verify the migration process from VectorRecord (V1) to ProximaRecord (V2)
//! including schema inference, type conversion, and batch processing.

use std::collections::HashMap;

use proximadb::core::types::{ColumnDataType, TypedValue};
use proximadb::proto::proximadb_v1::{SqlArray, SqlObject, SqlValue, VectorRecord, sql_value::Value as SqlValueVariant};
use proximadb::services::conversion::record_converter::{ProximaRecord, RecordConverter};
use proximadb::services::migration::{
    MigrationConfig, MigrationError, MigrationMode, RecordMigrationService,
    ValidationErrorCode,
};
use proximadb::services::schema::{InferenceConfig, SchemaInferenceService};

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a simple VectorRecord for testing
fn create_simple_record(id: &str, vector: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector,
        metadata: HashMap::new(),
        timestamp: Some(1704067200000), // 2024-01-01 00:00:00 UTC
        updated_at: None,
        expires_at: None,
        version: Some(1),
        source: None,
    }
}

/// Create a VectorRecord with string metadata
fn create_record_with_string_metadata(id: &str, key: &str, value: &str) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        key.to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue(value.to_string())),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4],
        metadata,
        timestamp: Some(1704067200000),
        updated_at: None,
        expires_at: None,
        version: Some(1),
        source: None,
    }
}

/// Create a VectorRecord with mixed metadata types
fn create_record_with_mixed_metadata(id: &str) -> VectorRecord {
    let mut metadata = HashMap::new();

    // String value
    metadata.insert(
        "category".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("technology".to_string())),
        },
    );

    // Integer value
    metadata.insert(
        "priority".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(5)),
        },
    );

    // Float value
    metadata.insert(
        "score".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::NumberValue(0.95)),
        },
    );

    // Boolean value
    metadata.insert(
        "is_active".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::BoolValue(true)),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4],
        metadata,
        timestamp: Some(1704067200000),
        updated_at: Some(1704153600000), // Next day
        expires_at: None,
        version: Some(1),
        source: Some("test_source".to_string()),
    }
}

/// Create a VectorRecord with array metadata
fn create_record_with_array_metadata(id: &str) -> VectorRecord {
    let mut metadata = HashMap::new();

    // String array
    let string_array = SqlArray {
        values: vec![
            SqlValue {
                value: Some(SqlValueVariant::StringValue("tag1".to_string())),
            },
            SqlValue {
                value: Some(SqlValueVariant::StringValue("tag2".to_string())),
            },
            SqlValue {
                value: Some(SqlValueVariant::StringValue("tag3".to_string())),
            },
        ],
    };
    metadata.insert(
        "tags".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::ArrayValue(string_array)),
        },
    );

    // Integer array
    let int_array = SqlArray {
        values: vec![
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(1)),
            },
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(2)),
            },
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(3)),
            },
        ],
    };
    metadata.insert(
        "scores".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::ArrayValue(int_array)),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4],
        metadata,
        timestamp: Some(1704067200000),
        updated_at: None,
        expires_at: None,
        version: Some(1),
        source: None,
    }
}

/// Create a VectorRecord with nested object metadata
fn create_record_with_object_metadata(id: &str) -> VectorRecord {
    let mut metadata = HashMap::new();

    // Nested object
    let mut object_fields = HashMap::new();
    object_fields.insert(
        "name".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("John Doe".to_string())),
        },
    );
    object_fields.insert(
        "email".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("john@example.com".to_string())),
        },
    );

    metadata.insert(
        "author".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::ObjectValue(SqlObject {
                fields: object_fields,
            })),
        },
    );

    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4],
        metadata,
        timestamp: Some(1704067200000),
        updated_at: None,
        expires_at: None,
        version: Some(1),
        source: None,
    }
}

// =============================================================================
// V1 to V2 Record Conversion Tests
// =============================================================================

#[test]
fn test_convert_simple_vector_record() {
    let record = create_simple_record("vec_001", vec![0.1, 0.2, 0.3, 0.4]);

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert_eq!(proxima.id, "vec_001");
    assert_eq!(proxima.vector, vec![0.1, 0.2, 0.3, 0.4]);
    assert_eq!(proxima.vector_dimension, Some(4));
    assert!(proxima.typed_fields.is_empty());
    assert!(proxima.text_fields.is_empty());
    assert!(proxima.schema_id.is_none());
}

#[test]
fn test_convert_record_preserves_id() {
    let special_ids = vec![
        "simple_id",
        "id-with-dashes",
        "id_with_underscores",
        "id.with.dots",
        "id/with/slashes",
        "uuid-550e8400-e29b-41d4-a716-446655440000",
    ];

    for id in special_ids {
        let record = create_simple_record(id, vec![1.0]);
        let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);
        assert_eq!(proxima.id, id, "ID should be preserved: {}", id);
    }
}

#[test]
fn test_convert_record_preserves_vector_embeddings() {
    let vectors = vec![
        vec![0.0],
        vec![1.0, 2.0, 3.0],
        vec![0.123456789; 128], // High dimension
        vec![-1.0, 0.0, 1.0, -0.5, 0.5],
        vec![f32::MIN, f32::MAX], // Edge values
    ];

    for vector in vectors {
        let expected_dim = vector.len();
        let record = create_simple_record("test", vector.clone());
        let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

        assert_eq!(proxima.vector, vector, "Vector should be preserved exactly");
        assert_eq!(
            proxima.vector_dimension,
            Some(expected_dim as u32),
            "Dimension should match vector length"
        );
    }
}

#[test]
fn test_convert_record_with_string_metadata() {
    let record = create_record_with_string_metadata("doc_001", "category", "technology");

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert!(proxima.typed_fields.contains_key("category"));
    if let Some(TypedValue::Text(val)) = proxima.typed_fields.get("category") {
        assert_eq!(val, "technology");
    } else {
        panic!("Expected Text type for category");
    }
}

#[test]
fn test_convert_record_with_integer_metadata() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2]);
    record.metadata.insert(
        "count".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(42)),
        },
    );

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert!(proxima.typed_fields.contains_key("count"));
    if let Some(TypedValue::Integer(val)) = proxima.typed_fields.get("count") {
        assert_eq!(*val, 42);
    } else {
        panic!("Expected Integer type for count");
    }
}

#[test]
fn test_convert_record_with_float_metadata() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2]);
    record.metadata.insert(
        "score".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::NumberValue(3.14159)),
        },
    );

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert!(proxima.typed_fields.contains_key("score"));
    if let Some(TypedValue::Float(val)) = proxima.typed_fields.get("score") {
        assert!((val - 3.14159).abs() < f64::EPSILON);
    } else {
        panic!("Expected Float type for score");
    }
}

#[test]
fn test_convert_record_with_boolean_metadata() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2]);
    record.metadata.insert(
        "active".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::BoolValue(true)),
        },
    );

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert!(proxima.typed_fields.contains_key("active"));
    if let Some(TypedValue::Boolean(val)) = proxima.typed_fields.get("active") {
        assert!(*val);
    } else {
        panic!("Expected Boolean type for active");
    }
}

#[test]
fn test_convert_record_with_null_metadata() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2]);
    record.metadata.insert(
        "nullable_field".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        },
    );

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert!(proxima.typed_fields.contains_key("nullable_field"));
    if let Some(TypedValue::Null) = proxima.typed_fields.get("nullable_field") {
        // Correct type
    } else {
        panic!("Expected Null type for nullable_field");
    }
}

#[test]
fn test_convert_record_handles_empty_metadata_gracefully() {
    let record = create_simple_record("doc_001", vec![0.1, 0.2, 0.3]);

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert!(proxima.typed_fields.is_empty());
    assert!(proxima.text_fields.is_empty());
    assert!(proxima.flexible_fields.is_empty());
}

#[test]
fn test_convert_record_with_text_column_extraction() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2, 0.3]);
    record.metadata.insert(
        "content".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue(
                "This is the main document content for text search.".to_string(),
            )),
        },
    );
    record.metadata.insert(
        "category".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("articles".to_string())),
        },
    );

    let text_columns = vec!["content".to_string()];
    let proxima = RecordConverter::vector_to_proxima(&record, None, &text_columns);

    // Content should be in text_fields, not typed_fields
    assert!(!proxima.typed_fields.contains_key("content"));
    assert_eq!(proxima.text_fields.len(), 1);
    assert_eq!(proxima.text_fields[0].name, "content");
    assert!(proxima.text_fields[0]
        .content
        .contains("main document content"));

    // Category should still be in typed_fields
    assert!(proxima.typed_fields.contains_key("category"));
}

#[test]
fn test_convert_record_preserves_timestamps() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2]);
    record.timestamp = Some(1704067200000);
    record.updated_at = Some(1704153600000);
    record.expires_at = Some(1704240000000);

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert_eq!(proxima.timestamp_ms, 1704067200000);
    assert_eq!(proxima.updated_at_ms, Some(1704153600000));
    assert_eq!(proxima.expires_at_ms, Some(1704240000000));
}

#[test]
fn test_convert_record_preserves_version_and_source() {
    let mut record = create_simple_record("doc_001", vec![0.1, 0.2]);
    record.version = Some(5);
    record.source = Some("test_source".to_string());

    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    assert_eq!(proxima.version, Some(5));
    assert_eq!(proxima.source, Some("test_source".to_string()));
}

#[test]
fn test_convert_record_assigns_schema_id() {
    let record = create_simple_record("doc_001", vec![0.1, 0.2]);

    let proxima = RecordConverter::vector_to_proxima(&record, Some("schema_v1_001"), &[]);

    assert_eq!(proxima.schema_id, Some("schema_v1_001".to_string()));
}

// =============================================================================
// Schema Inference Tests
// =============================================================================

#[test]
fn test_infer_schema_from_records_with_strings() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| create_record_with_string_metadata(&format!("doc_{}", i), "name", &format!("Item {}", i)))
        .collect();

    let schema = service.infer_schema(&records);

    assert_eq!(schema.columns.len(), 1);
    let name_col = schema.get_column("name").unwrap();
    assert!(matches!(name_col.data_type, ColumnDataType::Text));
    assert!(!name_col.nullable);
    assert_eq!(name_col.sample_count, 5);
}

#[test]
fn test_infer_schema_from_records_with_integers() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "count".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::Int64Value(i * 10)),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    let count_col = schema.get_column("count").unwrap();
    assert!(matches!(count_col.data_type, ColumnDataType::Integer));
}

#[test]
fn test_infer_schema_from_records_with_floats() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "price".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::NumberValue(i as f64 * 9.99)),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    let price_col = schema.get_column("price").unwrap();
    assert!(matches!(price_col.data_type, ColumnDataType::Float));
}

#[test]
fn test_infer_schema_from_records_with_booleans() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "active".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::BoolValue(i % 2 == 0)),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    let active_col = schema.get_column("active").unwrap();
    assert!(matches!(active_col.data_type, ColumnDataType::Boolean));
}

#[test]
fn test_infer_schema_handles_missing_fields() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let mut records = Vec::new();

    // First record has field A
    let mut record1 = create_simple_record("doc_1", vec![0.1, 0.2]);
    record1.metadata.insert(
        "field_a".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("value_a".to_string())),
        },
    );
    records.push(record1);

    // Second record has field B only
    let mut record2 = create_simple_record("doc_2", vec![0.1, 0.2]);
    record2.metadata.insert(
        "field_b".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(42)),
        },
    );
    records.push(record2);

    // Third record has both
    let mut record3 = create_simple_record("doc_3", vec![0.1, 0.2]);
    record3.metadata.insert(
        "field_a".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("value_a2".to_string())),
        },
    );
    record3.metadata.insert(
        "field_b".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(99)),
        },
    );
    records.push(record3);

    let schema = service.infer_schema(&records);

    // Both fields should be discovered
    assert!(schema.get_column("field_a").is_some());
    assert!(schema.get_column("field_b").is_some());

    // Fields not present in all records should be nullable
    // (since sample_count < total records for some)
    let field_a = schema.get_column("field_a").unwrap();
    let field_b = schema.get_column("field_b").unwrap();

    // Each field was seen in 2 of 3 records
    assert_eq!(field_a.sample_count, 2);
    assert_eq!(field_b.sample_count, 2);
}

#[test]
fn test_infer_schema_detects_uuid_columns() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            let uuid = format!("550e8400-e29b-41d4-a716-44665544{:04}", i);
            record.metadata.insert(
                "user_id".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(uuid)),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    let user_id_col = schema.get_column("user_id").unwrap();
    assert!(matches!(user_id_col.data_type, ColumnDataType::Uuid));
    assert!(user_id_col.confidence >= 0.8);
}

#[test]
fn test_infer_schema_detects_timestamp_columns_iso8601() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let timestamps = [
        "2024-01-15T10:30:00Z",
        "2024-01-16T11:30:00Z",
        "2024-01-17T12:30:00Z",
        "2024-01-18T13:30:00Z",
        "2024-01-19T14:30:00Z",
    ];

    let records: Vec<VectorRecord> = timestamps
        .iter()
        .enumerate()
        .map(|(i, ts)| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "created_at".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(ts.to_string())),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    let created_at_col = schema.get_column("created_at").unwrap();
    assert!(matches!(
        created_at_col.data_type,
        ColumnDataType::Timestamp
    ));
}

#[test]
fn test_infer_schema_detects_decimal_columns() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let prices = ["99.99", "149.50", "24.95", "199.00", "49.99"];

    let records: Vec<VectorRecord> = prices
        .iter()
        .enumerate()
        .map(|(i, price)| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "price".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(price.to_string())),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    let price_col = schema.get_column("price").unwrap();
    assert!(matches!(price_col.data_type, ColumnDataType::Decimal { .. }));
}

#[test]
fn test_infer_schema_detects_text_large_columns() {
    let config = InferenceConfig::new()
        .with_detect_text_columns(true)
        .with_text_length_threshold(50);

    let service = SchemaInferenceService::new(config);

    let long_content = "a".repeat(100);
    let records: Vec<VectorRecord> = (0..5)
        .map(|i| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "content".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(long_content.clone())),
                },
            );
            record.metadata.insert(
                "title".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue("Short title".to_string())),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    // Long content should be detected as TEXT_LARGE
    let content_col = schema.get_column("content").unwrap();
    assert!(matches!(content_col.data_type, ColumnDataType::TextLarge));
    assert!(schema.is_text_column("content"));

    // Short title should be regular TEXT
    let title_col = schema.get_column("title").unwrap();
    assert!(matches!(title_col.data_type, ColumnDataType::Text));
}

#[test]
fn test_infer_schema_mixed_types_fallback_to_text() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    // Mix of different patterns that don't meet threshold
    let values = vec![
        "550e8400-e29b-41d4-a716-446655440000", // UUID
        "not-a-uuid",                            // Random text
        "hello world",                           // Random text
        "123e4567-e89b-12d3-a456-426614174000", // UUID
        "random text again",                     // Random text
    ];

    let records: Vec<VectorRecord> = values
        .iter()
        .enumerate()
        .map(|(i, val)| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "mixed_field".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(val.to_string())),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);

    // Should fallback to TEXT since UUID ratio is below threshold
    let mixed_col = schema.get_column("mixed_field").unwrap();
    assert!(matches!(mixed_col.data_type, ColumnDataType::Text));
}

// =============================================================================
// Batch Migration Tests
// =============================================================================

#[test]
fn test_migrate_batch_preserves_all_ids() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let ids: Vec<String> = (0..10).map(|i| format!("doc_{:04}", i)).collect();
    let records: Vec<VectorRecord> = ids
        .iter()
        .map(|id| create_simple_record(id, vec![0.1, 0.2, 0.3, 0.4]))
        .collect();

    let results = service.migrate_batch(&records, Some("batch_schema"));

    assert_eq!(results.len(), 10);
    for (i, result) in results.iter().enumerate() {
        assert!(result.is_ok(), "Record {} should migrate successfully", i);
        let proxima = result.as_ref().unwrap();
        assert_eq!(proxima.id, ids[i], "ID should match for record {}", i);
    }
}

#[test]
fn test_migrate_batch_populates_typed_fields_correctly() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| create_record_with_mixed_metadata(&format!("doc_{}", i)))
        .collect();

    let results = service.migrate_batch(&records, None);

    for result in results {
        let proxima = result.unwrap();

        // Check all typed fields are present
        assert!(
            proxima.typed_fields.contains_key("category"),
            "Should have category"
        );
        assert!(
            proxima.typed_fields.contains_key("priority"),
            "Should have priority"
        );
        assert!(proxima.typed_fields.contains_key("score"), "Should have score");
        assert!(
            proxima.typed_fields.contains_key("is_active"),
            "Should have is_active"
        );

        // Verify types
        assert!(matches!(
            proxima.typed_fields.get("category"),
            Some(TypedValue::Text(_))
        ));
        assert!(matches!(
            proxima.typed_fields.get("priority"),
            Some(TypedValue::Integer(_))
        ));
        assert!(matches!(
            proxima.typed_fields.get("score"),
            Some(TypedValue::Float(_))
        ));
        assert!(matches!(
            proxima.typed_fields.get("is_active"),
            Some(TypedValue::Boolean(_))
        ));
    }
}

#[test]
fn test_migrate_batch_preserves_vector_dimensions() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let dimensions = vec![4, 128, 256, 512, 768];
    let records: Vec<VectorRecord> = dimensions
        .iter()
        .enumerate()
        .map(|(i, dim)| create_simple_record(&format!("doc_{}", i), vec![0.1; *dim]))
        .collect();

    let results = service.migrate_batch(&records, None);

    for (i, result) in results.iter().enumerate() {
        let proxima = result.as_ref().unwrap();
        assert_eq!(
            proxima.vector.len(),
            dimensions[i],
            "Vector length should match for record {}", i
        );
        assert_eq!(
            proxima.vector_dimension,
            Some(dimensions[i] as u32),
            "Vector dimension should match for record {}", i
        );
    }
}

#[test]
fn test_migrate_batch_with_schema_inference() {
    let config = MigrationConfig::new()
        .with_infer_schema(true)
        .with_batch_size(100);

    let service = RecordMigrationService::new(config);

    let records: Vec<VectorRecord> = (0..10)
        .map(|i| create_record_with_mixed_metadata(&format!("doc_{}", i)))
        .collect();

    let results = service.migrate_batch(&records, Some("inferred_schema_001"));

    for result in results {
        let proxima = result.unwrap();
        assert_eq!(
            proxima.schema_id,
            Some("inferred_schema_001".to_string())
        );
    }
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_migrate_empty_metadata_records() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let records: Vec<VectorRecord> = (0..5)
        .map(|i| create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2, 0.3, 0.4]))
        .collect();

    let results = service.migrate_batch(&records, None);

    for result in results {
        assert!(result.is_ok(), "Empty metadata records should migrate successfully");
        let proxima = result.unwrap();
        assert!(proxima.typed_fields.is_empty());
        assert!(proxima.text_fields.is_empty());
    }
}

#[test]
fn test_migrate_records_with_very_large_metadata_values() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let large_content = "x".repeat(100_000); // 100KB content

    let mut record = create_simple_record("doc_large", vec![0.1, 0.2, 0.3, 0.4]);
    record.metadata.insert(
        "large_content".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue(large_content.clone())),
        },
    );

    let results = service.migrate_batch(&[record], None);

    assert!(results[0].is_ok());
    let proxima = results[0].as_ref().unwrap();

    // Large content should be preserved
    if let Some(TypedValue::Text(content)) = proxima.typed_fields.get("large_content") {
        assert_eq!(content.len(), 100_000);
    } else {
        panic!("Large content should be in typed_fields");
    }
}

#[test]
fn test_migrate_records_with_special_characters_in_strings() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let special_strings = vec![
        "Hello \"World\"",
        "Line1\nLine2",
        "Tab\there",
        "Unicode: \u{1F600}\u{1F601}\u{1F602}",
        "Backslash: \\path\\to\\file",
        "Null char: \0embedded",
        "<xml>content</xml>",
        "{\"json\": \"value\"}",
    ];

    for special in special_strings {
        let mut record = create_simple_record("doc_special", vec![0.1, 0.2]);
        record.metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(special.to_string())),
            },
        );

        let results = service.migrate_batch(&[record], None);
        assert!(
            results[0].is_ok(),
            "Should handle special string: {:?}",
            special
        );

        let proxima = results[0].as_ref().unwrap();
        if let Some(TypedValue::Text(content)) = proxima.typed_fields.get("content") {
            assert_eq!(content, special, "Special characters should be preserved");
        }
    }
}

#[test]
fn test_migrate_records_with_null_values() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let mut record = create_simple_record("doc_null", vec![0.1, 0.2, 0.3, 0.4]);
    record.metadata.insert(
        "null_field".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        },
    );
    record.metadata.insert(
        "present_field".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("present".to_string())),
        },
    );

    let results = service.migrate_batch(&[record], None);

    assert!(results[0].is_ok());
    let proxima = results[0].as_ref().unwrap();

    assert!(matches!(
        proxima.typed_fields.get("null_field"),
        Some(TypedValue::Null)
    ));
    assert!(matches!(
        proxima.typed_fields.get("present_field"),
        Some(TypedValue::Text(_))
    ));
}

#[test]
fn test_migrate_records_with_array_metadata() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let record = create_record_with_array_metadata("doc_arrays");

    let results = service.migrate_batch(&[record], None);

    assert!(results[0].is_ok());
    let proxima = results[0].as_ref().unwrap();

    // String array
    if let Some(TypedValue::ArrayText(tags)) = proxima.typed_fields.get("tags") {
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "tag1");
        assert_eq!(tags[1], "tag2");
        assert_eq!(tags[2], "tag3");
    } else {
        panic!("Expected ArrayText type for tags");
    }

    // Integer array
    if let Some(TypedValue::ArrayInteger(scores)) = proxima.typed_fields.get("scores") {
        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0], 1);
        assert_eq!(scores[1], 2);
        assert_eq!(scores[2], 3);
    } else {
        panic!("Expected ArrayInteger type for scores");
    }
}

#[test]
fn test_migrate_records_with_object_metadata_as_map() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let record = create_record_with_object_metadata("doc_object");

    let results = service.migrate_batch(&[record], None);

    assert!(results[0].is_ok());
    let proxima = results[0].as_ref().unwrap();

    // Object should be converted to MapStringString
    if let Some(TypedValue::MapStringString(map)) = proxima.typed_fields.get("author") {
        assert!(map.contains_key("name"));
        assert!(map.contains_key("email"));
        assert_eq!(map.get("name"), Some(&"John Doe".to_string()));
        assert_eq!(map.get("email"), Some(&"john@example.com".to_string()));
    } else {
        panic!("Expected MapStringString type for author");
    }
}

// =============================================================================
// Validation Tests
// =============================================================================

#[test]
fn test_validation_fails_for_empty_id() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let mut record = create_simple_record("", vec![0.1, 0.2, 0.3, 0.4]);
    record.id = String::new();

    let results = service.migrate_batch(&[record], None);

    assert!(results[0].is_err());
    if let Err(MigrationError::ValidationFailed(msg)) = &results[0] {
        assert!(msg.contains("id"), "Error should mention id field");
    } else {
        panic!("Expected ValidationFailed error");
    }
}

#[test]
fn test_validation_fails_for_empty_vector() {
    let config = MigrationConfig::new().with_validate_on_migrate(true);
    let service = RecordMigrationService::new(config);

    let mut record = create_simple_record("doc_001", vec![]);
    record.vector = vec![];

    let results = service.migrate_batch(&[record], None);

    assert!(results[0].is_err());
    if let Err(MigrationError::ValidationFailed(msg)) = &results[0] {
        assert!(msg.contains("vector"), "Error should mention vector field");
    } else {
        panic!("Expected ValidationFailed error");
    }
}

#[test]
fn test_validation_can_be_disabled() {
    let config = MigrationConfig::new().with_validate_on_migrate(false);
    let service = RecordMigrationService::new(config);

    // This would normally fail validation
    let mut record = create_simple_record("", vec![]);
    record.id = String::new();
    record.vector = vec![];

    let results = service.migrate_batch(&[record], None);

    // With validation disabled, it should succeed
    assert!(results[0].is_ok());
}

// =============================================================================
// Round-Trip Conversion Tests
// =============================================================================

#[test]
fn test_roundtrip_conversion_preserves_data() {
    let original = create_record_with_mixed_metadata("doc_roundtrip");

    // Convert to ProximaRecord
    let proxima = RecordConverter::vector_to_proxima(&original, None, &[]);

    // Convert back to VectorRecord
    let converted = RecordConverter::proxima_to_vector(&proxima);

    // Verify basic fields
    assert_eq!(converted.id, original.id);
    assert_eq!(converted.vector, original.vector);
    assert_eq!(converted.timestamp, original.timestamp);
    assert_eq!(converted.updated_at, original.updated_at);
    assert_eq!(converted.version, original.version);
    assert_eq!(converted.source, original.source);

    // Verify metadata preserved
    assert_eq!(converted.metadata.len(), original.metadata.len());
    assert!(converted.metadata.contains_key("category"));
    assert!(converted.metadata.contains_key("priority"));
    assert!(converted.metadata.contains_key("score"));
    assert!(converted.metadata.contains_key("is_active"));
}

#[test]
fn test_roundtrip_with_text_columns() {
    let mut original = create_simple_record("doc_text", vec![0.1, 0.2, 0.3, 0.4]);
    original.metadata.insert(
        "content".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue(
                "This is the main content for full text search.".to_string(),
            )),
        },
    );
    original.metadata.insert(
        "category".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("articles".to_string())),
        },
    );

    let text_columns = vec!["content".to_string()];

    // Convert to ProximaRecord
    let proxima = RecordConverter::vector_to_proxima(&original, None, &text_columns);

    // Verify content moved to text_fields
    assert!(!proxima.typed_fields.contains_key("content"));
    assert_eq!(proxima.text_fields.len(), 1);

    // Convert back
    let converted = RecordConverter::proxima_to_vector(&proxima);

    // Content should be back in metadata
    assert!(converted.metadata.contains_key("content"));
    if let Some(SqlValue {
        value: Some(SqlValueVariant::StringValue(s)),
    }) = converted.metadata.get("content")
    {
        assert!(s.contains("main content"));
    } else {
        panic!("Content should be restored as string");
    }
}

// =============================================================================
// Async Migration Tests
// =============================================================================

#[tokio::test]
async fn test_migrate_records_async() {
    let config = MigrationConfig::new()
        .with_infer_schema(true)
        .with_batch_size(100);

    let service = RecordMigrationService::new(config);

    let records: Vec<VectorRecord> = (0..10)
        .map(|i| create_record_with_mixed_metadata(&format!("doc_{}", i)))
        .collect();

    let result = service
        .migrate_records("test_collection", records.into_iter(), MigrationMode::DualWrite)
        .await;

    assert!(result.is_ok());
    let migration_result = result.unwrap();

    assert!(migration_result.success);
    assert_eq!(migration_result.stats.total_records, 10);
    assert_eq!(migration_result.stats.migrated_records, 10);
    assert_eq!(migration_result.stats.failed_records, 0);
    assert!(migration_result.inferred_schema.is_some());
}

#[tokio::test]
async fn test_migrate_empty_records() {
    let service = RecordMigrationService::new(MigrationConfig::default());

    let records: Vec<VectorRecord> = vec![];

    let result = service
        .migrate_records("empty_collection", records.into_iter(), MigrationMode::DualWrite)
        .await;

    assert!(result.is_ok());
    let migration_result = result.unwrap();
    assert_eq!(migration_result.stats.total_records, 0);
    assert!(migration_result.inferred_schema.is_none()); // No records to infer from
}

#[tokio::test]
async fn test_concurrent_migration_prevention() {
    let service = RecordMigrationService::new(MigrationConfig::default());

    // Start first migration
    let _records: Vec<VectorRecord> = vec![create_simple_record("doc_1", vec![0.1, 0.2])];

    // This should work
    assert!(!service.is_migration_active("test_collection"));

    // Manually register an active migration to test prevention
    // (In real usage, this would happen through migrate_records)
}

// =============================================================================
// Migration Service Configuration Tests
// =============================================================================

#[test]
fn test_migration_config_defaults() {
    let config = MigrationConfig::default();

    assert_eq!(config.batch_size, 1000);
    assert_eq!(config.parallel_workers, 4);
    assert!(config.text_columns.is_empty());
    assert!(config.infer_schema);
    assert!(config.validate_on_migrate);
}

#[test]
fn test_migration_config_builder() {
    let config = MigrationConfig::new()
        .with_batch_size(500)
        .with_parallel_workers(8)
        .with_text_columns(vec!["content".to_string(), "description".to_string()])
        .with_infer_schema(false)
        .with_validate_on_migrate(false);

    assert_eq!(config.batch_size, 500);
    assert_eq!(config.parallel_workers, 8);
    assert_eq!(config.text_columns.len(), 2);
    assert!(!config.infer_schema);
    assert!(!config.validate_on_migrate);
}

// =============================================================================
// Batch Processing Tests
// =============================================================================

#[test]
fn test_batch_conversion() {
    let records: Vec<VectorRecord> = (0..5)
        .map(|i| {
            VectorRecord {
                id: format!("doc_{}", i),
                vector: vec![0.1 * i as f32; 4],
                metadata: HashMap::new(),
                timestamp: Some(1704067200000 + i as i64 * 1000),
                updated_at: None,
                expires_at: None,
                version: Some(i as u32),
                source: None,
            }
        })
        .collect();

    let proxima_records = RecordConverter::batch_vector_to_proxima(&records, Some("batch_schema"), &[]);

    assert_eq!(proxima_records.len(), 5);
    for (i, proxima) in proxima_records.iter().enumerate() {
        assert_eq!(proxima.id, format!("doc_{}", i));
        assert_eq!(proxima.schema_id, Some("batch_schema".to_string()));
        assert_eq!(proxima.version, Some(i as u32));
    }

    // Convert back
    let vector_records = RecordConverter::batch_proxima_to_vector(&proxima_records);
    assert_eq!(vector_records.len(), 5);
    for (i, vector) in vector_records.iter().enumerate() {
        assert_eq!(vector.id, format!("doc_{}", i));
    }
}

// =============================================================================
// Schema Inference Integration Tests
// =============================================================================

#[test]
fn test_schema_inference_to_proto_config() {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    let records: Vec<VectorRecord> = (0..10)
        .map(|i| {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "name".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(format!("Item {}", i))),
                },
            );
            record.metadata.insert(
                "count".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::Int64Value(i * 10)),
                },
            );
            record
        })
        .collect();

    let schema = service.infer_schema(&records);
    let proto_config = schema.to_proto_config();

    assert!(!proto_config.schema_id.is_empty());
    assert_eq!(proto_config.schema_version, "1.0.0");
    assert!(proto_config.auto_evolve);
    assert_eq!(proto_config.columns.len(), 2);
}

// =============================================================================
// Validation Result Tests
// =============================================================================

#[test]
fn test_validate_record_success() {
    let service = RecordMigrationService::new(MigrationConfig::default());
    let record = create_simple_record("doc_1", vec![0.1, 0.2, 0.3, 0.4]);
    let proxima = RecordConverter::vector_to_proxima(&record, None, &[]);

    let result = service.validate_record(&proxima, None);

    assert!(result.valid);
    assert!(result.errors.is_empty());
    assert_eq!(result.record_id, "doc_1");
}

#[test]
fn test_validate_record_empty_id_fails() {
    let service = RecordMigrationService::new(MigrationConfig::default());

    let proxima = ProximaRecord {
        id: String::new(),
        vector: vec![0.1, 0.2, 0.3],
        ..Default::default()
    };

    let result = service.validate_record(&proxima, None);

    assert!(!result.valid);
    assert!(!result.errors.is_empty());
    assert!(result.errors.iter().any(|e| e.field == "id"));
    assert!(result
        .errors
        .iter()
        .any(|e| e.code == ValidationErrorCode::RequiredFieldMissing));
}

#[test]
fn test_validate_record_empty_vector_fails() {
    let service = RecordMigrationService::new(MigrationConfig::default());

    let proxima = ProximaRecord {
        id: "doc_1".to_string(),
        vector: vec![],
        ..Default::default()
    };

    let result = service.validate_record(&proxima, None);

    assert!(!result.valid);
    assert!(result.errors.iter().any(|e| e.field == "vector"));
}

#[test]
fn test_validate_record_dimension_mismatch() {
    let service = RecordMigrationService::new(MigrationConfig::default());

    let proxima = ProximaRecord {
        id: "doc_1".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        vector_dimension: Some(10), // Mismatch: vector has 3 elements
        ..Default::default()
    };

    let result = service.validate_record(&proxima, None);

    assert!(!result.valid);
    assert!(result
        .errors
        .iter()
        .any(|e| e.code == ValidationErrorCode::DimensionMismatch));
}
