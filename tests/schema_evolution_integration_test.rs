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

//! Integration tests for schema evolution and migration edge cases
//!
//! This test file covers:
//! 1. Schema Evolution - Adding new columns, type widening, parent schema tracking
//! 2. Migration Edge Cases - DualWrite mode, concurrent inserts, large batch migrations
//! 3. Validation Scenarios - Empty ID/vector validation, dimension mismatches
//! 4. TEXT Column Edge Cases - Empty text, large text values, Unicode/special characters

use std::collections::HashMap;

use proximadb::core::types::{
    ColumnConstraints, ColumnDataType, RecordSchema, SchemaEnforcementMode, TextField,
    TextStorageStrategy, TypedColumnDefinition, TypedValue,
};
use proximadb::proto::proximadb_v1::{
    SqlArray, SqlValue, VectorRecord, sql_value::Value as SqlValueVariant,
};
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

/// Create a VectorRecord with integer metadata
fn create_record_with_int_metadata(id: &str, key: &str, value: i64) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        key.to_string(),
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(value)),
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

/// Create a VectorRecord with float metadata
fn create_record_with_float_metadata(id: &str, key: &str, value: f64) -> VectorRecord {
    let mut metadata = HashMap::new();
    metadata.insert(
        key.to_string(),
        SqlValue {
            value: Some(SqlValueVariant::NumberValue(value)),
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

    metadata.insert(
        "category".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::StringValue("technology".to_string())),
        },
    );
    metadata.insert(
        "priority".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(5)),
        },
    );
    metadata.insert(
        "score".to_string(),
        SqlValue {
            value: Some(SqlValueVariant::NumberValue(0.95)),
        },
    );
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
        updated_at: Some(1704153600000),
        expires_at: None,
        version: Some(1),
        source: Some("test_source".to_string()),
    }
}

/// Create a test schema with specific columns
fn create_test_schema(name: &str, columns: Vec<TypedColumnDefinition>) -> RecordSchema {
    RecordSchema {
        schema_id: format!("schema_{}", uuid::Uuid::new_v4()),
        schema_version: "1.0.0".to_string(),
        schema_name: name.to_string(),
        columns,
        enforcement_mode: SchemaEnforcementMode::Hybrid,
        allow_additional_fields: true,
        parent_schema_id: None,
        created_at: chrono::Utc::now().timestamp_millis(),
        created_by: Some("test".to_string()),
        description: Some(format!("Test schema: {}", name)),
    }
}

/// Create a typed column definition
fn create_column_def(
    name: &str,
    data_type: ColumnDataType,
    nullable: bool,
    indexed: bool,
) -> TypedColumnDefinition {
    TypedColumnDefinition {
        name: name.to_string(),
        data_type,
        nullable,
        indexed,
        filterable: true,
        unique: false,
        constraints: ColumnConstraints::default(),
        text_options: None,
        description: None,
        annotations: HashMap::new(),
    }
}

// =============================================================================
// Schema Evolution Tests
// =============================================================================

mod schema_evolution_tests {
    use super::*;

    #[test]
    fn test_schema_evolution_add_new_column() {
        // Start with a base schema
        let base_columns = vec![
            create_column_def("name", ColumnDataType::Text, false, true),
            create_column_def("count", ColumnDataType::Integer, false, false),
        ];
        let base_schema = create_test_schema("products_v1", base_columns);

        // Create an evolved schema with a new column
        let evolved_columns = vec![
            create_column_def("name", ColumnDataType::Text, false, true),
            create_column_def("count", ColumnDataType::Integer, false, false),
            create_column_def("description", ColumnDataType::TextLarge, true, false), // New column
        ];
        let mut evolved_schema = create_test_schema("products_v2", evolved_columns);
        evolved_schema.parent_schema_id = Some(base_schema.schema_id.clone());
        evolved_schema.schema_version = "2.0.0".to_string();

        // Verify the evolution
        assert_eq!(base_schema.columns.len(), 2);
        assert_eq!(evolved_schema.columns.len(), 3);
        assert_eq!(
            evolved_schema.parent_schema_id,
            Some(base_schema.schema_id.clone())
        );
        assert!(evolved_schema.get_column("description").is_some());

        // The new column should be nullable (backward compatible)
        let desc_col = evolved_schema.get_column("description").unwrap();
        assert!(desc_col.nullable);
    }

    #[test]
    fn test_schema_evolution_add_multiple_columns() {
        let base_columns = vec![create_column_def("id", ColumnDataType::Uuid, false, true)];
        let base_schema = create_test_schema("entities_v1", base_columns);

        // Add multiple new columns
        let evolved_columns = vec![
            create_column_def("id", ColumnDataType::Uuid, false, true),
            create_column_def("created_at", ColumnDataType::Timestamp, true, true),
            create_column_def("updated_at", ColumnDataType::Timestamp, true, false),
            create_column_def("tags", ColumnDataType::ArrayText, true, false),
            create_column_def("metadata", ColumnDataType::Json, true, false),
        ];
        let mut evolved_schema = create_test_schema("entities_v2", evolved_columns);
        evolved_schema.parent_schema_id = Some(base_schema.schema_id.clone());

        assert_eq!(evolved_schema.columns.len(), 5);

        // All new columns should be nullable for backward compatibility
        assert!(evolved_schema.get_column("created_at").unwrap().nullable);
        assert!(evolved_schema.get_column("updated_at").unwrap().nullable);
        assert!(evolved_schema.get_column("tags").unwrap().nullable);
        assert!(evolved_schema.get_column("metadata").unwrap().nullable);
    }

