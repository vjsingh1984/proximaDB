#[cfg(test)]
mod tests {
    use crate::storage::engines::sst::SstableWriter;
    use proximadb_data_model::ProximaValue;
    use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    fn test_record(id: impl Into<String>, vector: Vec<f32>) -> ProximaRecord {
        let id = id.into();
        ProximaRecord {
            oid: id.clone(),
            local_id: Some(id),
            record_version: 1,
            created_at_ns: 123_456_789_000_000,
            updated_at_ns: 123_456_789_000_000,
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: vector.len() as u32,
                values: proximadb_records::EmbeddingValues::Fp32(vector),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        }
    }

    #[tokio::test]
    async fn test_sstable_writer_basic() {
        // Note: This test would need a mock filesystem for full testing
        // For now, just test the data structure building
        let _temp_file = NamedTempFile::new().unwrap();

        // Create test records
        let mut records = BTreeMap::new();
        for i in 0..10 {
            let record = test_record(format!("key{:03}", i), vec![1.0, 2.0, 3.0]);
            records.insert(record.oid.clone(), record);
        }

        assert_eq!(records.len(), 10);
    }

    /// Regression: SstableWriter MUST NOT emit Vector Object Economy
    /// directory updates by default. Opt-in is via
    /// `with_directory_emission`. Flipping this default would silently
    /// touch the read-side cache for every existing call site — a
    /// behaviour change masquerading as a no-op refactor.
    #[tokio::test]
    async fn sstable_writer_default_does_not_emit_directory() {
        use crate::storage::engines::sst::SstableWriter;
        use crate::storage::engines::sst::object_economy_directory::{
            SstableWriterDirectoryHooks, VectorObjectEconomyDirectoryCache,
        };
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use proximadb_catalog::CatalogAuthorityMode;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let fs = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap(),
        );

        // Default construction: no directory emission configured.
        let writer = SstableWriter::new_with_config(
            temp.path().join("test.sst"),
            64 * 1024,
            fs.clone(),
            None,
        );
        assert!(
            !writer.directory_emission_configured(),
            "freshly-constructed writer must NOT emit directory updates"
        );

        // Opt in via builder — accessor flips.
        let cache = Arc::new(VectorObjectEconomyDirectoryCache::new());
        let writer_opted_in = SstableWriter::new_with_config(
            temp.path().join("test.sst"),
            64 * 1024,
            fs,
            None,
        )
        .with_directory_emission(SstableWriterDirectoryHooks {
            cache,
            collection_id: "coll".to_string(),
            collection_root: "file:///tmp/coll".to_string(),
            storage_epoch: 1,
            authority_mode: CatalogAuthorityMode::RebuildableProjection,
            freshness_lsn: 0,
            level: 0,
        });
        assert!(writer_opted_in.directory_emission_configured());
    }

    #[tokio::test]
    async fn test_sstable_write_read_format() {
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
        let record = test_record("test_vec", vec![1.0, 2.0, 3.0]);
        records.insert(record.oid.clone(), record);

        // Write SSTable
        let writer = SstableWriter::new(
            &sstable_path,
            4096, // block size
            filesystem.clone(),
        );

        // Write records using streaming approach for production consistency
        let record_count = records.len();
        let sorted_records_iter = records.into_values();
        writer
            .write_sorted_proxima_records(sorted_records_iter, record_count)
            .await
            .expect("Failed to write SSTable");

        // Verify file exists
        let fs = filesystem.get_filesystem("file:///").unwrap();
        assert!(fs.exists(sstable_path.to_str().unwrap()).await.unwrap());
    }

    #[tokio::test]
    async fn test_sstable_format_inspection() {
        use crate::storage::engines::sst::readers::sst_query_engine::SstDirectReader;
        use crate::storage::persistence::filesystem::FilesystemConfig;
        use crate::storage::persistence::filesystem::FilesystemFactory;
        use tempfile::TempDir;

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
            let mut record = test_record(format!("vec_{}", i), vec![i as f32; 3]);
            record.props.insert(
                "category".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(format!("cat_{}", i % 2))),
            );

            records.insert(record.oid.clone(), record);
        }

        // Write SSTable
        let writer = SstableWriter::new(&sstable_path, 4096, filesystem.clone());

        // Write records using streaming approach for production consistency
        let record_count = records.len();
        let sorted_records_iter = records.into_values();
        writer
            .write_sorted_proxima_records(sorted_records_iter, record_count)
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
        let first_vector = read_vectors.iter().find(|v| v.oid == "vec_0");
        assert!(
            first_vector.is_some(),
            "Should find vec_0 in {} vectors",
            read_vectors.len()
        );
        if let Some(vec) = first_vector {
            let values = vec
                .embeddings
                .first()
                .map(|embedding| embedding.as_fp32_slice())
                .unwrap_or(&[]);
            assert_eq!(values.len(), 3, "Vector should have 3 dimensions");
            assert_eq!(values, &[0.0, 0.0, 0.0], "Vector values should match");
        }
    }
}
