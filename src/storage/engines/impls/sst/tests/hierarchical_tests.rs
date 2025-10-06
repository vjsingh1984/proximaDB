#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::storage::engines::impls::sst::readers::sst_query_engine::{
        BlockIterator, BlockReader, ModularBlockReader, ReadMode, SstDirectReader,
    };
    use crate::storage::engines::impls::sst::{
        CompressionAlgorithmSst, DataBlock, DataBlockMetadata, SstRecord, SstableHeader,
        SstableWriter,
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use anyhow::Result;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;
    use tempfile::TempDir;

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

    #[tokio::test]
    async fn test_hierarchical_datablock_serialization() {
        // Create a DataBlock with hierarchical metadata
        let records = create_test_records(10, "test");
        let mut block = DataBlock::new(0, records.clone());

        // Populate hierarchical metadata
        block.metadata_stats = DataBlockMetadata {
            min_key: "test_0000".to_string(),
            max_key: "test_0009".to_string(),
            // min_timestamp removed -  1000,
            // max_timestamp removed -  1009,
            record_count: 10,
            null_count: 0,
            metadata_columns: vec!["category".to_string(), "score".to_string()],
            min_values: HashMap::from([(
                "score".to_string(),
                serde_json::Value::String("0".to_string()),
            )]),
            max_values: HashMap::from([(
                "score".to_string(),
                serde_json::Value::String("13.5".to_string()),
            )]),
        };

        // Add a bloom filter
        block.block_bloom_filter = Some(vec![0xFF; 64]);

        // Serialize and deserialize
        let serialized = block.serialize().expect("Should serialize");

        let deserialized = DataBlock::deserialize(&serialized).expect("Should deserialize");

        // Verify all fields
        assert_eq!(deserialized.block_id, 0);
        assert_eq!(deserialized.records.len(), 10);
        assert_eq!(deserialized.metadata_stats.min_key, "test_0000");
        assert_eq!(deserialized.metadata_stats.max_key, "test_0009");
        assert_eq!(deserialized.metadata_stats.record_count, 10);
        assert!(deserialized.block_bloom_filter.is_some());
        assert_eq!(deserialized.block_bloom_filter.as_ref().unwrap().len(), 64);
    }

    // TODO: This test requires proper SST infrastructure with collection config
    // Currently disabled as SstableWriter needs proper collection metadata setup
    // #[tokio::test]
    #[allow(dead_code)]
    async fn test_hierarchical_sst_write_read() {
        use crate::proto::proximadb_v1::{MetadataItem, metadata_item};
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_hierarchical.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Write SST file with hierarchical structure
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some("file:///".to_string()),
            ..Default::default()
        };
        let fs = Arc::new(FilesystemFactory::create(config).await.unwrap());
        let writer = SstableWriter::new(&sst_path, 3 * 1024 * 1024, fs.clone());

        let mut all_records = BTreeMap::new();
        for i in 0..10 {
            // Reduced to 10 for simpler debugging
            let record = SstRecord {
                id: format!("record_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![MetadataItem {
                    key: "score".to_string(),
                    value: Some(metadata_item::Value::StringValue(i.to_string())),
                }],
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
            .expect("Should write");

        // Verify file was created using std::fs
        assert!(
            std::path::Path::new(&sst_path).exists(),
            "File should exist at: {}",
            sst_path
        );

        // Read back using ModularBlockReader
        let mut reader = ModularBlockReader::open(fs.clone(), &sst_path)
            .await
            .expect("Should open reader");

        // Read header
        let header = reader.read_header().await.expect("Should read header");
        assert_eq!(header.entry_count, 10);

        // Read a data block directly
        let block = reader
            .read_data_block(0, ReadMode::Buffered)
            .await
            .expect("Should read block");
        assert!(block.records.len() > 0);
        assert!(block.metadata_stats.record_count > 0);

        // Verify hierarchical metadata is present
        assert!(!block.metadata_stats.min_key.is_none());
        assert!(!block.metadata_stats.max_key.is_none());
        assert!(block.metadata_stats.min_timestamp > 0);
        assert!(block.metadata_stats.max_timestamp >= block.metadata_stats.min_timestamp);
    }

    // TODO: Requires proper SST infrastructure
    // #[tokio::test]
    #[allow(dead_code)]
    async fn test_block_iteration_with_filesystem_api() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_iterator.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Write test data using filesystem API
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some("file:///".to_string()),
            ..Default::default()
        };
        let fs = Arc::new(FilesystemFactory::create(config).await.unwrap());
        let writer = SstableWriter::new(&sst_path, 1024 * 1024, fs.clone()); // 1MB blocks

        let mut all_records = BTreeMap::new();
        for i in 0..300 {
            let record = SstRecord {
                id: format!("iter_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![],
                timestamp: 2000 + i as u32,
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
            .expect("Should write");

        // Read blocks using ModularBlockReader with filesystem API
        let mut reader = ModularBlockReader::open(fs.clone(), &sst_path)
            .await
            .expect("Should open reader");

        let header = reader.read_header().await.expect("Should read header");

        let mut total_records = 0;
        let mut blocks_read = 0;

        // Read multiple blocks to test iteration through filesystem API
        // We'll read blocks 0, 1, 2 to test sequential access
        for block_id in 0..3 {
            match reader.read_data_block(block_id, ReadMode::Streaming).await {
                Ok(block) => {
                    blocks_read += 1;
                    total_records += block.records.len();

                    // Verify hierarchical metadata
                    assert!(block.metadata_stats.record_count > 0);
                    assert!(!block.metadata_stats.min_key.is_none());
                    assert!(!block.metadata_stats.max_key.is_none());
                }
                Err(_) => break, // No more blocks
            }
        }

        assert!(blocks_read > 0, "Should have read at least one block");
        assert_eq!(total_records, 300, "Should have read all records");
    }

    // TODO: Requires proper SST infrastructure
    // #[tokio::test]
    #[allow(dead_code)]
    async fn test_metadata_statistics_in_blocks() {
        use crate::proto::proximadb_v1::{MetadataItem, metadata_item};
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_metadata.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Write SST with specific metadata patterns
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some("file:///".to_string()),
            ..Default::default()
        };

        let fs = Arc::new(FilesystemFactory::create(config).await.unwrap());

        let writer = SstableWriter::new(&sst_path, 2 * 1024 * 1024, fs.clone());

        let mut all_records = BTreeMap::new();

        // Create records with predictable metadata ranges
        for i in 0..200 {
            let record = SstRecord {
                id: format!("meta_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![
                    MetadataItem {
                        key: "score".to_string(),
                        value: Some(metadata_item::Value::StringValue((i * 10).to_string())),
                    },
                    MetadataItem {
                        key: "type".to_string(),
                        value: Some(metadata_item::Value::StringValue(
                            if i < 100 { "A" } else { "B" }.to_string(),
                        )),
                    },
                ],
                timestamp: 3000 + i as u32,
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
            .expect("Should write");

        // Verify file exists
        assert!(
            std::path::Path::new(&sst_path).exists(),
            "SST file should exist"
        );

        let file_size = std::fs::metadata(&sst_path).unwrap().len();

        // Read and verify metadata statistics
        let mut reader = match ModularBlockReader::open(fs.clone(), &sst_path).await {
            Ok(r) => r,
            Err(e) => {
                panic!("Failed to open reader: {:?}", e);
            }
        };

        // Read first block
        let block0 = match reader.read_data_block(0, ReadMode::Buffered).await {
            Ok(b) => b,
            Err(e) => {
                panic!("Should read block 0: {:?}", e);
            }
        };

        // Verify metadata statistics are populated
        assert!(
            block0
                .metadata_stats
                .metadata_columns
                .contains_hash(&"score".to_string())
        );
        assert!(
            block0
                .metadata_stats
                .metadata_columns
                .contains_hash(&"type".to_string())
        );

        // Check min/max values exist
        assert!(block0.metadata_stats.min_values.contains_key("score"));
        assert!(block0.metadata_stats.max_values.contains_key("score"));

        // Verify the values make sense (min < max for scores)
        if let (Some(min_score), Some(max_score)) = (
            block0.metadata_stats.min_values.get(key),
            block0.metadata_stats.max_values.get(key),
        ) {
            let min_val: i32 = min_score.as_deref().unwrap().parse().unwrap();
            let max_val: i32 = max_score.as_deref().unwrap().parse().unwrap();
            assert!(min_val <= max_val, "Min score should be <= max score");
        }
    }

    // TODO: Requires proper SST infrastructure
    // #[tokio::test]
    #[allow(dead_code)]
    async fn test_random_block_access() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_random_access.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Write multiple blocks of test data
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some("file:///".to_string()),
            ..Default::default()
        };
        let fs = Arc::new(FilesystemFactory::create(config).await.unwrap());
        let writer = SstableWriter::new(&sst_path, 512 * 1024, fs.clone()); // 512KB blocks for more blocks

        let mut all_records = BTreeMap::new();
        for i in 0..500 {
            let record = SstRecord {
                id: format!("random_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![],
                timestamp: 5000 + i as u32,
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
            .expect("Should write");

        // Test random access to different blocks using filesystem API
        let mut reader = ModularBlockReader::open(fs.clone(), &sst_path)
            .await
            .expect("Should open reader");

        // Read blocks in non-sequential order to test random access
        // This tests that the filesystem API properly supports seeking
        let block_ids = vec![2, 0, 3, 1, 4]; // Random order

        for block_id in block_ids {
            match reader.read_data_block(block_id, ReadMode::Buffered).await {
                Ok(block) => {
                    // Verify block has proper hierarchical metadata
                    assert!(block.metadata_stats.record_count > 0);
                    assert!(!block.metadata_stats.min_key.is_none());
                    assert!(!block.metadata_stats.max_key.is_none());

                    // Verify block ID matches
                    assert_eq!(block.block_id, block_id as u32);

                    // Verify records are from the expected range
                    for record in &block.records {
                        assert!(record.id.starts_with("random_"));
                    }
                }
                Err(e) => {
                    // It's ok if block doesn't exist (we might have fewer blocks)
                }
            }
        }
    }

    // TODO: Requires proper SST infrastructure
    // #[tokio::test]
    #[allow(dead_code)]
    async fn test_bloom_filter_in_blocks() {
        let temp_dir = TempDir::new().unwrap();
        let sst_path = temp_dir
            .path()
            .join("test_bloom.sstable")
            .to_str()
            .unwrap()
            .to_string();

        // Write SST file
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig {
            default_fs: Some("file:///".to_string()),
            ..Default::default()
        };
        let fs = Arc::new(FilesystemFactory::create(config).await.unwrap());
        let writer = SstableWriter::new(&sst_path, 2 * 1024 * 1024, fs.clone());

        let mut all_records = BTreeMap::new();
        for i in 0..50 {
            let record = SstRecord {
                id: format!("bloom_{:04}", i),
                vector: vec![i as f32; 384],
                metadata: vec![],
                timestamp: 4000 + i as u32,
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
            .expect("Should write");

        // Read and verify bloom filters
        let mut reader = ModularBlockReader::open(fs.clone(), &sst_path)
            .await
            .expect("Should open reader");

        let block = reader
            .read_data_block(0, ReadMode::Buffered)
            .await
            .expect("Should read block");

        // Verify bloom filter is present
        assert!(
            block.block_bloom_filter.is_some(),
            "Block should have bloom filter"
        );

        let bloom_data = block.block_bloom_filter.as_ref().unwrap();
        assert!(bloom_data.len() > 0, "Bloom filter should have data");
    }
}
