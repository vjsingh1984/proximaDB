use anyhow::Result;
use tracing::{debug, error, info, warn};
use chrono::Utc;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;

use proximadb::core::SstConfig;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::sst::compaction::{CompactionManager, CompactionTask, CompactionPriority};
use proximadb::storage::engines::sst::SstEntry;
use proximadb::storage::engines::viper::ViperEngine;
use proximadb::storage::memtable::core::MemtableConfig;
use proximadb::storage::persistence::filesystem::FilesystemFactory;

/// Test SST engine expired record deletion through the full pipeline:
/// WAL → Flush → Compaction → Physical deletion
#[tokio::test]
async fn test_sst_expired_record_full_pipeline() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let data_dir = temp_dir.path().to_path_buf();
    
    // Create LSM config with aggressive compaction settings
    let mut config = SstConfig::default();
    config.compaction_threshold = 1; // Trigger compaction with just 1 file
    config.memtable_size_mb = 1; // Small memtable to trigger frequent flushes
    config.data_directory = data_dir.join("lsm").to_string_lossy().to_string();
    
    let collection_id = "test_collection";
    let current_time = Utc::now().timestamp_millis();
    let expired_time = current_time - (3 * 60 * 60 * 1000); // 3 hours ago (well expired)
    let future_time = current_time + (3 * 60 * 60 * 1000); // 3 hours from now
    
    // Create test records with different expiry states
    let records = vec![
        // Record 1: Active (no expiry)
        SstEntry {
            id: "active_record".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
            timestamp: current_time,
            created_at: current_time,
            updated_at: current_time,
            expires_at: None, // No expiry
            version: Some(1),
            is_tombstone: false,
            sequence_number: 1,
            level: 0,
        },
        // Record 2: Expired (should be deleted)
        SstEntry {
            id: "expired_record".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![4.0, 5.0, 6.0],
            metadata: HashMap::new(),
            timestamp: expired_time,
            created_at: expired_time,
            updated_at: expired_time,
            expires_at: Some(expired_time), // Expired 3 hours ago
            version: Some(1),
            is_tombstone: false,
            sequence_number: 2,
            level: 0,
        },
        // Record 3: Active with future expiry
        SstEntry {
            id: "future_expiry_record".to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![7.0, 8.0, 9.0],
            metadata: HashMap::new(),
            timestamp: current_time,
            created_at: current_time,
            updated_at: current_time,
            expires_at: Some(future_time), // Expires in 3 hours
            version: Some(1),
            is_tombstone: false,
            sequence_number: 3,
            level: 0,
        },
    ];
    
    // Step 1: Simulate WAL → Flush by creating SST files directly
    let collection_dir = data_dir.join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;
    
    // Create multiple SST files to trigger compaction
    for (i, record) in records.iter().enumerate() {
        let sst_file = collection_dir.join(format!("sst_{}_{}.sstable", i, record.sequence_number));
        let serialized = bincode::serialize(record)?;
        let mut sst_data = Vec::new();
        sst_data.extend_from_slice(&(serialized.len() as u32).to_le_bytes());
        sst_data.extend_from_slice(&serialized);
        std::fs::write(&sst_file, &sst_data)?;
    }
    
    // Step 2: Trigger compaction
    let compaction_manager = CompactionManager::new(config.clone()).await.unwrap();
    let output_file = collection_dir.join("compacted_output.sstable");
    
    let task = CompactionTask {
        collection_id: collection_id.to_string(),
        level: 0,
        input_files: vec![
            collection_dir.join("sst_0_1.sstable"),
            collection_dir.join("sst_1_2.sstable"),
            collection_dir.join("sst_2_3.sstable"),
        ],
        output_file: output_file.clone(),
        priority: CompactionPriority::High,
    };
    
    // Perform compaction
    let stats = CompactionManager::perform_compaction(&task, &config).await?;
    
    // Step 3: Verify results
    debug!("🧹 Compaction Stats:");
    debug!("  - Total compactions: {}", stats.total_compactions);
    debug!("  - Bytes read: {}", stats.bytes_read);
    debug!("  - Bytes written: {}", stats.bytes_written);
    debug!("  - Files merged: {}", stats.files_merged);
    debug!("  - Expired records deleted: {}", stats.expired_records_deleted);
    debug!("  - Tombstones removed: {}", stats.tombstones_removed);
    
    // Should have deleted 1 expired record
    assert_eq!(stats.expired_records_deleted, 1, "Expected 1 expired record to be deleted");
    
    // Verify output file exists
    assert!(output_file.exists(), "Compaction output file should exist");
    
    // Step 4: Read and verify the output file
    let output_data = std::fs::read(&output_file)?;
    let mut remaining_records = Vec::new();
    let mut offset = 0;
    
    while offset < output_data.len() {
        if offset + 4 > output_data.len() {
            break;
        }
        
        let entry_len = u32::from_le_bytes([
            output_data[offset],
            output_data[offset + 1],
            output_data[offset + 2],
            output_data[offset + 3],
        ]) as usize;
        
        offset += 4;
        
        if offset + entry_len > output_data.len() {
            break;
        }
        
        let entry_data = &output_data[offset..offset + entry_len];
        if let Ok(record) = SstEntry::deserialize(entry_data) {
            remaining_records.push(record);
        }
        
        offset += entry_len;
    }
    
    // Should have 2 records (active_record and future_expiry_record)
    assert_eq!(remaining_records.len(), 2, "Expected 2 records after compaction");
    
    // Verify specific records
    let ids: Vec<&String> = remaining_records.iter().map(|r| &r.id).collect();
    assert!(ids.contains(&&"active_record".to_string()), "Active record should remain");
    assert!(ids.contains(&&"future_expiry_record".to_string()), "Future expiry record should remain");
    assert!(!ids.contains(&&"expired_record".to_string()), "Expired record should be deleted");
    
    debug!("✅ LSM expired record deletion test passed!");
    Ok(())
}