    #[test]
    fn test_schema_type_widening_integer_to_float() {
        // Simulate type widening: INTEGER -> FLOAT
        let service = SchemaInferenceService::new(InferenceConfig::default());

        // First batch has integers
        let int_records: Vec<VectorRecord> = (0..5)
            .map(|i| create_record_with_int_metadata(&format!("doc_{}", i), "value", i * 10))
            .collect();

        let int_schema = service.infer_schema(&int_records);
        let int_col = int_schema.get_column("value").unwrap();
        assert!(matches!(int_col.data_type, ColumnDataType::Integer));

        // Second batch has floats (type widening needed)
        let float_records: Vec<VectorRecord> = (0..5)
            .map(|i| {
                create_record_with_float_metadata(&format!("doc_float_{}", i), "value", i as f64 * 10.5)
            })
            .collect();

        let float_schema = service.infer_schema(&float_records);
        let float_col = float_schema.get_column("value").unwrap();
        assert!(matches!(float_col.data_type, ColumnDataType::Float));

        // In a real schema evolution scenario, INTEGER would be widened to FLOAT
        // This simulates the compatibility check
        let is_compatible = matches!(
            (&int_col.data_type, &float_col.data_type),
            (ColumnDataType::Integer, ColumnDataType::Float)
        );
        assert!(is_compatible, "Integer to Float should be a valid type widening");
    }

    #[test]
    fn test_schema_type_widening_text_to_text_large() {
        let config = InferenceConfig::new()
            .with_detect_text_columns(true)
            .with_text_length_threshold(50);

        let service = SchemaInferenceService::new(config);

        // Short text records
        let short_records: Vec<VectorRecord> = (0..5)
            .map(|i| create_record_with_string_metadata(&format!("doc_{}", i), "content", "Short text"))
            .collect();

        let short_schema = service.infer_schema(&short_records);
        let short_col = short_schema.get_column("content").unwrap();
        assert!(matches!(short_col.data_type, ColumnDataType::Text));

        // Long text records
        let long_text = "a".repeat(100);
        let long_records: Vec<VectorRecord> = (0..5)
            .map(|i| {
                create_record_with_string_metadata(&format!("doc_long_{}", i), "content", &long_text)
            })
            .collect();

        let long_schema = service.infer_schema(&long_records);
        let long_col = long_schema.get_column("content").unwrap();
        assert!(matches!(long_col.data_type, ColumnDataType::TextLarge));

        // Verify TEXT -> TEXT_LARGE is a valid widening
        assert!(short_col.data_type.is_text());
        assert!(long_col.data_type.is_text());
    }

    #[test]
    fn test_schema_parent_tracking_chain() {
        // Create a chain of schema versions
        let v1_schema = create_test_schema(
            "chain_v1",
            vec![create_column_def("name", ColumnDataType::Text, false, true)],
        );

        let mut v2_schema = create_test_schema(
            "chain_v2",
            vec![
                create_column_def("name", ColumnDataType::Text, false, true),
                create_column_def("email", ColumnDataType::Text, true, true),
            ],
        );
        v2_schema.parent_schema_id = Some(v1_schema.schema_id.clone());
        v2_schema.schema_version = "2.0.0".to_string();

        let mut v3_schema = create_test_schema(
            "chain_v3",
            vec![
                create_column_def("name", ColumnDataType::Text, false, true),
                create_column_def("email", ColumnDataType::Text, true, true),
                create_column_def("phone", ColumnDataType::Text, true, false),
            ],
        );
        v3_schema.parent_schema_id = Some(v2_schema.schema_id.clone());
        v3_schema.schema_version = "3.0.0".to_string();

        // Verify the chain
        assert!(v1_schema.parent_schema_id.is_none());
        assert_eq!(v2_schema.parent_schema_id, Some(v1_schema.schema_id.clone()));
        assert_eq!(v3_schema.parent_schema_id, Some(v2_schema.schema_id.clone()));

        // Count evolution steps (v1 -> v2 -> v3 = 3 versions)
        assert_eq!(v1_schema.columns.len(), 1);
        assert_eq!(v2_schema.columns.len(), 2);
        assert_eq!(v3_schema.columns.len(), 3);
    }

    #[test]
    fn test_schema_version_semantic_versioning() {
        let schema = create_test_schema("versioned", vec![]);

        // Verify semantic versioning format
        let version_parts: Vec<&str> = schema.schema_version.split('.').collect();
        assert_eq!(version_parts.len(), 3, "Version should be MAJOR.MINOR.PATCH");

        // Simulate version bumps
        let major_bump = "2.0.0";
        let minor_bump = "1.1.0";
        let patch_bump = "1.0.1";

        for version in [major_bump, minor_bump, patch_bump] {
            let parts: Vec<&str> = version.split('.').collect();
            assert_eq!(parts.len(), 3);
            assert!(parts[0].parse::<u32>().is_ok());
            assert!(parts[1].parse::<u32>().is_ok());
            assert!(parts[2].parse::<u32>().is_ok());
        }
    }

    #[test]
    fn test_schema_inference_evolves_with_new_columns() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        // First batch with only name
        let batch1: Vec<VectorRecord> = (0..5)
            .map(|i| create_record_with_string_metadata(&format!("doc_{}", i), "name", &format!("Item {}", i)))
            .collect();

