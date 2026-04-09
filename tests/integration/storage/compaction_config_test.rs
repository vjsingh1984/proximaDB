//! Unit tests for compaction configuration and triggers
//!
//! Tests compaction trigger conditions, collection-specific compaction settings,
//! and integration with flush operations.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;
use tracing::{debug, error, info, warn};

use proximadb::core::SstConfig;
use proximadb::storage::engines::sst::SstEntry;
use proximadb::storage::engines::sst::compaction::{
    CompactionManager, CompactionPriority, CompactionTask,
};
use proximadb::storage::persistence::write_ahead_log::background_manager::BackgroundMaintenanceManager;
use proximadb::storage::persistence::write_ahead_log::config::WALConfig;

/// Helper function to create test LSM records
fn create_test_sst_records(collection_id: &str, count: usize) -> Vec<SstEntry> {
    let now = chrono::Utc::now().timestamp_millis();

    (0..count)
        .map(|i| SstEntry {
            id: format!("lsm_record_{}", i),
            collection_id: collection_id.to_string(),
            vector: vec![1.0f32; 100], // 100-dimensional vector
            metadata: std::collections::HashMap::new(),
            timestamp: now as u32,
            created_at: now,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            is_tombstone: false,
            sequence_number: i as u64,
            level: 0,
        })
        .collect()
}

/// Helper function to create test SST file with records
async fn create_test_sst_file(file_path: &std::path::Path, records: &[SstEntry]) -> Result<()> {
    let mut file_data = Vec::new();

    for record in records {
        let serialized = bincode::serialize(record)?;
        file_data.extend_from_slice(&(serialized.len() as u32).to_le_bytes());
        file_data.extend_from_slice(&serialized);
    }

    std::fs::write(file_path, file_data)?;
    Ok(())
}

#[tokio::test]
async fn test_compaction_threshold_configuration() -> Result<()> {
    // Test default compaction threshold
    let default_config = SstConfig::default();
    assert_eq!(default_config.compaction_threshold, 4);

    // Test custom compaction threshold
    let mut custom_config = SstConfig::default();
    custom_config.compaction_threshold = 2; // More aggressive compaction
    assert_eq!(custom_config.compaction_threshold, 2);

    // Test memory flush size affects compaction decisions
    assert_eq!(default_config.memory_flush_size_bytes, 64 * 1024 * 1024);

    // Test that compaction threshold should be reasonable
    assert!(custom_config.compaction_threshold > 0);
    assert!(custom_config.compaction_threshold < 10); // Reasonable upper bound

    debug!("✅ Compaction threshold configuration test passed");
    Ok(())
}

#[tokio::test]
async fn test_compaction_trigger_conditions() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_id = "test_collection";
    let collection_dir = temp_dir.path().join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;

    let config = SstConfig {
        compaction_threshold: 2, // Trigger compaction with 2 files
        data_directory: temp_dir.path().to_string_lossy().to_string(),
        decompression_cache_config: None,
        ..Default::default()
    };

    // Create test records
    let records_file1 = create_test_sst_records(collection_id, 100);
    let records_file2 = create_test_sst_records(collection_id, 150);
    let records_file3 = create_test_sst_records(collection_id, 200);

    // Create SST files
    let sst_file1 = collection_dir.join("sst_1.sstable");
    let sst_file2 = collection_dir.join("sst_2.sstable");
    let sst_file3 = collection_dir.join("sst_3.sstable");

    create_test_sst_file(&sst_file1, &records_file1).await?;
    create_test_sst_file(&sst_file2, &records_file2).await?;
    create_test_sst_file(&sst_file3, &records_file3).await?;

    // Test compaction with threshold reached
    let compaction_manager = CompactionManager::new(config.clone()).await.unwrap();
    let output_file = collection_dir.join("compacted_output.sstable");

    let task = CompactionTask {
        collection_id: collection_id.to_string(),
        level: 0,
        input_files: vec![sst_file1, sst_file2], // 2 files = threshold reached
        output_file: output_file.clone(),
        priority: CompactionPriority::Medium,
    };

    let stats = CompactionManager::perform_compaction(&task, &config).await?;

    // Verify compaction was performed
    assert_eq!(stats.total_compactions, 1);
    assert_eq!(stats.files_merged, 2);
    assert!(stats.bytes_read > 0);
    assert!(stats.bytes_written > 0);
    assert!(output_file.exists());

    debug!("✅ Compaction trigger conditions test passed");
    Ok(())
}

