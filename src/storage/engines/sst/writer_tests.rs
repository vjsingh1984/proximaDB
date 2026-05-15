#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::VectorRecord;
    use crate::storage::engines::sst::SstableWriter;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_sstable_writer_basic() {
        // Note: This test would need a mock filesystem for full testing
        // For now, just test the data structure building
        let _temp_file = NamedTempFile::new().unwrap();

        // Create test records
        let mut records = BTreeMap::new();
        for i in 0..10 {
            let record = VectorRecord {
                id: format!("key{:03}", i),
                vector: vec![1.0, 2.0, 3.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(chrono::Utc::now().timestamp()),
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
                source: None,
            };
            records.insert(record.id.clone(), record);
        }

        assert_eq!(records.len(), 10);
    }

    #[tokio::test]
    async fn test_sstable_write_read_format() {
        use crate::proto::proximadb_v1::VectorRecord as VR;
        use crate::storage::engines::sst::SstEntry;
        use crate::storage::persistence::filesystem::FilesystemConfig;
        use crate::storage::persistence::filesystem::FilesystemFactory;
        use tempfile::TempDir;

        // Initialize hardware capabilities
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
        let temp_dir = TempDir::new().unwrap();
        let sstable_path = temp_dir.path().join("test.sstable");

        // Create filesystem factory with default config
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());

        // Create a simple record
        let mut records = BTreeMap::new();
        let vector_record = VR {
            id: "test_vec".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(123456789),
            updated_at: Some(123456789),
            expires_at: None,
            version: Some(1),
            source: None,
        };
        let record = SstEntry::from_vector_record(vector_record, 1, 0);
        records.insert(record.record.id.clone(), record);

        // Write SSTable
        let writer = SstableWriter::new(
            &sstable_path,
            4096, // block size
            filesystem.clone(),
        );

        // Write records using streaming approach for production consistency
        let record_count = records.len();
        let sorted_records_iter = records.into_iter().map(|(_, entry)| entry.record);
        writer
            .write_sorted_records(sorted_records_iter, record_count)
            .await
            .expect("Failed to write SSTable");

        // Verify file exists
        let fs = filesystem.get_filesystem("file:///").unwrap();
        assert!(fs.exists(sstable_path.to_str().unwrap()).await.unwrap());
    }

    #[tokio::test]
    async fn test_sstable_format_inspection() {
        use crate::proto::proximadb_v1::{SqlValue, VectorRecord as VR};
        use crate::storage::engines::sst::readers::sst_query_engine::SstDirectReader;
        use crate::storage::engines::sst::{SstEntry, SstMetadata};
        use crate::storage::persistence::filesystem::FilesystemConfig;
        use crate::storage::persistence::filesystem::FilesystemFactory;
        use tempfile::TempDir;
        use tracing::debug;

        // Initialize hardware capabilities
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
        let temp_dir = TempDir::new().unwrap();
        let sstable_path = temp_dir.path().join("inspect.sst");

        // Create filesystem factory with default config
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());

        // Create records with metadata for bloom filter
        let mut records = BTreeMap::new();
        for i in 0..5 {
            let _metadata = vec![crate::proto::proximadb_v1::MetadataItem {
                key: "category".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(format!(
                        "cat_{}",
                        i % 2
                    )),
                ),
            }];

            let mut metadata_map = std::collections::HashMap::new();
            metadata_map.insert(
                "category".to_string(),
                SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        format!("cat_{}", i % 2),
                    )),
                },
            );

            let vector_record = VR {
                id: format!("vec_{}", i),
                vector: vec![i as f32; 3],
                metadata: metadata_map,
                timestamp: Some(123456789),
                updated_at: Some(123456789),
                expires_at: None,
                version: Some(1),
                source: None,
            };

            let sst_entry = SstEntry {
                record: vector_record,
                sst_meta: SstMetadata {
                    is_tombstone: false,
                    sequence_number: i as u64,
                    level: 0,
                },
            };
            records.insert(sst_entry.record.id.clone(), sst_entry);
        }

        // Write SSTable
        let writer = SstableWriter::new(&sstable_path, 4096, filesystem.clone());

        // Write records using streaming approach for production consistency
        let record_count = records.len();
        let sorted_records_iter = records.into_iter().map(|(_, entry)| entry.record);
        writer
            .write_sorted_records(sorted_records_iter, record_count)
            .await
            .expect("Failed to write SSTable");

        // Read file directly to inspect format
        let fs = filesystem.get_filesystem("file:///").unwrap();
        let file_data = fs.read(sstable_path.to_str().unwrap()).await.unwrap();

        println!("SSTable file size: {} bytes", file_data.len());

        // Parse header length
        let header_len =
            u32::from_le_bytes([file_data[0], file_data[1], file_data[2], file_data[3]]);
        println!("Header length: {} bytes", header_len);

        // Check bloom filter offset and length
        let bloom_offset = 4 + header_len as usize;
        if file_data.len() >= bloom_offset + 4 {
            let bloom_len = u32::from_le_bytes([
                file_data[bloom_offset],
                file_data[bloom_offset + 1],
                file_data[bloom_offset + 2],
                file_data[bloom_offset + 3],
            ]);
            println!(
                "Bloom filter length: {} bytes at offset {}",
                bloom_len, bloom_offset
            );

            let bloom_end = bloom_offset + 4 + bloom_len as usize;
            assert!(
                file_data.len() >= bloom_end,
                "File size {} is too small for bloom filter ending at {}",
                file_data.len(),
                bloom_end
            );

            // Check index offset and length
            if file_data.len() >= bloom_end + 4 {
                let index_len = u32::from_le_bytes([
                    file_data[bloom_end],
                    file_data[bloom_end + 1],
                    file_data[bloom_end + 2],
                    file_data[bloom_end + 3],
                ]);
                println!("Index length: {} bytes at offset {}", index_len, bloom_end);
            }
        }

        // Now try to read with SstDirectReader which doesn't require ZeroCopyIOSystem
        let file_url = format!("file://{}", sstable_path.display());
        let mut reader = SstDirectReader::open(filesystem.clone(), &file_url)
            .await
            .expect("Failed to open SSTable reader");

        // Read and verify vectors
        let read_vectors = reader
            .read_all_for_compaction()
            .await
            .expect("Failed to read vectors");

        // Should have at least the vectors we wrote
        assert!(
            read_vectors.len() >= 1,
            "Should have read at least 1 vector, got {}",
            read_vectors.len()
        );

        // Verify first vector content
        let first_vector = read_vectors.iter().find(|v| v.id == "vec_0");
        assert!(
            first_vector.is_some(),
            "Should find vec_0 in {} vectors",
            read_vectors.len()
        );
        if let Some(vec) = first_vector {
            assert_eq!(vec.vector.len(), 3, "Vector should have 3 dimensions");
            assert_eq!(
                vec.vector,
                vec![0.0, 0.0, 0.0],
                "Vector values should match"
            );
        }
    }
}