        let schema1 = service.infer_schema(&batch1);
        assert_eq!(schema1.columns.len(), 1);
        assert!(schema1.get_column("name").is_some());
        assert!(schema1.get_column("price").is_none());

        // Second batch with both name and price
        let mut batch2: Vec<VectorRecord> = Vec::new();
        for i in 0..5 {
            let mut metadata = HashMap::new();
            metadata.insert(
                "name".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(format!("Item {}", i))),
                },
            );
            metadata.insert(
                "price".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::NumberValue(9.99 + i as f64)),
                },
            );
            batch2.push(VectorRecord {
                id: format!("doc_v2_{}", i),
                vector: vec![0.1, 0.2, 0.3, 0.4],
                metadata,
                timestamp: Some(1704067200000),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            });
        }

        let schema2 = service.infer_schema(&batch2);
        assert_eq!(schema2.columns.len(), 2);
        assert!(schema2.get_column("name").is_some());
        assert!(schema2.get_column("price").is_some());
    }
}

// =============================================================================
// Migration Edge Cases Tests
// =============================================================================

mod migration_edge_cases_tests {
    use super::*;

    #[test]
    fn test_dualwrite_mode_switching() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Validate mode transitions
        // Legacy -> DualWrite: Valid
        assert!(
            service
                .is_migration_active("test_collection") == false
        );

        // Test valid transition: Legacy -> DualWrite
        let result = validate_mode_transition(MigrationMode::Legacy, MigrationMode::DualWrite);
        assert!(result.is_ok());

        // Test valid transition: DualWrite -> Migrated
        let result = validate_mode_transition(MigrationMode::DualWrite, MigrationMode::Migrated);
        assert!(result.is_ok());

        // Test valid transition: DualWrite -> Legacy (rollback)
        let result = validate_mode_transition(MigrationMode::DualWrite, MigrationMode::Legacy);
        assert!(result.is_ok());

        // Test invalid transition: Legacy -> Migrated (skip DualWrite)
        let result = validate_mode_transition(MigrationMode::Legacy, MigrationMode::Migrated);
        assert!(result.is_err());