/// Test VIPER engine expired record deletion through compaction
#[tokio::test]
async fn test_viper_expired_record_compaction() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let data_dir = temp_dir.path().to_path_buf();
    
    // Create VIPER engine
    let collection_id = "test_viper_collection";
    let filesystem = FilesystemFactory::create("file", &data_dir.to_string_lossy())?;
    let memtable_config = MemtableConfig::default();
    
    let viper_engine = ViperEngine::new(
        collection_id.to_string(),
        filesystem,
        memtable_config,
        None, // No collection service for this test
    );
    
    // Create test data directory
    let collection_dir = data_dir.join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;
    
    let current_time = Utc::now().timestamp_millis();
    let expired_time = current_time - (4 * 60 * 60 * 1000); // 4 hours ago
    let future_time = current_time + (4 * 60 * 60 * 1000); // 4 hours from now
    
    // Create test Parquet files (simulating flushed data)
    // In a real scenario, these would be created by the flush process
    let test_files = vec![
        collection_dir.join("vectors_1.parquet"),
        collection_dir.join("vectors_2.parquet"),
        collection_dir.join("vectors_3.parquet"),
    ];
    
    // Create minimal Parquet files with Arrow
    use arrow_array::{Int64Array, StringArray, ListArray, Float32Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;
    
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("collection_id", DataType::Utf8, false),
        Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, false)), 3), false),
        Field::new("timestamp", DataType::Int64, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("expires_at", DataType::Int64, true), // nullable
        Field::new("version", DataType::Int64, false),
    ]));
    
    // Create test data batches
    let test_data = vec![
        // File 1: Active record
        (
            "active_viper_record",
            vec![1.0, 2.0, 3.0],
            current_time,
            None, // No expiry
        ),
        // File 2: Expired record
        (
            "expired_viper_record", 
            vec![4.0, 5.0, 6.0],
            expired_time,
            Some(expired_time), // Expired
        ),
        // File 3: Future expiry record
        (
            "future_viper_record",
            vec![7.0, 8.0, 9.0],
            current_time,
            Some(future_time), // Future expiry
        ),
    ];
    
    // Create Parquet files
    for (i, (id, vector, timestamp, expires_at)) in test_data.iter().enumerate() {
        let file = std::fs::File::create(&test_files[i])?;
        let mut writer = ArrowWriter::try_new(file, schema.clone(), None)?;
        
        // Create arrays
        let id_array = StringArray::from(vec![*id]);
        let collection_id_array = StringArray::from(vec![collection_id]);
        
        // Vector as List<Float32>
        let vector_values = Float32Array::from(vector.clone());
        let vector_list = ListArray::from_iter_primitive::<arrow_array::types::Float32Type, _, _>(
            vec![Some(vector_values.values().clone())]
        );
        
        let timestamp_array = Int64Array::from(vec![*timestamp]);
        let created_at_array = Int64Array::from(vec![*timestamp]);
        let updated_at_array = Int64Array::from(vec![*timestamp]);
        let expires_at_array = Int64Array::from(vec![*expires_at]);
        let version_array = Int64Array::from(vec![1i64]);
        
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(id_array),
                Arc::new(collection_id_array),
                Arc::new(vector_list),
                Arc::new(timestamp_array),
                Arc::new(created_at_array),
                Arc::new(updated_at_array),
                Arc::new(expires_at_array),
                Arc::new(version_array),
            ],
        )?;
        
        writer.write(&batch)?;
        writer.close()?;
    }
    
    // Step 2: Trigger VIPER compaction
    let input_file_paths: Vec<String> = test_files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    
    let compaction_result = viper_engine
        .compact_parquet_files(&collection_id.to_string(), input_file_paths)
        .await?;
    
    // Step 3: Verify results
    assert_eq!(compaction_result.len(), 1, "Expected 1 compacted output file");
    
    let output_file = std::path::Path::new(&compaction_result[0]);
    assert!(output_file.exists(), "Compacted output file should exist");
    
    // Step 4: Read and verify the compacted file
    let file = std::fs::File::open(output_file)?;
    let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?;
    let reader = builder.build()?;
    
    let mut record_count = 0;
    let mut found_ids = Vec::new();
    
    for batch_result in reader {
        let batch = batch_result?;
        record_count += batch.num_rows();
        
        let id_array = batch.column_by_name("id").unwrap()
            .as_any().downcast_ref::<StringArray>().unwrap();
        
        for i in 0..batch.num_rows() {
            found_ids.push(id_array.value(i).to_string());
        }
    }
    
    // Should have 2 records (active and future expiry), expired should be gone
    assert_eq!(record_count, 2, "Expected 2 records after VIPER compaction");
    assert!(found_ids.contains(&"active_viper_record".to_string()), "Active record should remain");
    assert!(found_ids.contains(&"future_viper_record".to_string()), "Future expiry record should remain");
    assert!(!found_ids.contains(&"expired_viper_record".to_string()), "Expired record should be deleted");
    
    debug!("✅ VIPER expired record deletion test passed!");
    Ok(())
}

