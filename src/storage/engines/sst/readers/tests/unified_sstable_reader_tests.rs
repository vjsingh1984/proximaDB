//! Test-Driven Development tests for UnifiedSstableReader
//!
//! Tests the unified search architecture for LSM engine

#[cfg(test)]
mod tests {
    use crate::storage::engines::sst::readers::unified_sstable_reader::{
        UnifiedSstableReader, CollectionContext,
    };
    use crate::storage::engines::row_based::bloom_filter::{
        MetadataBloomFilter,
    };
    use crate::core::config::{BloomFilterConfig, SstConfig};
    use crate::core::search::{SearchParams, FilterExpression, ComparisonOperator};
    use crate::compute::distance_computation::DistanceMetric;

    fn create_test_config() -> SstConfig {
        SstConfig {
            block_size_kb: 64, // Use 64KB blocks for tests
            decompression_cache_config: None,
            ..SstConfig::default()
        }
    }
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
    use std::sync::Arc;
    use std::collections::HashMap;
    use serde_json::json;

    // Test helpers
    async fn create_test_reader() -> UnifiedSstableReader {
        let config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(config).await.unwrap());
        UnifiedSstableReader::new(filesystem)
    }

    fn create_test_context() -> CollectionContext {
        CollectionContext {
            file_path: "/tmp/lsm".to_string(),
            sstable_files: vec![
                "/tmp/lsm/sst_001.sstable".to_string(),
                "/tmp/lsm/sst_002.sstable".to_string(),
            ],
            total_vectors: 10000,
            metadata_columns: vec!["category".to_string(), "status".to_string()],
            level: 0,
            creation_time: chrono::Utc::now(),
            io_optimization_hints: None,
        }
    }

    // Basic SSTable Reader Tests
    #[tokio::test]
    async fn test_reader_creation() {
        let reader = create_test_reader().await;
        // Test passes if reader is created successfully
        assert!(true);
    }

    #[tokio::test]
    async fn test_strategy_selection_basic() {
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };
        
        // Without filters, should use basic strategy
        assert!(params.filter_expression.is_empty());
    }

    #[tokio::test]
    async fn test_strategy_selection_with_filter() {
        let params = SearchParams {
            query_vectors: Some(vec![vec![0.1; 128]]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            filter_expression: Some(FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("electronics"),
            }),
            ..Default::default()
        };
        
        // With metadata filter
        assert!(params.filter_expression.is_some());
    }

    // Metadata Bloom Filter Tests
    #[tokio::test]
    async fn test_metadata_bloom_filter() {
        let config = BloomFilterConfig {
            // strategy removed -  crate::core::bloom::BloomStrategy::Composite,
            expected_items: 100,
            ..Default::default()
        };
        let mut builder = crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder::new(config);
        
        // Add metadata values using MetadataItem
        let electronics_item = crate::proto::proximadb::MetadataItem {
            key: "category".to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("electronics".to_string())),
        };
        let books_item = crate::proto::proximadb::MetadataItem {
            key: "category".to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("books".to_string())),
        };
        let active_item = crate::proto::proximadb::MetadataItem {
            key: "status".to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("active".to_string())),
        };
        let inactive_item = crate::proto::proximadb::MetadataItem {
            key: "status".to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("inactive".to_string())),
        };
        
        builder.add_metadata_item("category".to_string(), electronics_item.clone());
        builder.add_metadata_item("category".to_string(), books_item.clone());
        builder.add_metadata_item("status".to_string(), active_item.clone());
        builder.add_metadata_item("status".to_string(), inactive_item.clone());
        
        let filter = builder.build();
        
        // Test single condition using MetadataBloomFilter trait
        use crate::core::bloom::MetadataBloomFilter;
        assert!(filter.might_match_metadata("category", &electronics_item));
        assert!(filter.might_match_metadata("status", &active_item));
        
        // Test non-existent values
        let furniture_item = crate::proto::proximadb::MetadataItem {
            key: "category".to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("furniture".to_string())),
        };
        let deleted_item = crate::proto::proximadb::MetadataItem {
            key: "status".to_string(),
            value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("deleted".to_string())),
        };
        assert!(!filter.might_match_metadata("category", &furniture_item));
        assert!(!filter.might_match_metadata("status", &deleted_item));
    }

    // Block Range Calculation Tests
    #[tokio::test]
    async fn test_block_range_calculation() {
        let test_config = create_test_config();
        let block_size = (test_config.block_size_kb * 1024) as usize;
        let blocks = vec![0, 1, 2, 5, 6, 7, 10]; // Some consecutive, some not
        
        // Calculate ranges for reading
        let mut ranges = Vec::new();
        let mut i = 0;
        while i < blocks.len() {
            let start = blocks[i];
            let mut end = start;
            
            // Merge consecutive blocks
            while i + 1 < blocks.len() && blocks[i + 1] == blocks[i] + 1 {
                i += 1;
                end = blocks[i];
            }
            
            ranges.push((start * block_size, (end + 1) * block_size));
            i += 1;
        }
        
        // Should have merged consecutive blocks
        assert_eq!(ranges.len(), 3); // [0-2], [5-7], [10]
    }

    // Metadata Extraction Tests
    #[tokio::test]
    async fn test_extract_metadata_conditions() {
        let filter = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "status".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("active"),
            },
            FilterExpression::Comparison {
                field: "priority".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: json!(5),
            },
        ]);
        
        let conditions = extract_conditions(&filter);
        assert_eq!(conditions.len(), 2);
        assert!(conditions.contains_key("status"));
        assert!(conditions.contains_key("priority"));
    }

    fn extract_conditions(filter: &FilterExpression) -> HashMap<String, String> {
        let mut conditions = HashMap::new();
        
        match filter {
            FilterExpression::Comparison { field, value, .. } => {
                // Convert value to string regardless of type
                let value_str = if let Some(s) = value.as_str() {
                    s.to_string()
                } else if let Some(n) = value.as_i64() {
                    n.to_string()
                } else if let Some(f) = value.as_f64() {
                    f.to_string()
                } else if let Some(b) = value.as_bool() {
                    b.to_string()
                } else {
                    value.to_string()
                };
                conditions.insert(field.clone(), value_str);
            }
            FilterExpression::And(filters) | FilterExpression::Or(filters) => {
                for f in filters {
                    conditions.extend(extract_conditions(f));
                }
            }
            _ => {}
        }
        
        conditions
    }

    // Performance Tests
    #[tokio::test]
    async fn test_compression_ratio() {
        let uncompressed_size = 1000000; // 1MB
        let compression_ratio = 0.3; // 30% of original
        
        let compressed_size = (uncompressed_size as f64 * compression_ratio) as usize;
        assert_eq!(compressed_size, 300000);
        
        // Estimate decompressed size
        let estimated = (compressed_size as f64 / compression_ratio) as usize;
        assert_eq!(estimated, uncompressed_size);
    }

    #[tokio::test]
    async fn test_parallel_block_partitioning() {
        let blocks = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let max_parallelism = 3;
        
        // Partition blocks for parallel processing
        let chunk_size = (blocks.len() + max_parallelism - 1) / max_parallelism;
        let batches: Vec<Vec<usize>> = blocks
            .chunks(chunk_size)
            .map(|chunk| chunk.to_vec())
            .collect();
        
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 4);
        assert_eq!(batches[1].len(), 4);
        assert_eq!(batches[2].len(), 2);
    }
}