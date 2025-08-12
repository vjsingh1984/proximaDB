// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Tests for zero-copy SST compactor

#[cfg(test)]
mod tests {
    use super::super::sst_compactor::{SstCompactor, CompactionSortStrategy, ZeroCopyCompactionStats};
    use super::super::{SstRecord, SstableWriter};
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
    use crate::core::search::mvcc_resolution::MvccResolver;
    use crate::proto::proximadb::MetadataItem;
    use std::sync::Arc;
    use tempfile::TempDir;
    use std::path::PathBuf;

    /// Helper to create a test SstRecord
    fn create_test_record(
        id: String,
        version: Option<u32>,
        timestamp: u32,
        expires_at: Option<u32>,
        is_tombstone: bool,
    ) -> SstRecord {
        SstRecord {
            id,
            vector: vec![1.0, 2.0, 3.0],
            metadata: vec![],
            timestamp,
            expires_at,
            version,
            is_tombstone,
            level: 0,
            sequence_number: timestamp as u64,
            collection_id: "test_collection".to_string(),
        }
    }

    /// Helper to create an SST file with test records
    async fn create_test_sst_file(
        filesystem_factory: Arc<FilesystemFactory>,
        path: &str,
        records: Vec<SstRecord>,
    ) -> anyhow::Result<()> {
        let writer = SstableWriter::new(
            path,
            4096, // block_size
            filesystem_factory,
        );

        let sorted_records: Vec<(String, SstRecord)> = records
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();

        let record_count = sorted_records.len();
        writer.write_sorted_records(
            sorted_records.into_iter(),
            record_count,
        ).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_basic_compaction() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        // Create test SST files
        let file1_path = temp_dir.path().join("file1.sst").to_string_lossy().to_string();
        let file2_path = temp_dir.path().join("file2.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        // File 1: Records with IDs A and B
        let records1 = vec![
            create_test_record("A".to_string(), Some(1), 100, None, false),
            create_test_record("B".to_string(), Some(1), 150, None, false),
        ];

        // File 2: Updated versions of A and B
        let records2 = vec![
            create_test_record("A".to_string(), Some(2), 200, None, false),
            create_test_record("B".to_string(), Some(2), 250, None, false),
        ];

        create_test_sst_file(filesystem_factory.clone(), &file1_path, records1).await.unwrap();
        create_test_sst_file(filesystem_factory.clone(), &file2_path, records2).await.unwrap();

        // Create compactor and perform compaction
        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file1_path, file2_path],
            output_path.clone(),
            1, // target_level
        ).await.unwrap();

        // Verify stats
        assert_eq!(stats.records_read, 4);
        assert_eq!(stats.records_written, 2); // Should keep highest versions
        assert_eq!(stats.files_compacted, 2);
        assert!(stats.updated_vector_ids.contains(&"A".to_string()));
        assert!(stats.updated_vector_ids.contains(&"B".to_string()));