/// Test that demonstrates the time propagation from WAL to compaction
#[tokio::test]
async fn test_expired_record_time_propagation() -> Result<()> {
    debug!("🕐 Testing time propagation: WAL → Flush → Compaction → Physical deletion");
    
    // This test demonstrates the typical flow:
    // 1. Records are written to WAL with TTL
    // 2. WAL is flushed to storage engines (LSM SST files, VIPER Parquet files)
    // 3. Background compaction runs and physically deletes expired records
    // 4. Expired records are no longer accessible
    
    let current_time = Utc::now().timestamp_millis();
    let short_ttl = 100; // 100ms TTL for testing
    let expired_time = current_time - (short_ttl * 2); // Already expired
    
    debug!("📊 Timing:");
    debug!("  - Current time: {}", current_time);
    debug!("  - Short TTL: {}ms", short_ttl);
    debug!("  - Expired time: {}", expired_time);
    debug!("  - Time since expiry: {}ms", current_time - expired_time);
    
    // In a real scenario, you would:
    // 1. Write records with TTL to WAL
    // 2. Wait for flush to occur (memtable threshold reached)
    // 3. Wait for compaction to occur (SST file threshold reached)
    // 4. Verify expired records are physically deleted
    
    // Simulate waiting for background processes
    debug!("⏰ Simulating background process timing...");
    sleep(Duration::from_millis(50)).await; // Simulate flush delay
    debug!("📤 Flush completed");
    
    sleep(Duration::from_millis(100)).await; // Simulate compaction delay
    debug!("🗜️ Compaction completed");
    
    // In production, expired records would be:
    // - Skipped during search (logical deletion)
    // - Physically deleted during compaction
    // - No longer consuming storage space
    
    debug!("✅ Time propagation test demonstrates the flow!");
    Ok(())
}