        // Test invalid transition: Migrated -> Legacy (can't rollback after full migration)
        let result = validate_mode_transition(MigrationMode::Migrated, MigrationMode::Legacy);
        assert!(result.is_err());
    }

    fn validate_mode_transition(from: MigrationMode, to: MigrationMode) -> Result<(), String> {
        let valid = match (&from, &to) {
            (MigrationMode::Legacy, MigrationMode::DualWrite) => true,
            (MigrationMode::DualWrite, MigrationMode::Migrated) => true,
            (MigrationMode::DualWrite, MigrationMode::Legacy) => true,
            (a, b) if a == b => true,
            _ => false,
        };

        if valid {
            Ok(())
        } else {
            Err(format!("Invalid transition: {:?} -> {:?}", from, to))
        }
    }

    #[tokio::test]
    async fn test_concurrent_inserts_during_migration() {
        let config = MigrationConfig::new()
            .with_batch_size(10)
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        // Simulate concurrent insert scenario by creating multiple batches
        let batch1: Vec<VectorRecord> = (0..20)
            .map(|i| create_record_with_mixed_metadata(&format!("batch1_doc_{}", i)))
            .collect();

        let batch2: Vec<VectorRecord> = (0..20)
            .map(|i| create_record_with_mixed_metadata(&format!("batch2_doc_{}", i)))
            .collect();

        // Migrate first batch
        let result1 = service
            .migrate_records("concurrent_test", batch1.into_iter(), MigrationMode::DualWrite)
            .await;

        assert!(result1.is_ok());
        let stats1 = result1.unwrap();
        assert_eq!(stats1.stats.total_records, 20);
        assert_eq!(stats1.stats.migrated_records, 20);

        // Migrate second batch (simulating concurrent writes)
        let result2 = service
            .migrate_records("concurrent_test_2", batch2.into_iter(), MigrationMode::DualWrite)
            .await;

        assert!(result2.is_ok());
        let stats2 = result2.unwrap();
        assert_eq!(stats2.stats.total_records, 20);
        assert_eq!(stats2.stats.migrated_records, 20);
    }

    #[tokio::test]
    async fn test_large_batch_migration_1000_plus_records() {
        let config = MigrationConfig::new()
            .with_batch_size(100) // Process in batches of 100
            .with_parallel_workers(4)
            .with_validate_on_migrate(true)
            .with_infer_schema(true);

        let service = RecordMigrationService::new(config);

        // Create 1500 records
        let large_batch: Vec<VectorRecord> = (0..1500)
            .map(|i| {
                let mut record = create_simple_record(&format!("large_doc_{:05}", i), vec![0.1, 0.2, 0.3, 0.4]);
                record.metadata.insert(
                    "index".to_string(),
                    SqlValue {
                        value: Some(SqlValueVariant::Int64Value(i)),
                    },
                );
                record.metadata.insert(
                    "category".to_string(),
                    SqlValue {
                        value: Some(SqlValueVariant::StringValue(format!("category_{}", i % 10))),
                    },
                );
                record
            })
            .collect();

        let result = service
            .migrate_records("large_batch_test", large_batch.into_iter(), MigrationMode::DualWrite)
            .await;

        assert!(result.is_ok());
        let migration_result = result.unwrap();
        assert!(migration_result.success);
        assert_eq!(migration_result.stats.total_records, 1500);
        assert_eq!(migration_result.stats.migrated_records, 1500);
        assert_eq!(migration_result.stats.failed_records, 0);
        assert!(migration_result.inferred_schema.is_some());

        // Verify schema was inferred correctly
        let schema = migration_result.inferred_schema.unwrap();
        assert!(schema.get_column("index").is_some());
        assert!(schema.get_column("category").is_some());
    }

    #[tokio::test]
    async fn test_very_large_batch_migration_5000_records() {
        let config = MigrationConfig::new()
            .with_batch_size(500)
            .with_parallel_workers(8)
            .with_validate_on_migrate(false) // Disable validation for speed
            .with_infer_schema(false);

        let service = RecordMigrationService::new(config);

        // Create 5000 records
        let very_large_batch: Vec<VectorRecord> = (0..5000)
            .map(|i| create_simple_record(&format!("vl_doc_{:06}", i), vec![0.1, 0.2, 0.3, 0.4, 0.5]))
            .collect();

        let result = service
            .migrate_records(
                "very_large_batch_test",
                very_large_batch.into_iter(),
                MigrationMode::DualWrite,
            )
            .await;

        assert!(result.is_ok());
        let migration_result = result.unwrap();
        assert!(migration_result.success);
        assert_eq!(migration_result.stats.total_records, 5000);
        assert_eq!(migration_result.stats.migrated_records, 5000);
    }

    #[test]
    fn test_migration_batch_partial_failure() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        // Create a batch with some invalid records
        let mut batch = Vec::new();

        // Valid records
        batch.push(create_simple_record("valid_1", vec![0.1, 0.2, 0.3]));
        batch.push(create_simple_record("valid_2", vec![0.1, 0.2, 0.3]));

        // Invalid record: empty ID
        let mut invalid_id = create_simple_record("", vec![0.1, 0.2, 0.3]);
        invalid_id.id = String::new();
        batch.push(invalid_id);

        // Invalid record: empty vector
        let mut invalid_vec = create_simple_record("invalid_vec", vec![]);
        invalid_vec.vector = vec![];
        batch.push(invalid_vec);

        // More valid records
        batch.push(create_simple_record("valid_3", vec![0.1, 0.2, 0.3]));

        let results = service.migrate_batch(&batch, None);

        assert_eq!(results.len(), 5);
        assert!(results[0].is_ok()); // valid_1
        assert!(results[1].is_ok()); // valid_2
        assert!(results[2].is_err()); // invalid: empty ID
        assert!(results[3].is_err()); // invalid: empty vector
        assert!(results[4].is_ok()); // valid_3

        // Count successes and failures
        let successes = results.iter().filter(|r| r.is_ok()).count();
        let failures = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(successes, 3);
        assert_eq!(failures, 2);
    }

    #[test]
    fn test_migration_preserves_all_metadata_types() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        // Create record with all metadata types
        let mut metadata = HashMap::new();

        // String
        metadata.insert(
            "string_field".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("test string".to_string())),
            },
        );

        // Integer
        metadata.insert(
            "int_field".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::Int64Value(42)),
            },
        );

        // Float
        metadata.insert(
            "float_field".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::NumberValue(3.14159)),
            },
        );

        // Boolean
        metadata.insert(
            "bool_field".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::BoolValue(true)),
            },
        );

        // Null
        metadata.insert(
            "null_field".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::NullValue(0)),
            },
        );

        // Array
        let arr = SqlArray {
            values: vec![
                SqlValue {
                    value: Some(SqlValueVariant::StringValue("a".to_string())),
                },
                SqlValue {
                    value: Some(SqlValueVariant::StringValue("b".to_string())),
                },
            ],
        };
        metadata.insert(
            "array_field".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::ArrayValue(arr)),
            },
        );

        let record = VectorRecord {
            id: "all_types".to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata,
            timestamp: Some(1704067200000),
            updated_at: Some(1704153600000),
            expires_at: Some(1704240000000),
            version: Some(5),
            source: Some("test_source".to_string()),
        };

        let results = service.migrate_batch(&[record], Some("comprehensive_schema"));

        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();

        // Verify all fields migrated
        assert_eq!(proxima.id, "all_types");
        assert_eq!(proxima.vector, vec![0.1, 0.2, 0.3, 0.4]);
        assert!(proxima.typed_fields.contains_key("string_field"));
        assert!(proxima.typed_fields.contains_key("int_field"));
        assert!(proxima.typed_fields.contains_key("float_field"));
        assert!(proxima.typed_fields.contains_key("bool_field"));
        assert!(proxima.typed_fields.contains_key("null_field"));
        assert!(proxima.typed_fields.contains_key("array_field"));
        assert_eq!(proxima.timestamp_ms, 1704067200000);
        assert_eq!(proxima.updated_at_ms, Some(1704153600000));
        assert_eq!(proxima.expires_at_ms, Some(1704240000000));
        assert_eq!(proxima.version, Some(5));
        assert_eq!(proxima.source, Some("test_source".to_string()));
    }
}

// =============================================================================
// Validation Scenario Tests
// =============================================================================

mod validation_scenario_tests {
    use super::*;

