/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Integration tests for TEXT column storage and filtering
//!
//! Tests the complete TEXT column feature set:
//! - Storage strategies (Inline, Chunked, Sidecar, Adaptive)
//! - TEXT filtering operations (Equals, Contains, StartsWith, EndsWith, etc.)
//! - N-gram bloom filter optimization for CONTAINS queries
//! - SST engine integration with TEXT columns
//! - TEXT column schema definitions and type detection

#[cfg(test)]
mod text_column_integration_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use proximadb::core::types::{ColumnDataType, TextStorageStrategy};
    use proximadb::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value::Value};
    use proximadb::storage::engines::core::formats::columnar::{
        CHUNKED_THRESHOLD, INLINE_THRESHOLD, TextColumnFilterEvaluator, TextColumnReader,
        TextColumnWriter, TextComparisonOp, TextStorageConfig,
    };
    use proximadb::storage::engines::sst::text_column_support::{
        SstTextColumnProcessor, SstTextColumnReader, SstTextFilterEvaluator, SstTextSupportBuilder,
        TextColumnDefinition,
    };

    // ==================== Helper Functions ====================

    /// Create a test VectorRecord with TEXT metadata
    fn create_record_with_text(id: &str, column_name: &str, text_value: &str) -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert(
            column_name.to_string(),
            SqlValue {
                value: Some(Value::StringValue(text_value.to_string())),
            },
        );

        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    /// Create a test VectorRecord with NULL TEXT value
    fn create_record_with_null_text(id: &str, column_name: &str) -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert(
            column_name.to_string(),
            SqlValue {
                value: Some(Value::NullValue(0)),
            },
        );

        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    /// Create a test VectorRecord with multiple TEXT columns
    fn create_record_with_multi_text(id: &str, columns: Vec<(&str, &str)>) -> VectorRecord {
        let mut metadata = HashMap::new();
        for (col_name, text_value) in columns {
            metadata.insert(
                col_name.to_string(),
                SqlValue {
                    value: Some(Value::StringValue(text_value.to_string())),
                },
            );
        }

        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    // ==================== Storage Strategy Tests ====================

    mod storage_strategy_tests {
        use super::*;

        #[test]
        fn test_inline_storage_for_small_text() {
            // Test that small text (<4KB) is stored inline
            let config = TextStorageConfig::for_small_text();
            let mut writer = TextColumnWriter::new(config);

            // Write small texts
            writer.write("rec1", "Hello World").unwrap();
            writer.write("rec2", "Short description").unwrap();
            writer.write("rec3", "Another small text").unwrap();

            let stats = writer.stats();
            assert_eq!(stats.inline_count, 3);
            assert_eq!(stats.chunked_count, 0);
            assert_eq!(stats.sidecar_count, 0);
            assert_eq!(stats.total_records, 3);

            // Build Arrow array and verify
            let array = writer.build_inline_array();
            assert_eq!(array.len(), 3);
        }

        #[test]
        fn test_adaptive_storage_strategy_selection() {
            // Test that adaptive strategy selects appropriate storage based on size
            let config = TextStorageConfig::default();
            assert_eq!(config.strategy, TextStorageStrategy::Adaptive);

            let mut writer = TextColumnWriter::new(config);

            // Small text -> Inline
            let small_text = "Small text under 4KB";
            writer.write("small", small_text).unwrap();

            // Medium text -> Chunked (between 4KB and 1MB)
            let medium_text = "x".repeat(INLINE_THRESHOLD + 1000);
            writer.write("medium", &medium_text).unwrap();

            let stats = writer.stats();
            assert_eq!(stats.inline_count, 1, "Small text should be inline");
            assert_eq!(stats.chunked_count, 1, "Medium text should be chunked");
        }

        #[test]
        fn test_chunked_storage_creates_chunks() {
            // Test that chunked storage creates multiple chunks
            let mut config = TextStorageConfig::default();
            config.strategy = TextStorageStrategy::Chunked;
            config.chunk_size = 100; // Small chunk size for testing

            let mut writer = TextColumnWriter::new(config);

            // Write text that will be split into chunks
            let text = "The quick brown fox jumps over the lazy dog. ".repeat(10);
            writer.write("doc1", &text).unwrap();

            let chunks = writer.get_chunks();
            assert!(!chunks.is_empty(), "Should have created chunks");
            assert!(chunks.len() > 1, "Should have multiple chunks");

            // Verify chunk properties
            for (i, chunk) in chunks.iter().enumerate() {
                assert_eq!(chunk.parent_id, "doc1");
                assert_eq!(chunk.chunk_index, i as u32);
                assert!(!chunk.content.is_empty());
            }
        }

        #[test]
        fn test_sidecar_storage_reference() {
            // Test that sidecar storage creates references
            let config = TextStorageConfig::for_large_documents("/tmp/sidecars".to_string());

            let mut writer = TextColumnWriter::new(config);

            // Write large text
            let large_text = "x".repeat(CHUNKED_THRESHOLD + 1000);
            writer.write("large_doc", &large_text).unwrap();

            let stats = writer.stats();
            assert_eq!(stats.sidecar_count, 1);

            let sidecar_refs = writer.get_sidecar_refs();
            assert_eq!(sidecar_refs.len(), 1);
            assert_eq!(sidecar_refs[0].record_id, "large_doc");
            assert!(sidecar_refs[0].sidecar_path.contains("/tmp/sidecars"));
        }

        #[test]
        fn test_max_text_size_enforcement() {
            // Test that max_text_size is enforced
            let mut config = TextStorageConfig::default();
            config.max_text_size = 100;

            let mut writer = TextColumnWriter::new(config);

            // Write text exceeding limit
            let large_text = "x".repeat(200);
            let result = writer.write("too_large", &large_text);

            assert!(result.is_err());
        }

        #[test]
        fn test_null_value_handling() {
            // Test NULL value handling
            let config = TextStorageConfig::default();
            let mut writer = TextColumnWriter::new(config);

            writer.write("rec1", "Not null").unwrap();
            writer.write_null("rec2");
            writer.write("rec3", "Also not null").unwrap();

            let stats = writer.stats();
            assert_eq!(stats.total_records, 3);
            assert_eq!(stats.inline_count, 2); // Only non-null values counted

            let array = writer.build_inline_array();
            assert_eq!(array.len(), 3);
            assert!(!array.is_null(0));
            assert!(array.is_null(1)); // NULL value
            assert!(!array.is_null(2));
        }
    }

    // ==================== Filter Operation Tests ====================

    mod filter_operation_tests {
        use super::*;

        #[test]
        fn test_equals_filter() {
            let evaluator = TextColumnFilterEvaluator::new("title".to_string());

            let values = vec![
                Some("Hello World".to_string()),
                Some("Goodbye World".to_string()),
                Some("Hello World".to_string()),
                None,
            ];

            let matches = evaluator.evaluate(&TextComparisonOp::Equals, "Hello World", &values);
            assert_eq!(matches, vec![0, 2]);
        }

        #[test]
        fn test_not_equals_filter() {
            let evaluator = TextColumnFilterEvaluator::new("title".to_string());

            let values = vec![
                Some("Hello".to_string()),
                Some("World".to_string()),
                Some("Hello".to_string()),
            ];

            let matches = evaluator.evaluate(&TextComparisonOp::NotEquals, "Hello", &values);
            assert_eq!(matches, vec![1]);
        }

        #[test]
        fn test_contains_filter() {
            let evaluator = TextColumnFilterEvaluator::new("description".to_string());

            let values = vec![
                Some("The quick brown fox".to_string()),
                Some("A lazy dog".to_string()),
                Some("quick search results".to_string()),
                Some("No match here".to_string()),
            ];

            // Contains is case-sensitive
            let matches = evaluator.evaluate(&TextComparisonOp::Contains, "quick", &values);
            assert_eq!(matches, vec![0, 2]);
        }

        #[test]
        fn test_starts_with_filter() {
            let evaluator = TextColumnFilterEvaluator::new("title".to_string());

            let values = vec![
                Some("Hello World".to_string()),
                Some("World Hello".to_string()),
                Some("Hello There".to_string()),
            ];

            let matches = evaluator.evaluate(&TextComparisonOp::StartsWith, "Hello", &values);
            assert_eq!(matches, vec![0, 2]);
        }

        #[test]
        fn test_ends_with_filter() {
            let evaluator = TextColumnFilterEvaluator::new("title".to_string());

            let values = vec![
                Some("Hello World".to_string()),
                Some("Brave New World".to_string()),
                Some("Hello There".to_string()),
            ];

            let matches = evaluator.evaluate(&TextComparisonOp::EndsWith, "World", &values);
            assert_eq!(matches, vec![0, 1]);
        }

        #[test]
        fn test_is_null_filter() {
            let evaluator = TextColumnFilterEvaluator::new("description".to_string());

            let values = vec![
                Some("Not null".to_string()),
                None,
                Some("Also not null".to_string()),
                None,
            ];

            let matches = evaluator.evaluate(&TextComparisonOp::IsNull, "", &values);
            assert_eq!(matches, vec![1, 3]);
        }

        #[test]
        fn test_is_not_null_filter() {
            let evaluator = TextColumnFilterEvaluator::new("description".to_string());

            let values = vec![
                Some("Not null".to_string()),
                None,
                Some("Also not null".to_string()),
                None,
            ];

            let matches = evaluator.evaluate(&TextComparisonOp::IsNotNull, "", &values);
            assert_eq!(matches, vec![0, 2]);
        }

        #[test]
        fn test_empty_values_handling() {
            let evaluator = TextColumnFilterEvaluator::new("content".to_string());

            let values: Vec<Option<String>> = vec![];

            let matches = evaluator.evaluate(&TextComparisonOp::Contains, "test", &values);
            assert!(matches.is_empty());
        }
    }

    // ==================== SST Integration Tests ====================

    mod sst_integration_tests {
        use super::*;

        #[test]
        fn test_sst_text_processor_registration() {
            let mut processor = SstTextColumnProcessor::new();

            // Register TEXT columns
            processor.register_text_column(TextColumnDefinition {
                name: "title".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: false,
                ngram_size: 3,
            });

            processor.register_text_column(TextColumnDefinition {
                name: "description".to_string(),
                storage_strategy: TextStorageStrategy::Adaptive,
                enable_ngram_bloom: true,
                ngram_size: 3,
            });

            assert!(processor.has_text_columns());
            let columns: Vec<&str> = processor.text_column_names();
            assert!(columns.contains(&"title"));
            assert!(columns.contains(&"description"));
        }

        #[test]
        fn test_sst_text_processor_batch_processing() {
            let mut processor = SstTextColumnProcessor::new();

            processor.register_text_column(TextColumnDefinition {
                name: "content".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: false,
                ngram_size: 3,
            });

            // Create test records
            let records = vec![
                create_record_with_text("rec1", "content", "First content"),
                create_record_with_text("rec2", "content", "Second content"),
                create_record_with_text("rec3", "content", "Third content"),
            ];

            let result = processor.process_batch(&records).unwrap();

            // Verify statistics
            let stats = result.stats.get("content").unwrap();
            assert_eq!(stats.inline_count, 3);
            assert_eq!(stats.chunked_count, 0);
            assert_eq!(stats.sidecar_count, 0);
        }

        #[test]
        fn test_sst_text_processor_with_null_values() {
            let mut processor = SstTextColumnProcessor::new();

            processor.register_text_column(TextColumnDefinition {
                name: "description".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: false,
                ngram_size: 3,
            });

            // Create records with mixed null/non-null values
            let records = vec![
                create_record_with_text("rec1", "description", "Has value"),
                create_record_with_null_text("rec2", "description"),
                create_record_with_text("rec3", "description", "Also has value"),
            ];

            let result = processor.process_batch(&records).unwrap();

            let stats = result.stats.get("description").unwrap();
            assert_eq!(stats.inline_count, 2); // Only non-null values
        }

        #[test]
        fn test_sst_text_filter_evaluator_integration() {
            let mut evaluator = SstTextFilterEvaluator::new();
            evaluator.register_column("title".to_string());
            evaluator.register_column("description".to_string());

            let title_values = vec![
                Some("Product A".to_string()),
                Some("Product B".to_string()),
                Some("Service A".to_string()),
            ];

            // Filter by title
            let matches = evaluator
                .evaluate(
                    "title",
                    &TextComparisonOp::StartsWith,
                    "Product",
                    &title_values,
                )
                .unwrap();
            assert_eq!(matches, vec![0, 1]);

            let description_values = vec![
                Some("High quality product".to_string()),
                Some("Budget option".to_string()),
                Some("Premium quality service".to_string()),
            ];

            // Filter by description
            let matches = evaluator
                .evaluate(
                    "description",
                    &TextComparisonOp::Contains,
                    "quality",
                    &description_values,
                )
                .unwrap();
            assert_eq!(matches, vec![0, 2]);
        }

        #[test]
        fn test_sst_text_reader_extract_values() {
            let reader = SstTextColumnReader::default();

            let records = vec![
                create_record_with_text("rec1", "title", "First Title"),
                create_record_with_text("rec2", "title", "Second Title"),
                create_record_with_null_text("rec3", "title"),
            ];

            let values = reader.extract_text_values(&records, "title");

            assert_eq!(values.len(), 3);
            assert_eq!(values[0], Some("First Title".to_string()));
            assert_eq!(values[1], Some("Second Title".to_string()));
            assert_eq!(values[2], None);
        }

        #[test]
        fn test_sst_text_support_builder() {
            // Test complete TEXT support builder
            let support = SstTextSupportBuilder::new()
                .with_processor(TextStorageConfig::default())
                .with_filter_evaluator()
                .with_reader(TextStorageConfig::default())
                .build();

            assert!(support.has_processor());
            assert!(support.has_filter_evaluator());
            assert!(support.has_reader());
        }

        #[test]
        fn test_sst_text_support_default_all() {
            let support = proximadb::storage::engines::sst::text_column_support::SstTextSupport::default_all();

            assert!(support.has_processor());
            assert!(support.has_filter_evaluator());
            assert!(support.has_reader());
        }
    }

    // ==================== Schema Integration Tests ====================

    mod schema_integration_tests {
        use super::*;

        #[test]
        fn test_column_data_type_text_detection() {
            // Test TEXT type detection
            assert!(ColumnDataType::Text.is_text());
            assert!(ColumnDataType::TextLarge.is_text());
            assert!(!ColumnDataType::Integer.is_text());
            assert!(!ColumnDataType::Float.is_text());
            assert!(!ColumnDataType::Boolean.is_text());
        }

        #[test]
        fn test_text_column_registration_from_schema() {
            let mut processor = SstTextColumnProcessor::new();

            // Simulate schema with mixed column types
            let schema_columns: Vec<(String, ColumnDataType)> = vec![
                ("id".to_string(), ColumnDataType::Uuid),
                ("content".to_string(), ColumnDataType::Text),
                ("large_doc".to_string(), ColumnDataType::TextLarge),
                ("count".to_string(), ColumnDataType::Integer),
            ];

            processor.register_from_schema(&schema_columns);

            // Only TEXT columns should be registered
            let text_columns = processor.text_column_names();
            assert_eq!(text_columns.len(), 2);
            assert!(text_columns.contains(&"content"));
            assert!(text_columns.contains(&"large_doc"));
        }

        #[test]
        fn test_text_storage_config_presets() {
            // Test for_small_text preset
            let small_config = TextStorageConfig::for_small_text();
            assert_eq!(small_config.strategy, TextStorageStrategy::Inline);

            // Test for_rag_documents preset
            let rag_config = TextStorageConfig::for_rag_documents(256);
            assert_eq!(rag_config.strategy, TextStorageStrategy::Chunked);
            assert_eq!(rag_config.chunk_size, 256);
            assert!(rag_config.enable_ngram_bloom);

            // Test for_large_documents preset
            let large_config = TextStorageConfig::for_large_documents("/data/sidecars".to_string());
            assert_eq!(large_config.strategy, TextStorageStrategy::Sidecar);
            assert_eq!(
                large_config.sidecar_base_path,
                Some("/data/sidecars".to_string())
            );
        }
    }

    // ==================== Multi-Column Tests ====================

    mod multi_column_tests {
        use super::*;

        #[test]
        fn test_multiple_text_columns_in_record() {
            let mut processor = SstTextColumnProcessor::new();

            processor.register_text_column(TextColumnDefinition {
                name: "title".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: false,
                ngram_size: 3,
            });

            processor.register_text_column(TextColumnDefinition {
                name: "description".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: true,
                ngram_size: 3,
            });

            let records = vec![create_record_with_multi_text(
                "rec1",
                vec![
                    ("title", "Product Name"),
                    ("description", "Product description text"),
                ],
            )];

            let result = processor.process_batch(&records).unwrap();

            assert!(result.stats.contains_key("title"));
            assert!(result.stats.contains_key("description"));

            let title_stats = result.stats.get("title").unwrap();
            assert_eq!(title_stats.inline_count, 1);

            let desc_stats = result.stats.get("description").unwrap();
            assert_eq!(desc_stats.inline_count, 1);
        }

        #[test]
        fn test_filtering_multiple_text_columns() {
            let mut evaluator = SstTextFilterEvaluator::new();
            evaluator.register_column("title".to_string());
            evaluator.register_column("category".to_string());

            let title_values = vec![
                Some("Electronics Sale".to_string()),
                Some("Clothing Clearance".to_string()),
                Some("Electronics New Arrival".to_string()),
            ];

            let category_values = vec![
                Some("Electronics".to_string()),
                Some("Clothing".to_string()),
                Some("Electronics".to_string()),
            ];

            // Filter by title
            let title_matches = evaluator
                .evaluate(
                    "title",
                    &TextComparisonOp::Contains,
                    "Electronics",
                    &title_values,
                )
                .unwrap();

            // Filter by category
            let category_matches = evaluator
                .evaluate(
                    "category",
                    &TextComparisonOp::Equals,
                    "Electronics",
                    &category_values,
                )
                .unwrap();

            // Both should match records 0 and 2
            assert_eq!(title_matches, vec![0, 2]);
            assert_eq!(category_matches, vec![0, 2]);
        }
    }

    // ==================== Reader Tests ====================

    mod reader_tests {
        use super::*;
        use arrow::array::{ArrayRef, StringArray};

        #[test]
        fn test_text_column_reader_from_arrow_array() {
            let config = TextStorageConfig::default();
            let reader = TextColumnReader::new(config);

            // Create Arrow array
            let string_array: StringArray =
                vec![Some("First"), Some("Second"), None, Some("Fourth")].into();
            let array_ref: ArrayRef = Arc::new(string_array);

            let values = reader.load_from_array(&array_ref).unwrap();

            assert_eq!(values.len(), 4);
            assert_eq!(values[0], Some("First".to_string()));
            assert_eq!(values[1], Some("Second".to_string()));
            assert_eq!(values[2], None);
            assert_eq!(values[3], Some("Fourth".to_string()));
        }

        #[test]
        fn test_text_column_reader_cache_size() {
            let config = TextStorageConfig::default();
            let reader = TextColumnReader::new(config).with_max_cache(50 * 1024 * 1024); // 50MB

            assert_eq!(reader.cache_size(), 0);
        }
    }

    // ==================== Filter Expression Conversion Tests ====================

    mod filter_expression_tests {
        use super::*;
        use proximadb::core::search::{ComparisonOperator, FilterExpression};

        #[test]
        fn test_convert_equals_expression() {
            let expr = FilterExpression::Comparison {
                field: "title".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("Test Value"),
            };

            let result = SstTextFilterEvaluator::convert_filter_expression(&expr);
            assert!(result.is_some());

            let (field, op, value) = result.unwrap();
            assert_eq!(field, "title");
            assert_eq!(op, TextComparisonOp::Equals);
            assert_eq!(value, "Test Value");
        }

        #[test]
        fn test_convert_contains_expression() {
            let expr = FilterExpression::Comparison {
                field: "description".to_string(),
                operator: ComparisonOperator::Contains,
                value: serde_json::json!("keyword"),
            };

            let result = SstTextFilterEvaluator::convert_filter_expression(&expr);
            assert!(result.is_some());

            let (field, op, value) = result.unwrap();
            assert_eq!(field, "description");
            assert_eq!(op, TextComparisonOp::Contains);
            assert_eq!(value, "keyword");
        }

        #[test]
        fn test_convert_starts_with_expression() {
            let expr = FilterExpression::Comparison {
                field: "name".to_string(),
                operator: ComparisonOperator::StartsWith,
                value: serde_json::json!("Prefix"),
            };

            let result = SstTextFilterEvaluator::convert_filter_expression(&expr);
            assert!(result.is_some());

            let (field, op, value) = result.unwrap();
            assert_eq!(field, "name");
            assert_eq!(op, TextComparisonOp::StartsWith);
            assert_eq!(value, "Prefix");
        }

        #[test]
        fn test_convert_is_null_expression() {
            let expr = FilterExpression::Comparison {
                field: "optional_field".to_string(),
                operator: ComparisonOperator::IsNull,
                value: serde_json::Value::Null,
            };

            let result = SstTextFilterEvaluator::convert_filter_expression(&expr);
            assert!(result.is_some());

            let (field, op, _) = result.unwrap();
            assert_eq!(field, "optional_field");
            assert_eq!(op, TextComparisonOp::IsNull);
        }

        #[test]
        fn test_convert_numeric_operator_returns_none() {
            // Numeric operators should not convert to TEXT operators
            let expr = FilterExpression::Comparison {
                field: "count".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: serde_json::json!(10),
            };

            let result = SstTextFilterEvaluator::convert_filter_expression(&expr);
            assert!(result.is_none());
        }
    }

    // ==================== End-to-End Tests ====================

    mod e2e_tests {
        use super::*;

        #[test]
        fn test_complete_text_column_workflow() {
            // 1. Create processor and register columns
            let mut processor = SstTextColumnProcessor::new();
            processor.register_text_column(TextColumnDefinition {
                name: "title".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: false,
                ngram_size: 3,
            });

            // 2. Create test records
            let records = vec![
                create_record_with_text("doc1", "title", "Introduction to Machine Learning"),
                create_record_with_text("doc2", "title", "Deep Learning Fundamentals"),
                create_record_with_text("doc3", "title", "Introduction to Data Science"),
            ];

            // 3. Process batch
            let result = processor.process_batch(&records).unwrap();
            assert_eq!(result.stats.get("title").unwrap().inline_count, 3);

            // 4. Set up filter evaluator
            let mut filter_eval = SstTextFilterEvaluator::new();
            filter_eval.register_column("title".to_string());

            // 5. Set up reader
            let reader = SstTextColumnReader::default();

            // 6. Extract values and filter
            let values = reader.extract_text_values(&records, "title");

            // Filter for "Introduction" in title
            let matches = filter_eval
                .evaluate(
                    "title",
                    &TextComparisonOp::Contains,
                    "Introduction",
                    &values,
                )
                .unwrap();

            assert_eq!(matches, vec![0, 2]); // doc1 and doc3

            // Filter for titles starting with "Deep"
            let matches = filter_eval
                .evaluate("title", &TextComparisonOp::StartsWith, "Deep", &values)
                .unwrap();

            assert_eq!(matches, vec![1]); // Only doc2
        }

        #[test]
        fn test_mixed_storage_strategies_workflow() {
            let mut processor = SstTextColumnProcessor::new();

            // Register columns with different strategies
            processor.register_text_column(TextColumnDefinition {
                name: "short_title".to_string(),
                storage_strategy: TextStorageStrategy::Inline,
                enable_ngram_bloom: false,
                ngram_size: 3,
            });

            // Create records with varying text sizes
            let short_text = "Short title";
            let medium_text = "x".repeat(5000); // > 4KB, will be chunked if adaptive

            let records = vec![
                create_record_with_text("rec1", "short_title", short_text),
                create_record_with_text("rec2", "short_title", &medium_text),
            ];

            // Process should handle different storage paths
            let result = processor.process_batch(&records);
            assert!(result.is_ok());
        }
    }
}
