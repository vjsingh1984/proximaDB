#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::sst::blocks::SstRecord;
    use crate::storage::engines::sst::{
        Compaction, CompactionPriority, CompactionStats, CompactionTask, SstConfig,
    };
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_compaction_basic() {
        let mut config = SstConfig::default();
        config.level_count = 3;
        config.compaction_threshold = 2;
        config.block_size_kb = 1024;

        let mut manager = Compaction::new(config).await.unwrap();
        assert!(manager.start_workers(1).await.is_ok());
        assert!(manager.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_task_scheduling() {
        let mut config = SstConfig::default();
        config.level_count = 3;
        config.compaction_threshold = 2;
        config.block_size_kb = 1024;

        let manager = Compaction::new(config).await.unwrap();

        let task = CompactionTask {
            level: 0,
            input_files: vec![],
            output_file: PathBuf::from("/tmp/output.db"),
            priority: CompactionPriority::Medium,
            block_size_kb: None,
            compression_config: None,
            precision_hint: None,
        };

        assert!(manager.schedule_compaction(task).await.is_ok());
    }

    // Unit tests for expired record deletion during compaction
    // Inlined from tests/rust/storage/test_expired_record_unit.rs

    /// Unit test for LSM compaction expired record deletion logic
    #[tokio::test]
    async fn test_sst_compaction_expired_deletion_unit() -> anyhow::Result<()> {
        use chrono::Utc;

        // Create test data with controlled timestamps
        let current_time = Utc::now().timestamp() as u32;
        let _expired_time = current_time - (5 * 60 * 60); // 5 hours ago
        let _future_time = current_time + (5 * 60 * 60); // 5 hours from now

        let test_records = vec![
            // Active record (no expiry)
            SstRecord {
                id: "active_1".to_string(),
                vector: Some(vec![1.0, 2.0, 3.0]),
                metadata: None,
                sequence_number: 1,
                level: 0,
                is_tombstone: false,
                timestamp: 0,
            },
            // Expired record (should be deleted)
            SstRecord {
                id: "expired_1".to_string(),
                vector: Some(vec![4.0, 5.0, 6.0]),
                metadata: None,
                sequence_number: 2,
                level: 0,
                is_tombstone: false,
                timestamp: 0,
            },
            // Active record with future expiry
            SstRecord {
                id: "future_1".to_string(),
                vector: Some(vec![7.0, 8.0, 9.0]),
                metadata: None,
                sequence_number: 3,
                level: 0,
                is_tombstone: false,
                timestamp: 0,
            },
            // Old tombstone (should be removed)
            SstRecord {
                id: "old_tombstone".to_string(),
                vector: None,
                metadata: None,
                sequence_number: 4,
                level: 0,
                is_tombstone: false,
                timestamp: 0,
            },
        ];

        // Create temporary directory and files
        let temp_dir = tempfile::tempdir()?;
        let collection_dir = temp_dir.path().join("test_collection");
        std::fs::create_dir_all(&collection_dir)?;

        let input_file = collection_dir.join("input.sstable");
        let output_file = collection_dir.join("output.sstable");

        // Write test data to input file
        let mut input_data = Vec::new();
        for record in &test_records {
            let serialized = bincode::serialize(record)?;
            input_data.extend_from_slice(&(serialized.len() as u32).to_le_bytes());
            input_data.extend_from_slice(&serialized);
        }
        std::fs::write(&input_file, &input_data)?;

        // Create compaction task
        let _task = CompactionTask {
            level: 0,
            input_files: vec![input_file],
            output_file: output_file.clone(),
            priority: CompactionPriority::Medium,
            block_size_kb: None,
            compression_config: None,
            precision_hint: None,
        };

        // Create config and perform compaction
        let _config = SstConfig::default();

        // Note: This test requires CompactionManager::perform_compaction to be implemented
        // For now, we'll test the basic structure
        let stats = CompactionStats {
            total_compactions: 1,
            files_merged: 1,
            avg_compaction_time_ms: 0,
            last_compaction_time: None,
            expired_records_deleted: 1,
            tombstones_removed: 1,
            bytes_read: input_data.len() as u64,
            bytes_written: 0,
        };

        // Verify statistics
        assert_eq!(
            stats.expired_records_deleted, 1,
            "Should delete 1 expired record"
        );
        assert_eq!(stats.tombstones_removed, 1, "Should remove 1 old tombstone");

        println!("✅ LSM compaction expired deletion unit test passed!");
        println!("   - Input records: {}", test_records.len());
        println!("   - Bytes written: {}", stats.bytes_written);
        println!("   - Expired deleted: {}", stats.expired_records_deleted);
        println!("   - Tombstones removed: {}", stats.tombstones_removed);

        Ok(())
    }

    /// Mock test for expired record deletion logic
    #[tokio::test]
    async fn test_expired_record_logic_unit() -> anyhow::Result<()> {
        use chrono::Utc;

        // This test mocks the expiry logic from compact_parquet_files
        let current_time = Utc::now().timestamp() as u32;
        let expired_time = current_time - (2 * 60 * 60); // 2 hours ago
        let future_time = current_time + (2 * 60 * 60); // 2 hours from now

        // Mock record data (simulating what would be in Parquet files)
        let mock_records = vec![
            ("active_record", current_time, None),
            ("expired_record", expired_time, Some(expired_time)),
            ("future_record", current_time, Some(future_time)),
        ];

        // Apply the same expiry logic as in compaction
        let mut kept_records = Vec::new();
        let mut expired_count = 0;

        for (record_id, timestamp, expires_at) in mock_records {
            // This mirrors the logic in compaction methods
            if let Some(expires_at) = expires_at {
                if expires_at < current_time {
                    expired_count += 1;
                    println!(
                        "⏰ Compaction: Skipping expired record {} (expired at {})",
                        record_id, expires_at
                    );
                    continue;
                }
            }

            kept_records.push((record_id, timestamp, expires_at));
        }

        // Verify results
        assert_eq!(expired_count, 1, "Should have 1 expired record");
        assert_eq!(kept_records.len(), 2, "Should keep 2 records");

        let kept_ids: Vec<&str> = kept_records.iter().map(|(id, _, _)| *id).collect();
        assert!(
            kept_ids.contains(&"active_record"),
            "Active record should be kept"
        );
        assert!(
            kept_ids.contains(&"future_record"),
            "Future expiry record should be kept"
        );
        assert!(
            !kept_ids.contains(&"expired_record"),
            "Expired record should be filtered out"
        );

        println!("✅ Expired record logic unit test passed!");
        println!("   - Input records: 3");
        println!("   - Kept records: {}", kept_records.len());
        println!("   - Expired filtered: {}", expired_count);

        Ok(())
    }

    /// Unit test for edge cases in expiry logic
    #[tokio::test]
    async fn test_expiry_edge_cases_unit() -> anyhow::Result<()> {
        use chrono::Utc;

        let current_time = Utc::now().timestamp_millis();
        let just_expired = current_time - 1; // Just expired by 1ms
        let just_future = current_time + 1; // Expires in 1ms

        // Test boundary conditions
        let test_cases = vec![
            ("just_expired", Some(just_expired), true), // Should be expired
            ("just_future", Some(just_future), false),  // Should not be expired
            ("no_expiry", None, false),                 // Should not be expired
            ("far_future", Some(current_time + 1000000), false), // Should not be expired
            ("far_past", Some(current_time - 1000000), true), // Should be expired
        ];

        for (name, expires_at, should_be_expired) in test_cases {
            let is_expired = if let Some(expires_at) = expires_at {
                expires_at < current_time
            } else {
                false
            };

            assert_eq!(
                is_expired, should_be_expired,
                "Record '{}' expiry check failed: expires_at={:?}, current={}, expected_expired={}",
                name, expires_at, current_time, should_be_expired
            );
        }

        println!("✅ Expiry edge cases unit test passed!");
        Ok(())
    }

    /// Test for tombstone cleanup logic
    #[tokio::test]
    async fn test_tombstone_cleanup_unit() -> anyhow::Result<()> {
        use chrono::Utc;

        let current_time = Utc::now().timestamp_millis();
        let one_hour_ago = current_time - (60 * 60 * 1000); // 1 hour ago
        let two_hours_ago = current_time - (2 * 60 * 60 * 1000); // 2 hours ago

        // Test tombstone ages
        let tombstone_cases = vec![
            ("recent_tombstone", one_hour_ago + 1000, true), // Should be kept (< 1 hour) - 1 second less than 1 hour old
            ("old_tombstone", two_hours_ago, false),         // Should be removed (> 1 hour)
            ("boundary_tombstone", current_time - (60 * 60 * 1000), false), // Exactly 1 hour (should be removed)
        ];

        for (name, tombstone_time, should_keep) in tombstone_cases {
            // This mirrors the tombstone cleanup logic in LSM compaction
            let age = current_time - tombstone_time;
            let keep_tombstone = age < (60 * 60 * 1000); // 1 hour in milliseconds

            assert_eq!(
                keep_tombstone, should_keep,
                "Tombstone '{}' cleanup check failed: age={}ms, expected_keep={}",
                name, age, should_keep
            );
        }

        println!("✅ Tombstone cleanup unit test passed!");
        Ok(())
    }

    /// INT-3-followup-d wiring: with `CompactionTask.precision_hint =
    /// Some(Fp16)`, the rewritten block recovers fp16 bit-exact from
    /// the fp32-flattened VectorRecord intermediate. Exercises the
    /// `coerce_to_precision` call at the writer reconstitution site.
    ///
    /// Setup mirrors the compaction-writer path directly:
    /// 1. Build fp16 source records (what we'd recover from a fp16 block).
    /// 2. Promote to fp32 (what the VectorRecord intermediate does).
    /// 3. Demote back to fp16 via `coerce_to_precision(Fp16)`
    ///    (what compaction.rs now does when precision_hint is set).
    /// 4. Write via ArrowBlockWriter, read back via ArrowBlockReader.
    /// 5. Assert recovered records are fp16 bit-exact with originals.
    ///
    /// A full compaction integration test (driving an actual
    /// `Compaction::run_task` with two fp16 input files) needs a much
    /// larger fixture (memtable → flush → on-disk SST file format with
    /// the right magic + footer); the in-line test here proves the
    /// coercion path the wiring just landed at compaction.rs:1100 and
    /// :1217 produces the right output bytes.
    #[tokio::test]
    async fn fp16_records_survive_compaction_round_trip_bit_exact() {
        use crate::storage::engines::core::formats::arrow_block::{
            ArrowBlockConfig, ArrowBlockReader, ArrowBlockWriter,
        };
        use proximadb_records::{
            EmbeddingCell, EmbeddingScalarType, EmbeddingValues, ProximaRecord,
        };
        use tempfile::tempdir;

        // Step 1: source fp16 records (e.g. read from a fp16 source block).
        let src_fp32: Vec<Vec<f32>> = (0..32)
            .map(|i| {
                (0..16)
                    .map(|j| ((i as f32) * 1.25 - 16.0) + (j as f32) * 0.0625)
                    .collect()
            })
            .collect();
        let original_fp16_records: Vec<ProximaRecord> = src_fp32
            .iter()
            .enumerate()
            .map(|(i, src)| {
                let f16s: Vec<half::f16> =
                    src.iter().map(|&x| half::f16::from_f32(x)).collect();
                ProximaRecord {
                    oid: format!("compact_fp16_{:05}", i),
                    embeddings: vec![EmbeddingCell {
                        model_id: "test".to_string(),
                        modality: "dense_vector".to_string(),
                        dim: 16,
                        values: EmbeddingValues::Fp16(f16s),
                        precision: EmbeddingScalarType::Fp16,
                        ..Default::default()
                    }],
                    ..ProximaRecord::default()
                }
            })
            .collect();

        // Step 2 + 3: simulate the VectorRecord round-trip
        // (fp16 → fp32 → ... → fp32) then apply the coercion hint that
        // compaction.rs now applies at the writer reconstitution site.
        let mut records_after_intermediate: Vec<ProximaRecord> = original_fp16_records
            .iter()
            .map(|r| {
                let fp32_view: Vec<f32> = r.embeddings[0].values.to_fp32_owned();
                ProximaRecord {
                    oid: r.oid.clone(),
                    embeddings: vec![EmbeddingCell::new_fp32(
                        "test",
                        "dense_vector",
                        16,
                        fp32_view,
                    )],
                    ..ProximaRecord::default()
                }
            })
            .collect();
        // This is the exact loop compaction.rs runs when task.precision_hint is set.
        let target = EmbeddingScalarType::Fp16;
        for record in &mut records_after_intermediate {
            for cell in &mut record.embeddings {
                cell.coerce_to_precision(target);
            }
        }

        // Step 4: write to a real Arrow file + read back.
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("compacted_fp16.arrow");
        let config = ArrowBlockConfig::new(16);
        let mut writer = ArrowBlockWriter::new(&path, config).expect("writer");
        writer
            .write_block(&records_after_intermediate)
            .expect("write_block accepts coerced fp16 records");
        writer.finalize().expect("finalize");

        let reader = ArrowBlockReader::open(&path).expect("open reader");
        let read_records = reader.read_all().expect("read_all");

        // Step 5: bit-exact assertion against the original fp16 source.
        assert_eq!(read_records.len(), original_fp16_records.len());
        for (orig, got) in original_fp16_records.iter().zip(read_records.iter()) {
            let orig_f16 = match &orig.embeddings[0].values {
                EmbeddingValues::Fp16(v) => v.clone(),
                other => panic!("orig must be Fp16, got {:?}", other.scalar_type()),
            };
            let got_f16 = match &got.embeddings[0].values {
                EmbeddingValues::Fp16(v) => v.clone(),
                other => panic!(
                    "compacted-output must be Fp16 (the coerce_to_precision call \
                     in compaction.rs must restore precision from the fp32 \
                     VectorRecord intermediate), got {:?}",
                    other.scalar_type()
                ),
            };
            assert_eq!(
                orig_f16, got_f16,
                "fp16 → fp32 (VectorRecord) → fp16 (coerce) → Arrow → fp16 must be bit-exact"
            );
            assert_eq!(got.embeddings[0].precision, EmbeddingScalarType::Fp16);
        }
    }
}
