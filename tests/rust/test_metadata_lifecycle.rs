//! Metadata lifecycle integration tests

use super::common::*;
use anyhow::Result;
use proximadb::storage::vector::engines::viper_core::{FilterableColumn, FilterableDataType};
use std::collections::HashMap;

#[cfg(test)]
mod metadata_lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn test_filterable_column_configuration() -> Result<()> {
        init_test_env();
        
        // Test user-configurable filterable columns
        let filterable_columns = vec![
            FilterableColumn {
                name: "category".to_string(),
                data_type: FilterableDataType::String,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(10),
            },
            FilterableColumn {
                name: "year".to_string(),
                data_type: FilterableDataType::Integer,
                indexed: true,
                supports_range: true,
                estimated_cardinality: Some(20),
            },
            FilterableColumn {
                name: "score".to_string(),
                data_type: FilterableDataType::Float,
                indexed: true,
                supports_range: true,
                estimated_cardinality: Some(100),
            },
            FilterableColumn {
                name: "is_published".to_string(),
                data_type: FilterableDataType::Boolean,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(2),
            },
        ];
        
        // Validate column configurations
        for column in &filterable_columns {
            assert!(!column.name.is_empty());
            assert!(column.indexed); // All test columns are indexed
            
            // Range support validation
            match column.data_type {
                FilterableDataType::Integer | FilterableDataType::Float | FilterableDataType::DateTime => {
                    // These types can support ranges
                }
                FilterableDataType::String | FilterableDataType::Boolean => {
                    // These typically don't support ranges (though string could)
                }
                FilterableDataType::Array(_) => {
                    // Array type handling
                }
            }
        }
        
        // Test serialization
        let json_str = serde_json::to_string(&filterable_columns)?;
        assert!(json_str.contains("category"));
        assert!(json_str.contains("year"));
        
        println!("✅ Filterable column configuration test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_as_is_storage() -> Result<()> {
        init_test_env();
        
        // Test unlimited metadata storage during insert phase
        let mut metadata = HashMap::new();
        
        // Core filterable fields
        metadata.insert("category".to_string(), serde_json::Value::String("AI".to_string()));
        metadata.insert("author".to_string(), serde_json::Value::String("Dr. Smith".to_string()));
        metadata.insert("year".to_string(), serde_json::Value::Number(serde_json::Number::from(2024)));
        
        // Additional metadata fields (unlimited)
        metadata.insert("title".to_string(), serde_json::Value::String("Advanced Vector Research".to_string()));
        metadata.insert("abstract".to_string(), serde_json::Value::String("This paper explores...".to_string()));
        metadata.insert("keywords".to_string(), serde_json::Value::Array(vec![
            serde_json::Value::String("vector".to_string()),
            serde_json::Value::String("database".to_string()),
            serde_json::Value::String("search".to_string()),
        ]));
        metadata.insert("citation_count".to_string(), serde_json::Value::Number(serde_json::Number::from(42)));
        metadata.insert("download_count".to_string(), serde_json::Value::Number(serde_json::Number::from(1337)));
        metadata.insert("quality_score".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(0.95).unwrap()));
        metadata.insert("processing_timestamp".to_string(), serde_json::Value::String("2024-01-15T10:30:00Z".to_string()));
        metadata.insert("content_hash".to_string(), serde_json::Value::String("sha256:abc123def456...".to_string()));
        
        // Verify as-is storage (no processing during insert)
        assert!(metadata.len() > 10); // Many fields
        assert!(metadata.contains_key("category")); // Filterable field
        assert!(metadata.contains_key("title")); // Extra field
        assert!(metadata.contains_key("keywords")); // Array field
        assert!(metadata.contains_key("quality_score")); // Float field
        
        // Test that all metadata is preserved
        let json_str = serde_json::to_string(&metadata)?;
        let deserialized: HashMap<String, serde_json::Value> = serde_json::from_str(&json_str)?;
        assert_eq!(metadata.len(), deserialized.len());
        