#[tokio::test]
async fn test_compaction_priority_levels() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_id = "priority_test_collection";
    let collection_dir = temp_dir.path().join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;

    let config = SstConfig {
        data_directory: temp_dir.path().to_string_lossy().to_string(),
        decompression_cache_config: None,
        ..Default::default()
    };

    // Create test records
    let records = create_test_sst_records(collection_id, 100);
    let input_file = collection_dir.join("input.sstable");
    create_test_sst_file(&input_file, &records).await?;

    // Test different priority levels
    let priorities = vec![
        CompactionPriority::Low,
        CompactionPriority::Medium,
        CompactionPriority::High,
        CompactionPriority::Critical,
    ];

    for (i, priority) in priorities.iter().enumerate() {
        let output_file = collection_dir.join(format!("output_{}.sstable", i));

        let task = CompactionTask {
            collection_id: collection_id.to_string(),
            level: 0,
            input_files: vec![input_file.clone()],
            output_file: output_file.clone(),
            priority: priority.clone(),
        };

        let stats = CompactionManager::perform_compaction(&task, &config).await?;

        // Verify each priority level works
        assert_eq!(stats.total_compactions, 1);
        assert!(output_file.exists());

        debug!("Compaction with priority {:?} completed", priority);
    }

    debug!("✅ Compaction priority levels test passed");
    Ok(())
}

#[tokio::test]
async fn test_compaction_with_expired_records() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_id = "expired_test_collection";
    let collection_dir = temp_dir.path().join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;

    let config = SstConfig {
        data_directory: temp_dir.path().to_string_lossy().to_string(),
        decompression_cache_config: None,
        ..Default::default()
    };

    let current_time = chrono::Utc::now().timestamp_millis();
    let expired_time = current_time - (24 * 60 * 60 * 1000); // 24 hours ago

    // Create records with different expiry states
    let mut records = Vec::new();

    // Active records (no expiry)
    for i in 0..50 {
        records.push(SstEntry {
            id: format!("active_record_{}", i),
            collection_id: collection_id.to_string(),
            vector: vec![1.0f32; 100],
            metadata: std::collections::HashMap::new(),
            timestamp: current_time,
            created_at: current_time,
            updated_at: current_time,
            expires_at: None,
            version: Some(1),
            is_tombstone: false,
            sequence_number: i as u64,
            level: 0,
        });
    }

    // Expired records (should be deleted during compaction)
    for i in 50..100 {
        records.push(SstEntry {
            id: format!("expired_record_{}", i),
            collection_id: collection_id.to_string(),
            vector: vec![1.0f32; 100],
            metadata: std::collections::HashMap::new(),
            timestamp: expired_time,
            created_at: expired_time,
            updated_at: expired_time,
            expires_at: Some(expired_time),
            version: Some(1),
            is_tombstone: false,
            sequence_number: i as u64,
            level: 0,
        });
    }

    // Create input file
    let input_file = collection_dir.join("input_with_expired.sstable");
    create_test_sst_file(&input_file, &records).await?;

    // Perform compaction
    let output_file = collection_dir.join("compacted_without_expired.sstable");
    let task = CompactionTask {
        collection_id: collection_id.to_string(),
        level: 0,
        input_files: vec![input_file],
        output_file: output_file.clone(),
        priority: CompactionPriority::Medium,
    };

    let stats = CompactionManager::perform_compaction(&task, &config).await?;

    // Verify expired records were deleted
    assert_eq!(stats.expired_records_deleted, 50);
    assert!(output_file.exists());

    // Verify output file has fewer records
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

    assert_eq!(remaining_records.len(), 50); // Only active records should remain

    debug!("✅ Compaction with expired records test passed");
    Ok(())
}

