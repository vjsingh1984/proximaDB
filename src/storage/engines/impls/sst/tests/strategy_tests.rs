#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::storage::engines::impls::sst::IndexEntry;
    use crate::storage::engines::impls::sst::readers::sst_query_engine::{
        ReadMode, ReadStrategy, SstableIndex, UnifiedSstableReader,
    };
    use crate::storage::engines::impls::sst::{
        CompressionAlgorithmSst, DataBlock, DataBlockMetadata, SstRecord, SstableHeader,
        SstableWriter,
    };
    use crate::storage::persistence::filesystem::local::LocalFileSystem;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tempfile::TempDir;

    // Define test-specific FilterPredicate
    #[derive(Debug, Clone, Default)]
    struct FilterPredicate {
        metadata_filters: Vec<(String, String)>,
        key_filter: Option<String>,
        timestamp_range: Option<(u32, u32)>,
    }

    // Helper to create test SST records
    fn create_test_records(count: usize, prefix: &str) -> Vec<SstRecord> {
        use crate::proto::proximadb_v1::{MetadataItem, metadata_item};

        (0..count)
            .map(|i| SstRecord {
                id: format!("{}_{:04}", prefix, i),
                vector: vec![i as f32; 384],
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(metadata_item::Value::StringValue(format!("cat_{}", i % 3))),
                    },
                    MetadataItem {
                        key: "score".to_string(),
                        value: Some(metadata_item::Value::StringValue(
                            (i as f32 * 1.5).to_string(),
                        )),
                    },
                ],
                timestamp: 1000 + i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: i as u64,
                level: 0,
            })
            .collect()
    }

    // Helper to create test DataBlock with hierarchical metadata
    fn create_test_datablock(block_id: u32, records: Vec<SstRecord>) -> DataBlock {
        let mut metadata_stats = DataBlockMetadata {
            min_key: records.first().map(|r| r.id.clone()).clone(),
            max_key: records.last().map(|r| r.id.clone()).clone(),
            // min_timestamp removed -  records.iter().map(|r| r.timestamp).min(),
            // max_timestamp removed -  records.iter().map(|r| r.timestamp).max(),
            record_count: records.len() as u32,
            null_count: 0,
            metadata_columns: vec!["category".to_string(), "score".to_string()],
            min_values: HashMap::new(),
            max_values: HashMap::new(),
        };

        // Calculate min/max metadata values
        for record in &records {
            for item in &record.metadata {
                if let Some(value) = &item.value {
                    let value_str = match value {
                        crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => s.clone(),
                        _ => continue, // Skip non-string values for this test
                    };

                    metadata_stats
                        .min_values
                        .entry(item.key.clone())
                        .and_modify(|v| {
                            if let (Ok(existing), Ok(new)) = (
                                v.as_deref().unwrap().parse::<f32>(),
                                value_str.parse::<f32>(),
                            ) {
                                if new < existing {
                                    *v = serde_json::Value::String(value_str.clone());
                                }
                            }
                        })
                        .or_insert(serde_json::Value::String(value_str.clone()));

                    metadata_stats
                        .max_values
                        .entry(item.key.clone())
                        .and_modify(|v| {
                            if let (Ok(existing), Ok(new)) = (
                                v.as_deref().unwrap().parse::<f32>(),
                                value_str.parse::<f32>(),
                            ) {
                                if new > existing {
                                    *v = serde_json::Value::String(value_str.clone());
                                }
                            }
                        })
                        .or_insert(serde_json::Value::String(value_str.clone()));
                }
            }
        }

        let mut block = DataBlock::new(block_id, records);
        block.metadata_stats = metadata_stats;

        // Create a simple bloom filter for the block
        let bloom_data = vec![0xFF; 64]; // Dummy bloom filter data
        block.block_bloom_filter = Some(bloom_data);

        block
    }

    async fn create_hierarchical_sst_file(path: &str) -> Result<()> {
        use crate::storage::persistence::filesystem::FilesystemConfig;

        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );
        let writer = SstableWriter::new(path, 3 * 1024 * 1024, fs.clone()); // 3MB blocks

        // Create all records and write them
        let mut all_records = std::collections::BTreeMap::new();
        for block_id in 0..3 {
            let records = create_test_records(100, &format!("block{}", block_id));
            for record in records {
                all_records.insert(record.id.clone(), record);
            }
        }

        let record_count = all_records.len();
        let sorted_records_iter = all_records.into_iter();
        writer
            .write_sorted_vector_records(sorted_records_iter, record_count)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_full_scan_strategy_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_full_scan.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Create hierarchical SST file
        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test full scan strategy
        let strategy = ReadStrategy::FullScan;
        let records = reader.read_with_strategy(&strategy).await.unwrap();

        // Validate results
        assert_eq!(records.len(), 300, "Should read all 300 records");

        // Verify records are from all blocks
        let block0_records: Vec<_> = records
            .iter()
            .filter(|r| r.id.starts_with("block0"))
            .collect();
        let block1_records: Vec<_> = records
            .iter()
            .filter(|r| r.id.starts_with("block1"))
            .collect();
        let block2_records: Vec<_> = records
            .iter()
            .filter(|r| r.id.starts_with("block2"))
            .collect();

        assert_eq!(
            block0_records.len(),
            100,
            "Should have 100 records from block 0"
        );
        assert_eq!(
            block1_records.len(),
            100,
            "Should have 100 records from block 1"
        );
        assert_eq!(
            block2_records.len(),
            100,
            "Should have 100 records from block 2"
        );

        // Verify order preservation
        for i in 0..100 {
            assert_eq!(block0_records[i].id, format!("block0_{:04}", i));
            assert_eq!(block1_records[i].id, format!("block1_{:04}", i));
            assert_eq!(block2_records[i].id, format!("block2_{:04}", i));
        }
    }

    #[tokio::test]
    async fn test_filtered_scan_strategy_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_filtered.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test filtered scan with metadata predicate
        let predicate = FilterPredicate {
            metadata_filters: vec![("category".to_string(), "cat_0".to_string())],
            ..Default::default()
        };

        let strategy = ReadStrategy::FilteredScan(predicate);
        let records = reader.read_with_strategy(&strategy).await.unwrap();

        // Validate filtered results
        assert!(records.len() <= 100, "Should have filtered records");

        // All records should have category = cat_0
        for record in &records {
            let category = record
                .metadata
                .iter()
                .find(|(k, _)| k == "category")
                .map(|(_, v)| v.as_deref())
                .unwrap();
            assert_eq!(category, "cat_0", "All records should match filter");
        }
    }

    #[tokio::test]
    async fn test_range_scan_strategy_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_range.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test filtered scan with key range filter (since RangeScan doesn't exist)
        // We'll use FilteredScan with a custom filter for range
        let strategy = ReadStrategy::FilteredScan(FilterExpression::Range {
            field: "id".to_string(),
            start: serde_json::Value::String("block1_0050".to_string()),
            end: serde_json::Value::String("block2_0020".to_string()),
        });

        let records = reader.read_with_strategy(&strategy).await.unwrap();

        // Validate range results
        assert!(!records.is_empty(), "Should have records in range");

        // All records should be within range
        for record in &records {
            assert!(record.id >= "block1_0050");
            assert!(record.id <= "block2_0020");
        }

        // Should include records from block1 (50-99) and block2 (0-20)
        let block1_count = records
            .iter()
            .filter(|r| r.id.starts_with("block1"))
            .count();
        let block2_count = records
            .iter()
            .filter(|r| r.id.starts_with("block2"))
            .count();

        assert_eq!(block1_count, 50, "Should have 50 records from block1");
        assert_eq!(block2_count, 21, "Should have 21 records from block2");
    }

    #[tokio::test]
    async fn test_point_lookup_strategy_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_point.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test point lookup
        let strategy = ReadStrategy::PointLookup {
            key: "block1_0042".to_string(),
        };

        let records = reader.read_with_strategy(&strategy).await.unwrap();

        // Validate point lookup result
        assert_eq!(records.len(), 1, "Should find exactly one record");
        assert_eq!(records[0].id, "block1_0042");
        assert_eq!(records[0].vector[0], 42.0);
    }

    #[tokio::test]
    async fn test_compaction_strategy_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_compaction.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test compaction strategy (streaming all records efficiently)
        let strategy = ReadStrategy::Compaction;
        let records = reader.read_with_strategy(&strategy).await.unwrap();

        // Validate compaction results
        assert_eq!(
            records.len(),
            300,
            "Should stream all records for compaction_info"
        );

        // Verify no duplicates
        let mut seen = std::collections::HashSet::new();
        for record in &records {
            assert!(seen.insert(record.id.clone()), "No duplicate records");
        }
    }

    #[tokio::test]
    async fn test_block_level_filtering_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_block_filter.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Read header and index to validate hierarchical structure
        let header = reader.read_header_async().await.unwrap();
        let index = reader
            .read_index_block_async(&ReadStrategy::FullScan)
            .await
            .unwrap();

        // Validate index has proper block metadata
        assert_eq!(index.entries.len(), 3, "Should have 3 blocks");

        for (i, entry) in index.entries.iter().enumerate() {
            // Each entry should have offset and size
            assert!(
                entry.offset > 0 || i == 0,
                "Block {} should have valid offset",
                i
            );
            assert!(entry.size > 0, "Block {} should have valid size", i);

            // Validate block can be read individually
            let block = reader
                .read_data_block_async(i as u64, ReadMode::Buffered)
                .await
                .unwrap();
            assert_eq!(block.block_id, i as u32);
            assert!(!block.records.is_none());

            // Validate hierarchical metadata
            assert!(!block.metadata_stats.min_key.is_none());
            assert!(!block.metadata_stats.max_key.is_none());
            assert!(block.metadata_stats.record_count > 0);
            assert!(
                block.block_bloom_filter.is_some(),
                "Block should have bloom filter"
            );
        }
    }

    #[tokio::test]
    async fn test_metadata_statistics_validation() {
        use crate::proto::proximadb_v1::{MetadataItem, metadata_item};

        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_metadata_stats.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Create SST with specific metadata patterns
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );
        let writer = SstableWriter::new(&sst_path, 3 * 1024 * 1024, fs.clone());

        // Create all records
        let mut all_records = std::collections::BTreeMap::new();

        // Block 0: scores 0-49
        for i in 0..50 {
            let record = SstRecord {
                id: format!("record_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![
                    MetadataItem {
                        key: "score".to_string(),
                        value: Some(metadata_item::Value::StringValue(i.to_string())),
                    },
                    MetadataItem {
                        key: "type".to_string(),
                        value: Some(metadata_item::Value::StringValue("A".to_string())),
                    },
                ],
                timestamp: 1000 + i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: i as u64,
                level: 0,
            };
            all_records.insert(record.id.clone(), record);
        }

        // Block 1: scores 50-99
        for i in 50..100 {
            let record = SstRecord {
                id: format!("record_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![
                    MetadataItem {
                        key: "score".to_string(),
                        value: Some(metadata_item::Value::StringValue(i.to_string())),
                    },
                    MetadataItem {
                        key: "type".to_string(),
                        value: Some(metadata_item::Value::StringValue("B".to_string())),
                    },
                ],
                timestamp: 1000 + i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: i as u64,
                level: 0,
            };
            all_records.insert(record.id.clone(), record);
        }

        let record_count = all_records.len();
        let sorted_records_iter = all_records.into_iter();
        writer
            .write_sorted_vector_records(sorted_records_iter, record_count)
            .await
            .unwrap();

        // Read and validate metadata statistics
        let mut reader = UnifiedSstableReader::open(fs.clone(), &sst_path)
            .await
            .unwrap();
        let index = reader
            .read_index_block_async(&ReadStrategy::FullScan)
            .await
            .unwrap();

        // Block 0 should have scores 0-49
        let block0 = reader
            .read_data_block_async(0, ReadMode::Buffered)
            .await
            .unwrap();
        assert_eq!(
            block0.metadata_stats.min_values.get(key).unwrap(),
            &serde_json::Value::String("0".to_string())
        );
        assert_eq!(
            block0.metadata_stats.max_values.get(key).unwrap(),
            &serde_json::Value::String("49".to_string())
        );

        // Block 1 should have scores 50-99
        let block1 = reader
            .read_data_block_async(1, ReadMode::Buffered)
            .await
            .unwrap();
        assert_eq!(
            block1.metadata_stats.min_values.get(key).unwrap(),
            &serde_json::Value::String("50".to_string())
        );
        assert_eq!(
            block1.metadata_stats.max_values.get(key).unwrap(),
            &serde_json::Value::String("99".to_string())
        );

        // Test metadata-based filtering using statistics
        let predicate = FilterPredicate {
            metadata_filters: vec![("score".to_string(), "25".to_string())],
            ..Default::default()
        };

        // Only block 0 should be accessed for score=25
        let strategy = ReadStrategy::FilteredScan(predicate);
        let records = reader.read_with_strategy(&strategy).await.unwrap();

        // Should find the record with score=25
        let target = records
            .iter()
            .find(|r| r.metadata.iter().any(|(k, v)| k == "score" && v == "25"));
        assert!(target.is_some(), "Should find record with score=25");
    }

    #[tokio::test]
    async fn test_bloom_filter_effectiveness() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_bloom.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test bloom filter for non-existent key
        let strategy = ReadStrategy::PointLookup {
            key: "non_existent_key".to_string(),
        };

        let records = reader.read_with_strategy(&strategy).await.unwrap();
        assert!(
            records.is_none(),
            "Bloom filter should prevent unnecessary reads"
        );

        // Test bloom filter for existing key
        let strategy = ReadStrategy::PointLookup {
            key: "block1_0050".to_string(),
        };

        let records = reader.read_with_strategy(&strategy).await.unwrap();
        assert_eq!(records.len(), 1, "Should find existing record");
        assert_eq!(records[0].id, "block1_0050");
    }

    #[tokio::test]
    async fn test_random_block_access_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_random_access.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );

        // UnifiedSstableReader doesn't have open(), need to construct it differently
        // For now, comment out as we need to fix the reader construction
        todo!("Fix UnifiedSstableReader construction");

        // Test random access to blocks in non-sequential order
        let block2 = reader
            .read_data_block_async(2, ReadMode::Buffered)
            .await
            .unwrap();
        assert_eq!(block2.block_id, 2);
        assert!(block2.records[0].id.starts_with("block2"));

        let block0 = reader
            .read_data_block_async(0, ReadMode::Buffered)
            .await
            .unwrap();
        assert_eq!(block0.block_id, 0);
        assert!(block0.records[0].id.starts_with("block0"));

        let block1 = reader
            .read_data_block_async(1, ReadMode::Buffered)
            .await
            .unwrap();
        assert_eq!(block1.block_id, 1);
        assert!(block1.records[0].id.starts_with("block1"));

        // Verify blocks are independent and complete
        assert_eq!(block0.records.len(), 100);
        assert_eq!(block1.records.len(), 100);
        assert_eq!(block2.records.len(), 100);
    }

    #[tokio::test]
    async fn test_streaming_iterator_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_streaming.sstable")
            .to_str()
            .unwrap()
            .to_string();

        create_hierarchical_sst_file(&sst_path).await.unwrap();

        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(sst_path.clone()),
        );
        let reader = UnifiedSstableReader::open(fs.clone(), &sst_path)
            .await
            .unwrap();

        // Test streaming with BlockIterator
        let mut iterator = reader
            .create_block_iterator(ReadMode::Streaming)
            .await
            .unwrap();

        let mut total_records = 0;
        let mut last_id = String::new();

        while let Some(block) = iterator.next_block().await.unwrap() {
            for record in &block.records {
                total_records += 1;
                // Verify ordering
                assert!(record.id > last_id || last_id.is_none());
                last_id = record.id.clone();
            }

            // Verify hierarchical metadata is present
            assert!(block.block_bloom_filter.is_some());
            assert!(block.metadata_stats.record_count > 0);
        }

        assert_eq!(total_records, 300, "Should stream all records");
    }

    #[tokio::test]
    async fn test_compression_handling_validation() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_compression.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Create SST with compression
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some(format!("file://{}", sst_path)),
            ..Default::default()
        };
        let fs = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(config)
                .await
                .unwrap(),
        );
        let writer = SstableWriter::new(&sst_path, 3 * 1024 * 1024, fs.clone());

        let mut all_records = std::collections::BTreeMap::new();
        let records = create_test_records(1000, "compressed");
        for record in records {
            all_records.insert(record.id.clone(), record);
        }

        let record_count = all_records.len();
        let sorted_records_iter = all_records.into_iter();
        writer
            .write_sorted_vector_records(sorted_records_iter, record_count)
            .await
            .unwrap();

        // Validate compressed blocks can be read
        let mut reader = UnifiedSstableReader::open(fs.clone(), &sst_path)
            .await
            .unwrap();

        let strategy = ReadStrategy::FullScan;
        let records = reader.read_with_strategy(&strategy).await.unwrap();

        assert_eq!(
            records.len(),
            1000,
            "Should decompress and read all records"
        );

        // Verify compression metadata
        let header = reader.read_header_async().await.unwrap();
        assert_eq!(header.compression_algorithm, CompressionAlgorithm::Zstd);
    }
}