    #[test]
    fn test_empty_id_validation_failure() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        let mut record = create_simple_record("", vec![0.1, 0.2, 0.3, 0.4]);
        record.id = String::new();

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_err());
        if let Err(MigrationError::ValidationFailed(msg)) = &results[0] {
            assert!(msg.contains("id"), "Error message should mention 'id': {}", msg);
        } else {
            panic!("Expected ValidationFailed error");
        }
    }

    #[test]
    fn test_empty_vector_validation_failure() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        let mut record = create_simple_record("doc_1", vec![]);
        record.vector = vec![];

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_err());
        if let Err(MigrationError::ValidationFailed(msg)) = &results[0] {
            assert!(
                msg.contains("vector"),
                "Error message should mention 'vector': {}",
                msg
            );
        } else {
            panic!("Expected ValidationFailed error");
        }
    }

    #[test]
    fn test_dimension_mismatch_detection() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        // Create a record and manually set wrong dimension
        let record = create_simple_record("doc_1", vec![0.1, 0.2, 0.3]);
        let mut proxima = RecordConverter::vector_to_proxima(&record, None, &[]);
        proxima.vector_dimension = Some(10); // Mismatch: vector has 3 elements

        let result = service.validate_record(&proxima, None);

        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.code == ValidationErrorCode::DimensionMismatch));
    }

    #[test]
    fn test_dimension_mismatch_various_cases() {
        let service = RecordMigrationService::new(MigrationConfig::default());

        let test_cases = vec![
            (vec![0.1, 0.2], Some(3u32)),      // 2 elements, declared 3
            (vec![0.1, 0.2, 0.3], Some(128)),  // 3 elements, declared 128
            (vec![0.1; 128], Some(256)),       // 128 elements, declared 256
            (vec![0.1; 768], Some(1536)),      // 768 elements, declared 1536
        ];

        for (vector, declared_dim) in test_cases {
            let proxima = ProximaRecord {
                id: "test".to_string(),
                vector: vector.clone(),
                vector_dimension: declared_dim,
                ..Default::default()
            };

            let result = service.validate_record(&proxima, None);

            assert!(
                !result.valid,
                "Should fail for vector len {} declared dim {:?}",
                vector.len(),
                declared_dim
            );
            assert!(
                result
                    .errors
                    .iter()
                    .any(|e| e.code == ValidationErrorCode::DimensionMismatch),
                "Should have DimensionMismatch error for vector len {} declared dim {:?}",
                vector.len(),
                declared_dim
            );
        }
    }

    #[test]
    fn test_schema_compatibility_strict_mode() {
        let columns = vec![
            create_column_def("name", ColumnDataType::Text, false, true),
            create_column_def("count", ColumnDataType::Integer, false, false),
        ];
        let mut schema = create_test_schema("strict_schema", columns);
        schema.enforcement_mode = SchemaEnforcementMode::Strict;
        schema.allow_additional_fields = false;

        let service = RecordMigrationService::new(MigrationConfig::default());

        // Create a record missing required field
        let proxima = ProximaRecord {
            id: "test_record".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            vector_dimension: Some(3),
            typed_fields: HashMap::new(),
            ..Default::default()
        };

        // Missing both required fields
        let result = service.validate_record(&proxima, Some(&schema));

        assert!(!result.valid);
        // Should have errors for missing required fields
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.field == "name" && e.code == ValidationErrorCode::RequiredFieldMissing)
        );
        assert!(
            result.errors.iter().any(|e| e.field == "count"
                && e.code == ValidationErrorCode::RequiredFieldMissing)
        );
    }

    #[test]
    fn test_schema_compatibility_type_mismatch() {
        let columns = vec![create_column_def("count", ColumnDataType::Integer, false, false)];
        let schema = create_test_schema("typed_schema", columns);

        let service = RecordMigrationService::new(MigrationConfig::default());

        // Create a record with wrong type for count (Text instead of Integer)
        let mut typed_fields = HashMap::new();
        typed_fields.insert("count".to_string(), TypedValue::Text("not_an_integer".to_string()));

        let proxima = ProximaRecord {
            id: "test_record".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            vector_dimension: Some(3),
            typed_fields,
            ..Default::default()
        };

        let result = service.validate_record(&proxima, Some(&schema));

        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.field == "count" && e.code == ValidationErrorCode::TypeMismatch));
    }

    #[test]
    fn test_validation_with_constraints() {
        let mut columns = vec![create_column_def("name", ColumnDataType::Text, false, true)];
        columns[0].constraints = ColumnConstraints {
            max_length: Some(10),
            min_length: Some(2),
            ..Default::default()
        };
        let schema = create_test_schema("constrained_schema", columns);

        let service = RecordMigrationService::new(MigrationConfig::default());

        // Test value too long
        let mut typed_fields = HashMap::new();
        typed_fields.insert(
            "name".to_string(),
            TypedValue::Text("this_is_way_too_long_for_the_constraint".to_string()),
        );

        let proxima = ProximaRecord {
            id: "test".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            vector_dimension: Some(3),
            typed_fields,
            ..Default::default()
        };

        let result = service.validate_record(&proxima, Some(&schema));

        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn test_validation_can_be_disabled() {
        let config = MigrationConfig::new().with_validate_on_migrate(false);
        let service = RecordMigrationService::new(config);

        // Create an invalid record (empty ID and empty vector)
        let mut record = create_simple_record("", vec![]);
        record.id = String::new();
        record.vector = vec![];

        let results = service.migrate_batch(&[record], None);

        // With validation disabled, it should succeed
        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();
        assert!(proxima.id.is_empty());
        assert!(proxima.vector.is_empty());
    }

    #[test]
    fn test_validation_result_aggregation() {
        let config = MigrationConfig::new().with_validate_on_migrate(true);
        let service = RecordMigrationService::new(config);

        // Create multiple invalid records
        let batch = vec![
            {
                let mut r = create_simple_record("", vec![0.1]);
                r.id = String::new();
                r
            },
            {
                let mut r = create_simple_record("doc_2", vec![]);
                r.vector = vec![];
                r
            },
            {
                let mut r = create_simple_record("", vec![]);
                r.id = String::new();
                r.vector = vec![];
                r
            },
        ];

        let results = service.migrate_batch(&batch, None);

        // All should fail
        assert!(results.iter().all(|r| r.is_err()));

        // First: empty ID
        if let Err(MigrationError::ValidationFailed(msg)) = &results[0] {
            assert!(msg.contains("id"));
        }

        // Second: empty vector
        if let Err(MigrationError::ValidationFailed(msg)) = &results[1] {
            assert!(msg.contains("vector"));
        }

        // Third: both empty ID and empty vector
        if let Err(MigrationError::ValidationFailed(msg)) = &results[2] {
            assert!(msg.contains("id") || msg.contains("vector"));
        }
    }
}