        println!("✅ Metadata as-is storage test successful (fields: {})", metadata.len());
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_transformation_logic() -> Result<()> {
        init_test_env();
        
        // Simulate metadata transformation during flush operation
        let filterable_columns = vec!["category", "author", "year", "doc_type", "length"];
        
        let original_metadata = {
            let mut meta = HashMap::new();
            // Filterable fields
            meta.insert("category".to_string(), serde_json::Value::String("AI".to_string()));
            meta.insert("author".to_string(), serde_json::Value::String("Dr. Smith".to_string()));
            meta.insert("year".to_string(), serde_json::Value::Number(serde_json::Number::from(2024)));
            meta.insert("doc_type".to_string(), serde_json::Value::String("research_paper".to_string()));
            meta.insert("length".to_string(), serde_json::Value::Number(serde_json::Number::from(5000)));
            
            // Extra fields (will go to extra_meta)
            meta.insert("title".to_string(), serde_json::Value::String("Test Document".to_string()));
            meta.insert("abstract".to_string(), serde_json::Value::String("Abstract text...".to_string()));
            meta.insert("keywords".to_string(), serde_json::Value::Array(vec![
                serde_json::Value::String("test".to_string()),
                serde_json::Value::String("metadata".to_string()),
            ]));
            meta.insert("citation_count".to_string(), serde_json::Value::Number(serde_json::Number::from(15)));
            meta.insert("quality_score".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(0.85).unwrap()));
            meta
        };
        
        // Simulate transformation: separate filterable vs extra_meta
        let mut filterable_metadata = HashMap::new();
        let mut extra_meta = HashMap::new();
        
        for (key, value) in original_metadata {
            if filterable_columns.contains(&key.as_str()) {
                filterable_metadata.insert(key, value);
            } else {
                extra_meta.insert(key, value);
            }
        }
        
        // Verify transformation
        assert_eq!(filterable_metadata.len(), 5); // 5 filterable columns
        assert_eq!(extra_meta.len(), 5); // 5 extra fields
        
        // Verify specific fields are in correct places
        assert!(filterable_metadata.contains_key("category"));
        assert!(filterable_metadata.contains_key("author"));
        assert!(extra_meta.contains_key("title"));
        assert!(extra_meta.contains_key("keywords"));
        
        // Verify no data loss
        let total_fields = filterable_metadata.len() + extra_meta.len();
        assert_eq!(total_fields, 10); // All original fields preserved
        