        // Verify output file exists
        let fs = filesystem_factory.get_filesystem("file:///").unwrap();
        assert!(fs.exists(&output_path).await.unwrap());
    }

    #[tokio::test]
    async fn test_version_continuity() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        let file_path = temp_dir.path().join("versions.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        // Create records with version gaps
        let records = vec![
            create_test_record("continuous".to_string(), Some(1), 100, None, false),
            create_test_record("continuous".to_string(), Some(2), 200, None, false),
            create_test_record("continuous".to_string(), Some(3), 300, None, false),
            create_test_record("gap".to_string(), Some(1), 150, None, false),
            create_test_record("gap".to_string(), Some(3), 350, None, false), // Missing v2
        ];

        create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file_path],
            output_path,
            1,
        ).await.unwrap();

        // "continuous" should have v3, "gap" should only have v1 (stopped at gap)
        assert_eq!(stats.records_written, 2);
        assert!(stats.updated_vector_ids.contains(&"continuous".to_string()));
    }

    #[tokio::test]
    async fn test_tombstone_handling() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        let file_path = temp_dir.path().join("tombstones.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        // Create records with tombstones
        let records = vec![
            create_test_record("alive".to_string(), Some(1), 100, None, false),
            create_test_record("alive".to_string(), Some(2), 200, None, false),
            create_test_record("deleted".to_string(), Some(1), 150, None, false),
            create_test_record("deleted".to_string(), Some(2), 250, None, true), // Tombstone
        ];

        create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file_path],
            output_path,
            1,
        ).await.unwrap();

        // Only "alive" should remain
        assert_eq!(stats.records_written, 1);
        assert!(stats.tombstoned_ids.contains(&"deleted".to_string()));
        assert_eq!(stats.records_deleted, 2); // Both versions of "deleted"
    }

    #[tokio::test]
    async fn test_expired_records() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        let file_path = temp_dir.path().join("expired.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        let now = chrono::Utc::now().timestamp() as u32;

        // Create records with expiry
        let records = vec![
            create_test_record("valid".to_string(), Some(1), 100, None, false),
            create_test_record("expired".to_string(), Some(1), 150, Some(now - 1000), false), // Expired
        ];

        create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file_path],
            output_path,
            1,
        ).await.unwrap();

        // Only "valid" should remain
        assert_eq!(stats.records_written, 1);
        assert!(stats.deleted_vector_ids.contains(&"expired".to_string()));
    }

    #[tokio::test]
    async fn test_append_only_records() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        let file_path = temp_dir.path().join("append_only.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        // Create append-only records (no ID or special IDs)
        let records = vec![
            create_test_record("".to_string(), None, 100, None, false),
            create_test_record("null".to_string(), None, 200, None, false),
            create_test_record("none".to_string(), None, 300, None, false),
            create_test_record("  ".to_string(), None, 400, None, false),
        ];

        create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file_path],
            output_path,
            1,
        ).await.unwrap();

        // All append-only records should be kept
        assert_eq!(stats.records_written, 4);
        assert_eq!(stats.records_read, 4);
    }

    #[tokio::test]
    async fn test_version_normalization() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        let file_path = temp_dir.path().join("versions.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        // Create records with None and 0 versions (should be treated as 1)
        let records = vec![
            create_test_record("test1".to_string(), None, 100, None, false),    // None -> 1
            create_test_record("test1".to_string(), Some(2), 200, None, false),
            create_test_record("test2".to_string(), Some(0), 150, None, false),  // 0 -> 1
            create_test_record("test2".to_string(), Some(2), 250, None, false),
        ];

        create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file_path],
            output_path,
            1,
        ).await.unwrap();

        // Both records should have version 2 selected
        assert_eq!(stats.records_written, 2);
        assert_eq!(stats.updated_vector_ids.len(), 2);
    }

    #[tokio::test]
    async fn test_sorting_strategies() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        // Test ByTimestamp sorting
        {
            let file_path = temp_dir.path().join("timestamp.sst").to_string_lossy().to_string();
            let output_path = temp_dir.path().join("timestamp_out.sst").to_string_lossy().to_string();

            let records = vec![
                create_test_record("A".to_string(), Some(1), 300, None, false),
                create_test_record("B".to_string(), Some(1), 100, None, false),
                create_test_record("C".to_string(), Some(1), 200, None, false),
            ];

            create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

            let mvcc_resolver = Arc::new(MvccResolver::new());
            let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver))
                .with_sort_strategy(CompactionSortStrategy::ByTimestamp);

            let stats = compactor.compact_files(
                vec![file_path],
                output_path,
                1,
            ).await.unwrap();

            assert_eq!(stats.records_written, 3);
        }

        // Test ByMetadata sorting
        {
            let file_path = temp_dir.path().join("metadata.sst").to_string_lossy().to_string();
            let output_path = temp_dir.path().join("metadata_out.sst").to_string_lossy().to_string();

            let mut record1 = create_test_record("A".to_string(), Some(1), 100, None, false);
            record1.metadata.push(MetadataItem {
                key: "priority".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("high".to_string())),
            });

            let mut record2 = create_test_record("B".to_string(), Some(1), 200, None, false);
            record2.metadata.push(MetadataItem {
                key: "priority".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("low".to_string())),
            });

            let records = vec![record1, record2];

            create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

            let mvcc_resolver = Arc::new(MvccResolver::new());
            let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver))
                .with_sort_strategy(CompactionSortStrategy::ByMetadata(vec!["priority".to_string()]));

            let stats = compactor.compact_files(
                vec![file_path],
                output_path,
                1,
            ).await.unwrap();

            assert_eq!(stats.records_written, 2);
        }
    }

    #[tokio::test]
    async fn test_multiple_file_merge() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        // Create 3 SST files with overlapping data
        let mut input_files = Vec::new();
        
        for i in 0..3 {
            let file_path = temp_dir.path().join(format!("file{}.sst", i)).to_string_lossy().to_string();
            
            let records = vec![
                create_test_record(format!("shared_{}", i), Some(1), 100 + i * 100, None, false),
                create_test_record(format!("unique_{}", i), Some(1), 150 + i * 100, None, false),
            ];
            
            create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();
            input_files.push(file_path);
        }

        let output_path = temp_dir.path().join("merged.sst").to_string_lossy().to_string();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            input_files,
            output_path,
            1,
        ).await.unwrap();

        assert_eq!(stats.files_compacted, 3);
        assert_eq!(stats.records_read, 6);
        assert_eq!(stats.records_written, 6); // All unique records
    }

    #[tokio::test]
    async fn test_index_rebuild_recommendation() {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );

        let file_path = temp_dir.path().join("many_changes.sst").to_string_lossy().to_string();
        let output_path = temp_dir.path().join("output.sst").to_string_lossy().to_string();

        // Create many records with tombstones and expired entries
        let mut records = Vec::new();
        for i in 0..10 {
            // Half will be tombstoned
            let is_tombstone = i >= 5;
            records.push(create_test_record(
                format!("record_{}", i),
                Some(1),
                100 + i * 10,
                None,
                is_tombstone,
            ));
        }

        create_test_sst_file(filesystem_factory.clone(), &file_path, records).await.unwrap();

        let mvcc_resolver = Arc::new(MvccResolver::new());
        let compactor = SstCompactor::new(filesystem_factory.clone(), Some(mvcc_resolver));

        let stats = compactor.compact_files(
            vec![file_path],
            output_path,
            1,
        ).await.unwrap();

        // With 50% changes, should recommend index rebuild
        assert!(stats.recommend_index_rebuild);
        assert_eq!(stats.tombstoned_ids.len(), 5);
    }
}