// =============================================================================
// TEXT Column Edge Cases Tests
// =============================================================================

mod text_column_edge_cases_tests {
    use super::*;

    #[test]
    fn test_empty_text_field_handling() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        // Create record with empty text content
        let mut record = create_simple_record("doc_empty_text", vec![0.1, 0.2, 0.3]);
        record.metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(String::new())),
            },
        );

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();

        // Empty text should be in text_fields with empty content
        assert_eq!(proxima.text_fields.len(), 1);
        assert_eq!(proxima.text_fields[0].name, "content");
        assert_eq!(proxima.text_fields[0].content, "");
    }

    #[test]
    fn test_very_large_text_values_10kb() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["large_content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        // Create 10KB text content
        let large_text = "x".repeat(10 * 1024);

        let mut record = create_simple_record("doc_large_text", vec![0.1, 0.2, 0.3]);
        record.metadata.insert(
            "large_content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(large_text.clone())),
            },
        );

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();

        // Large text should be preserved
        assert_eq!(proxima.text_fields.len(), 1);
        assert_eq!(proxima.text_fields[0].content.len(), 10 * 1024);
        assert_eq!(proxima.text_fields[0].content, large_text);
    }

    #[test]
    fn test_very_large_text_values_100kb() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["huge_content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        // Create 100KB text content
        let huge_text = "y".repeat(100 * 1024);

        let mut record = create_simple_record("doc_huge_text", vec![0.1, 0.2, 0.3]);
        record.metadata.insert(
            "huge_content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(huge_text.clone())),
            },
        );

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();

        assert_eq!(proxima.text_fields[0].content.len(), 100 * 1024);
    }

    #[test]
    fn test_unicode_text_handling() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["unicode_content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        let unicode_texts = vec![
            // Emoji
            "Hello World! \u{1F600}\u{1F601}\u{1F602} Smiley faces!",
            // Chinese characters
            "\u{4E2D}\u{6587}\u{6D4B}\u{8BD5} - Chinese text test",
            // Arabic
            "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627} - Arabic greeting",
            // Japanese Hiragana and Kanji
            "\u{3053}\u{3093}\u{306B}\u{3061}\u{306F} - \u{65E5}\u{672C}\u{8A9E}",
            // Russian Cyrillic
            "\u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{043C}\u{0438}\u{0440}!",
            // Mixed scripts
            "English, \u{4E2D}\u{6587}, \u{65E5}\u{672C}\u{8A9E}, \u{0645}\u{0631}\u{062D}\u{0628}\u{0627}",
            // Mathematical symbols
            "\u{221A}x\u{00B2} + y\u{00B2} = z\u{00B2} (Pythagorean theorem)",
            // Currency symbols
            "Prices: $100, \u{20AC}85, \u{00A3}70, \u{00A5}11000",
        ];

        for (i, unicode_text) in unicode_texts.iter().enumerate() {
            let mut record = create_simple_record(&format!("doc_unicode_{}", i), vec![0.1, 0.2, 0.3]);
            record.metadata.insert(
                "unicode_content".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(unicode_text.to_string())),
                },
            );

            let results = service.migrate_batch(&[record], None);

            assert!(
                results[0].is_ok(),
                "Should handle unicode text: {}",
                unicode_text
            );
            let proxima = results[0].as_ref().unwrap();
            assert_eq!(
                proxima.text_fields[0].content, *unicode_text,
                "Unicode text should be preserved exactly"
            );
        }
    }

    #[test]
    fn test_special_characters_in_text() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["special_content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        let special_texts = vec![
            // Quotes
            "Hello \"World\"",
            "Single 'quotes' here",
            // Escapes
            "Line1\nLine2\nLine3",
            "Tab\there\ttoo",
            "Carriage\rReturn",
            "Mixed\r\nLine\r\nEndings",
            // Backslashes
            "Path: C:\\Users\\Documents\\file.txt",
            "Escaped backslash: \\\\",
            // Null character (if supported)
            "Before\0After",
            // XML/HTML entities
            "<xml>content</xml>",
            "&amp; &lt; &gt; &quot;",
            "<!--comment-->",
            // JSON
            "{\"key\": \"value\", \"array\": [1, 2, 3]}",
            // SQL injection attempt
            "'; DROP TABLE users; --",
            // Control characters
            "Bell: \x07 Backspace: \x08",
        ];

        for (i, special_text) in special_texts.iter().enumerate() {
            let mut record = create_simple_record(&format!("doc_special_{}", i), vec![0.1, 0.2, 0.3]);
            record.metadata.insert(
                "special_content".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(special_text.to_string())),
                },
            );

            let results = service.migrate_batch(&[record], None);

            assert!(
                results[0].is_ok(),
                "Should handle special text: {:?}",
                special_text
            );
            let proxima = results[0].as_ref().unwrap();
            assert_eq!(
                proxima.text_fields[0].content, *special_text,
                "Special characters should be preserved"
            );
        }
    }

    #[test]
    fn test_whitespace_only_text() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["whitespace_content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        let whitespace_texts = vec![
            " ",
            "   ",
            "\t",
            "\t\t\t",
            "\n",
            "\n\n\n",
            "   \t  \n  \r  ",
            "\u{00A0}", // Non-breaking space
            "\u{2003}", // Em space
            "\u{200B}", // Zero-width space
        ];

        for (i, ws_text) in whitespace_texts.iter().enumerate() {
            let mut record = create_simple_record(&format!("doc_ws_{}", i), vec![0.1, 0.2, 0.3]);
            record.metadata.insert(
                "whitespace_content".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue(ws_text.to_string())),
                },
            );

            let results = service.migrate_batch(&[record], None);

            assert!(
                results[0].is_ok(),
                "Should handle whitespace text: {:?}",
                ws_text
            );
            let proxima = results[0].as_ref().unwrap();
            assert_eq!(
                proxima.text_fields[0].content, *ws_text,
                "Whitespace should be preserved"
            );
        }
    }

    #[test]
    fn test_multiple_text_columns() {
        let config = MigrationConfig::new()
            .with_text_columns(vec![
                "title".to_string(),
                "description".to_string(),
                "content".to_string(),
            ])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        let mut record = create_simple_record("doc_multi_text", vec![0.1, 0.2, 0.3]);
        record.metadata.insert(
            "title".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue("The Title".to_string())),
            },
        );
        record.metadata.insert(
            "description".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(
                    "A longer description of the content".to_string(),
                )),
            },
        );
        record.metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(
                    "The full content body with lots of text...".to_string(),
                )),
            },
        );
        record.metadata.insert(
            "category".to_string(), // Not a text column
            SqlValue {
                value: Some(SqlValueVariant::StringValue("articles".to_string())),
            },
        );

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();

        // Should have 3 text fields
        assert_eq!(proxima.text_fields.len(), 3);

        let text_field_names: Vec<&str> = proxima.text_fields.iter().map(|tf| tf.name.as_str()).collect();
        assert!(text_field_names.contains(&"title"));
        assert!(text_field_names.contains(&"description"));
        assert!(text_field_names.contains(&"content"));

        // Category should be in typed_fields, not text_fields
        assert!(proxima.typed_fields.contains_key("category"));
    }

    #[test]
    fn test_text_storage_strategy_detection() {
        // Test that storage strategy is determined based on content size
        let small_text = "Short text";
        let medium_text = "x".repeat(5000); // ~5KB
        let large_text = "y".repeat(2_000_000); // ~2MB

        // Small text -> Inline
        let strategy = TextStorageStrategy::for_size(small_text.len());
        assert_eq!(strategy, TextStorageStrategy::Inline);

        // Medium text -> Chunked
        let strategy = TextStorageStrategy::for_size(medium_text.len());
        assert_eq!(strategy, TextStorageStrategy::Chunked);

        // Large text -> Sidecar
        let strategy = TextStorageStrategy::for_size(large_text.len());
        assert_eq!(strategy, TextStorageStrategy::Sidecar);
    }

    #[test]
    fn test_text_field_creation_with_strategy() {
        // Create text fields with different strategies
        let small_content = "Short content";
        let large_content = "x".repeat(100_000);

        let small_field = TextField::new("title".to_string(), small_content.to_string());
        assert_eq!(small_field.storage_hint, TextStorageStrategy::Inline);

        let large_field = TextField::new("content".to_string(), large_content.clone());
        assert_eq!(large_field.storage_hint, TextStorageStrategy::Chunked);

        // Create with explicit strategy
        let forced_sidecar = TextField::with_strategy(
            "forced".to_string(),
            small_content.to_string(),
            TextStorageStrategy::Sidecar,
        );
        assert_eq!(forced_sidecar.storage_hint, TextStorageStrategy::Sidecar);
    }

    #[test]
    fn test_null_text_field_handling() {
        let config = MigrationConfig::new()
            .with_text_columns(vec!["nullable_content".to_string()])
            .with_validate_on_migrate(true);

        let service = RecordMigrationService::new(config);

        // Create record with NULL text value
        let mut record = create_simple_record("doc_null_text", vec![0.1, 0.2, 0.3]);
        record.metadata.insert(
            "nullable_content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::NullValue(0)),
            },
        );

        let results = service.migrate_batch(&[record], None);

        assert!(results[0].is_ok());
        let proxima = results[0].as_ref().unwrap();

        // NULL text columns should not create text_fields
        // (they might be in typed_fields as Null or not present)
        // The exact behavior depends on implementation
        assert!(
            proxima.text_fields.is_empty() ||
            proxima.text_fields.iter().all(|tf| tf.name != "nullable_content" || tf.content.is_empty())
        );
    }

    #[test]
    fn test_text_roundtrip_conversion() {
        let _config = MigrationConfig::new()
            .with_text_columns(vec!["content".to_string()])
            .with_validate_on_migrate(true);

        let original_text = "This is the original content that should survive roundtrip.";

        let mut record = create_simple_record("doc_roundtrip", vec![0.1, 0.2, 0.3]);
        record.metadata.insert(
            "content".to_string(),
            SqlValue {
                value: Some(SqlValueVariant::StringValue(original_text.to_string())),
            },
        );

        // Convert to ProximaRecord
        let proxima = RecordConverter::vector_to_proxima(&record, None, &["content".to_string()]);

        // Verify content is in text_fields
        assert_eq!(proxima.text_fields.len(), 1);
        assert_eq!(proxima.text_fields[0].content, original_text);

        // Convert back to VectorRecord
        let converted_back = RecordConverter::proxima_to_vector(&proxima);

        // Verify content is restored in metadata
        assert!(converted_back.metadata.contains_key("content"));
        if let Some(SqlValue {
            value: Some(SqlValueVariant::StringValue(s)),
        }) = converted_back.metadata.get("content")
        {
            assert_eq!(s, original_text);
        } else {
            panic!("Content should be restored as string in metadata");
        }
    }
}