#[tokio::test]
async fn test_compaction_background_integration() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let mut config = WriteBufferConfig::default();
    config.performance.memory_flush_size_bytes = 1024 * 1024; // 1MB

    let manager = BackgroundMaintenanceManager::new(Arc::new(config));
    let collection_id = "compaction_integration_test";

    // Test that compaction is triggered after flush
    let large_memory_size = 2 * 1024 * 1024; // 2MB
    let should_flush = manager
        .trigger_flush_if_needed(&collection_id.to_string(), large_memory_size)
        .await?;
    assert!(should_flush);

    // Wait for background operations to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check that background operations are running
    let has_active_ops = manager.has_active_operations().await;
    assert!(has_active_ops);

    // Test statistics
    let stats = manager.get_stats().await;
    assert!(stats.total_flush_operations > 0 || stats.total_compaction_operations > 0);

    debug!("✅ Compaction background integration test passed");
    Ok(())
}

#[tokio::test]
async fn test_compaction_level_configuration() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_id = "level_test_collection";
    let collection_dir = temp_dir.path().join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;

    // Test different level configurations
    let level_configs = vec![
        (3, 32 * 1024 * 1024),  // 3 levels, 32MB memtable
        (5, 64 * 1024 * 1024),  // 5 levels, 64MB memtable
        (7, 128 * 1024 * 1024), // 7 levels, 128MB memtable
    ];

    for (level_count, memtable_size) in level_configs {
        let config = SstConfig {
            level_count,
            memtable_size_mb: memtable_size / (1024 * 1024) as u64,
            data_directory: temp_dir.path().to_string_lossy().to_string(),
            decompression_cache_config: None,
            ..Default::default()
        };

        // Verify configuration is valid
        assert!(config.level_count > 0);
        assert!(config.level_count <= 10); // Reasonable upper bound
        assert!(config.memtable_size_mb > 0);

        // Test compaction with different levels
        let records = create_test_sst_records(collection_id, 100);
        let input_file = collection_dir.join(format!("input_level_{}.sstable", level_count));
        create_test_sst_file(&input_file, &records).await?;

        let output_file = collection_dir.join(format!("output_level_{}.sstable", level_count));
        let task = CompactionTask {
            collection_id: collection_id.to_string(),
            level: 0,
            input_files: vec![input_file],
            output_file: output_file.clone(),
            priority: CompactionPriority::Medium,
        };

        let stats = CompactionManager::perform_compaction(&task, &config).await?;

        // Verify compaction works with different level configurations
        assert_eq!(stats.total_compactions, 1);
        assert!(output_file.exists());

        debug!("Level {} configuration test passed", level_count);
    }

    debug!("✅ Compaction level configuration test passed");
    Ok(())
}

#[tokio::test]
async fn test_compaction_performance_metrics() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_id = "performance_test_collection";
    let collection_dir = temp_dir.path().join(collection_id);
    std::fs::create_dir_all(&collection_dir)?;

    let config = SstConfig {
        data_directory: temp_dir.path().to_string_lossy().to_string(),
        decompression_cache_config: None,
        ..Default::default()
    };

    // Create larger dataset for performance testing
    let large_records = create_test_sst_records(collection_id, 5000);
    let input_file = collection_dir.join("large_input.sstable");
    create_test_sst_file(&input_file, &large_records).await?;

    // Measure compaction performance
    let start_time = std::time::Instant::now();

    let output_file = collection_dir.join("performance_output.sstable");
    let task = CompactionTask {
        collection_id: collection_id.to_string(),
        level: 0,
        input_files: vec![input_file],
        output_file: output_file.clone(),
        priority: CompactionPriority::High,
    };

    let stats = CompactionManager::perform_compaction(&task, &config).await?;

    let compaction_duration = start_time.elapsed();

    // Verify performance metrics
    assert_eq!(stats.total_compactions, 1);
    assert_eq!(stats.files_merged, 1);
    assert!(stats.bytes_read > 0);
    assert!(stats.bytes_written > 0);
    assert!(output_file.exists());

    // Performance assertions (should be reasonably fast)
    assert!(
        compaction_duration.as_millis() < 5000,
        "Compaction took too long: {}ms",
        compaction_duration.as_millis()
    );

    // Calculate throughput
    let throughput_mb_per_sec =
        (stats.bytes_read as f64 / (1024.0 * 1024.0)) / compaction_duration.as_secs_f64();

    debug!("✅ Compaction performance metrics test passed");
    debug!("   Compaction duration: {:?}", compaction_duration);
    debug!("   Bytes read: {}", stats.bytes_read);
    debug!("   Bytes written: {}", stats.bytes_written);
    debug!("   Throughput: {:.2} MB/sec", throughput_mb_per_sec);

    Ok(())
}