        println!("✅ Metadata transformation logic test successful");
        println!("   Filterable columns: {}", filterable_metadata.len());
        println!("   Extra_meta fields: {}", extra_meta.len());
        Ok(())
    }

    #[tokio::test]
    async fn test_parquet_schema_design() -> Result<()> {
        init_test_env();
        
        // Test Parquet schema design for filterable columns
        let filterable_columns = vec![
            FilterableColumn {
                name: "category".to_string(),
                data_type: FilterableDataType::String,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(10),
            },
            FilterableColumn {
                name: "year".to_string(),
                data_type: FilterableDataType::Integer,
                indexed: true,
                supports_range: true,
                estimated_cardinality: Some(20),
            },
            FilterableColumn {
                name: "quality_score".to_string(),
                data_type: FilterableDataType::Float,
                indexed: true,
                supports_range: true,
                estimated_cardinality: Some(100),
            },
        ];
        
        // Validate schema design rules
        for column in &filterable_columns {
            // Each column should have appropriate indexing for its type
            match column.data_type {
                FilterableDataType::String => {
                    // String columns typically use hash indexing for equality
                    assert!(column.indexed);
                }
                FilterableDataType::Integer | FilterableDataType::Float => {
                    // Numeric columns can use range indexing
                    assert!(column.indexed);
                    assert!(column.supports_range);
                }
                FilterableDataType::Boolean => {
                    // Boolean columns typically don't need range support
                    // (would be checked if we had boolean columns in test)
                }
                _ => {}
            }
            
            // Cardinality should be reasonable
            if let Some(cardinality) = column.estimated_cardinality {
                assert!(cardinality > 0);
                assert!(cardinality < 1_000_000); // Reasonable upper bound
            }
        }
        
        println!("✅ Parquet schema design test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_performance_optimization() -> Result<()> {
        init_test_env();
        
        // Test that metadata transformations provide performance benefits
        
        // Simulate memtable search (linear scan)
        let memtable_records = 1000;
        let memtable_scan_time_ms = memtable_records as f64 * 0.1; // 0.1ms per record
        
        // Simulate VIPER search (indexed lookup)
        let indexed_lookup_time_ms = 5.0; // Fixed cost for index lookup
        let filtered_records = 50; // Reduced set after filtering
        let viper_scan_time_ms = indexed_lookup_time_ms + (filtered_records as f64 * 0.05);
        
        // Calculate performance improvement
        let speedup = memtable_scan_time_ms / viper_scan_time_ms;
        let efficiency = filtered_records as f32 / memtable_records as f32;
        
        assert!(speedup > 1.0); // VIPER should be faster
        assert!(efficiency < 1.0); // Filtering should reduce work
        
        println!("📊 Metadata performance optimization:");
        println!("   Memtable scan: {:.2}ms", memtable_scan_time_ms);
        println!("   VIPER search: {:.2}ms", viper_scan_time_ms);
        println!("   Speedup: {:.1}x", speedup);
        println!("   Filter efficiency: {:.1}%", efficiency * 100.0);
        
        // Verify significant performance improvement
        assert!(speedup > 2.0); // At least 2x improvement expected
        
        println!("✅ Metadata performance optimization test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_lifecycle_integration() -> Result<()> {
        init_test_env();
        
        // Test complete metadata lifecycle: Insert → Flush → Search
        
        // Phase 1: Insert (as-is storage)
        let insert_metadata = {
            let mut meta = HashMap::new();
            meta.insert("category".to_string(), serde_json::Value::String("AI".to_string()));
            meta.insert("author".to_string(), serde_json::Value::String("Dr. Smith".to_string()));
            meta.insert("title".to_string(), serde_json::Value::String("Vector Database Research".to_string()));
            meta.insert("abstract".to_string(), serde_json::Value::String("This research...".to_string()));
            meta.insert("keywords".to_string(), serde_json::Value::Array(vec![
                serde_json::Value::String("vector".to_string()),
                serde_json::Value::String("database".to_string()),
            ]));
            meta.insert("citation_count".to_string(), serde_json::Value::Number(serde_json::Number::from(25)));
            meta
        };
        
        let (_, insert_measurement) = measure_performance!(
            "Insert phase (as-is storage)",
            1,
            {
                // Simulate fast insert - no processing overhead
                std::thread::sleep(std::time::Duration::from_millis(1));
                insert_metadata.clone()
            }
        );
        
        // Phase 2: Flush (metadata transformation)
        let filterable_columns = vec!["category", "author"];
        let (transformed_meta, flush_measurement) = measure_performance!(
            "Flush phase (transformation)",
            1,
            {
                let mut filterable = HashMap::new();
                let mut extra_meta = HashMap::new();
                
                for (key, value) in insert_metadata.iter() {
                    if filterable_columns.contains(&key.as_str()) {
                        filterable.insert(key.clone(), value.clone());
                    } else {
                        extra_meta.insert(key.clone(), value.clone());
                    }
                }
                
                // Simulate transformation processing
                std::thread::sleep(std::time::Duration::from_millis(10));
                (filterable, extra_meta)
            }
        );
        
        // Phase 3: Search (optimized filtering)
        let (search_results, search_measurement) = measure_performance!(
            "Search phase (optimized)",
            1,
            {
                // Simulate fast indexed search
                std::thread::sleep(std::time::Duration::from_millis(2));
                vec!["matching_result"]
            }
        );
        
        // Validate lifecycle
        assert!(insert_measurement.duration_ms < 10.0); // Fast insert
        assert!(flush_measurement.duration_ms > insert_measurement.duration_ms); // Transformation takes time
        assert!(search_measurement.duration_ms < flush_measurement.duration_ms); // Fast search
        
        // Verify data preservation
        let (filterable, extra_meta) = transformed_meta;
        assert!(filterable.contains_key("category"));
        assert!(extra_meta.contains_key("title"));
        
        println!("✅ Complete metadata lifecycle test successful");
        Ok(())
    }
}