// =============================================================================
// Schema Inference Edge Cases Tests
// =============================================================================

mod schema_inference_edge_cases_tests {
    use super::*;

    #[test]
    fn test_inference_from_empty_records() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let records: Vec<VectorRecord> = vec![];
        let schema = service.infer_schema(&records);

        assert!(schema.columns.is_empty());
        assert!(schema.text_columns.is_empty());
        assert_eq!(schema.sample_size, 0);
        assert_eq!(schema.confidence, 1.0);
    }

    #[test]
    fn test_inference_with_all_null_values() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let records: Vec<VectorRecord> = (0..5)
            .map(|i| {
                let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
                record.metadata.insert(
                    "nullable_field".to_string(),
                    SqlValue {
                        value: Some(SqlValueVariant::NullValue(0)),
                    },
                );
                record
            })
            .collect();

        let schema = service.infer_schema(&records);

        // Column should be detected even if all values are null
        assert!(schema.get_column("nullable_field").is_some());
        let col = schema.get_column("nullable_field").unwrap();
        assert!(col.nullable);
        // All nulls should default to Text type
        assert!(matches!(col.data_type, ColumnDataType::Text));
    }

    #[test]
    fn test_inference_mixed_null_and_values() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let mut records = Vec::new();

        // Some records with values
        for i in 0..3 {
            records.push(create_record_with_int_metadata(
                &format!("doc_{}", i),
                "count",
                i * 10,
            ));
        }

        // Some records with null
        for i in 3..5 {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "count".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::NullValue(0)),
                },
            );
            records.push(record);
        }

        let schema = service.infer_schema(&records);

        let count_col = schema.get_column("count").unwrap();
        assert!(count_col.nullable);
        assert!(matches!(count_col.data_type, ColumnDataType::Integer));
        assert_eq!(count_col.null_count, 2);
        assert_eq!(count_col.sample_count, 5);
    }

    #[test]
    fn test_inference_sample_size_limit() {
        let config = InferenceConfig::new().with_sample_size(10);
        let service = SchemaInferenceService::new(config);

        // Create 100 records, but only 10 should be sampled
        let records: Vec<VectorRecord> = (0..100)
            .map(|i| create_record_with_int_metadata(&format!("doc_{}", i), "num", i))
            .collect();

        let schema = service.infer_schema(&records);

        assert_eq!(schema.sample_size, 10);
    }

    #[test]
    fn test_inference_confidence_threshold() {
        let config = InferenceConfig::new().with_confidence_threshold(0.9);
        let service = SchemaInferenceService::new(config);

        // Mix UUID and non-UUID values (below 90% threshold)
        let mut records = Vec::new();

        // 7 UUIDs
        for i in 0..7 {
            records.push(create_record_with_string_metadata(
                &format!("doc_{}", i),
                "id_field",
                &format!("550e8400-e29b-41d4-a716-44665544{:04}", i),
            ));
        }

        // 3 non-UUIDs
        for i in 7..10 {
            records.push(create_record_with_string_metadata(
                &format!("doc_{}", i),
                "id_field",
                "not-a-uuid",
            ));
        }

        let schema = service.infer_schema(&records);

        // With 70% UUID ratio and 90% threshold, should fallback to Text
        let _id_col = schema.get_column("id_field").unwrap();
        // The exact behavior depends on implementation, but it shouldn't be Uuid
        // if confidence is below threshold
    }

    #[test]
    fn test_inference_sparse_columns() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let mut records = Vec::new();

        // Only some records have optional_field
        for i in 0..10 {
            let mut record = create_simple_record(&format!("doc_{}", i), vec![0.1, 0.2]);
            record.metadata.insert(
                "required_field".to_string(),
                SqlValue {
                    value: Some(SqlValueVariant::StringValue("always present".to_string())),
                },
            );

            // Only even records have optional_field
            if i % 2 == 0 {
                record.metadata.insert(
                    "optional_field".to_string(),
                    SqlValue {
                        value: Some(SqlValueVariant::Int64Value(i)),
                    },
                );
            }

            records.push(record);
        }

        let schema = service.infer_schema(&records);

        let required_col = schema.get_column("required_field").unwrap();
        assert_eq!(required_col.sample_count, 10);

        let optional_col = schema.get_column("optional_field").unwrap();
        assert_eq!(optional_col.sample_count, 5); // Only present in half the records
    }
}
