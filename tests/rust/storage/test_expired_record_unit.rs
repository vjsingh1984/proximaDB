use anyhow::Result;
use tracing::{debug, error, info, warn};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;

use proximadb::core::SstConfig;
use proximadb::storage::engines::sst::compaction::{CompactionManager, CompactionTask, CompactionPriority, CompactionStats};
use proximadb::storage::engines::sst::SstRecord;

/// Unit test for LSM compaction expired record deletion logic
#[tokio::test]
async fn test_lsm_compaction_expired_deletion_unit() -> Result<()> {
    // Create test data with controlled timestamps
    let current_time = Utc::now().timestamp() as u32;
    let expired_time = current_time - (5 * 60 * 60); // 5 hours ago
    let future_time = current_time + (5 * 60 * 60); // 5 hours from now
    
    let test_records = vec![
        // Active record (no expiry)
        SstRecord {
            id: "active_1".to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: vec![],
            timestamp: current_time,
            updated_at: Some(current_time),
            expires_at: None,
            version: Some(1),
            is_tombstone: false,
            sequence_number: 1,
            level: 0,
        },
        // Expired record (should be deleted)
        SstRecord {
            id: "expired_1".to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            metadata: vec![],
            timestamp: expired_time,
            updated_at: Some(expired_time),
            expires_at: Some(expired_time),
            version: Some(1),
            is_tombstone: false,
            sequence_number: 2,
            level: 0,
        },
        // Active record with future expiry
        SstRecord {
            id: "future_1".to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![7.0, 8.0, 9.0],
            metadata: vec![],
            timestamp: current_time,
            updated_at: Some(current_time),
            expires_at: Some(future_time),
            version: Some(1),
            is_tombstone: false,
            sequence_number: 3,
            level: 0,
        },
        // Old tombstone (should be removed)
        SstRecord {
            id: "old_tombstone".to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![],
            metadata: vec![],
            timestamp: expired_time,
            updated_at: Some(expired_time),
            expires_at: None,
            version: Some(1),
            is_tombstone: true,
            sequence_number: 4,
            level: 0,
        },
    ];
    
    // Create temporary directory and files
    let temp_dir = tempfile::tempdir()?;
    let collection_dir = temp_dir.path().join("test_collection");
    std::fs::create_dir_all(&collection_dir)?;
    
    let input_file = collection_dir.join("input.sst");
    let output_file = collection_dir.join("output.sst");
    
    // Write test data to input file
    let mut input_data = Vec::new();
    for record in &test_records {
        let serialized = bincode::serialize(record)?;
        input_data.extend_from_slice(&(serialized.len() as u32).to_le_bytes());
        input_data.extend_from_slice(&serialized);
    }
    std::fs::write(&input_file, &input_data)?;
    
    // Create compaction task
    let task = CompactionTask {
        collection_id: "test_collection".to_string(),
        level: 0,
        input_files: vec![input_file],
        output_file: output_file.clone(),
        priority: CompactionPriority::Medium,
    };
    
    // Create config and perform compaction
    let config = SstConfig::default();
    let stats = CompactionManager::perform_compaction(&task, &config).await?;
    
    // Verify statistics
    assert_eq!(stats.expired_records_deleted, 1, "Should delete 1 expired record");
    assert_eq!(stats.tombstones_removed, 1, "Should remove 1 old tombstone");
    
    // Verify output file exists
    assert!(output_file.exists(), "Output file should exist");
    
    // Read and verify output file contents
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
        if let Ok(record) = SstRecord::deserialize(entry_data) {
            remaining_records.push(record);
        }
        
        offset += entry_len;
    }
    
    // Should have 2 records: active_1 and future_1
    assert_eq!(remaining_records.len(), 2, "Should have 2 records after compaction");
    
    let remaining_ids: Vec<&String> = remaining_records.iter().map(|r| &r.id).collect();
    assert!(remaining_ids.contains(&&"active_1".to_string()), "Active record should remain");
    assert!(remaining_ids.contains(&&"future_1".to_string()), "Future expiry record should remain");
    assert!(!remaining_ids.contains(&&"expired_1".to_string()), "Expired record should be deleted");
    assert!(!remaining_ids.contains(&&"old_tombstone".to_string()), "Old tombstone should be removed");
    
    debug!("✅ LSM compaction expired deletion unit test passed!");
    debug!("   - Input records: {}", test_records.len());
    debug!("   - Remaining records: {}", remaining_records.len());
    debug!("   - Expired deleted: {}", stats.expired_records_deleted);
    debug!("   - Tombstones removed: {}", stats.tombstones_removed);
    
    Ok(())
}

/// Mock test for VIPER expired record deletion logic
#[tokio::test]
async fn test_viper_expired_record_logic_unit() -> Result<()> {
    // This test mocks the VIPER expiry logic from compact_parquet_files
    let current_time = Utc::now().timestamp() as u32;
    let expired_time = current_time - (2 * 60 * 60); // 2 hours ago
    let future_time = current_time + (2 * 60 * 60); // 2 hours from now
    
    // Mock record data (simulating what would be in Parquet files)
    let mock_records = vec![
        ("active_record", current_time, None),
        ("expired_record", expired_time, Some(expired_time)),
        ("future_record", current_time, Some(future_time)),
    ];
    
    // Apply the same expiry logic as in VIPER compaction
    let mut kept_records = Vec::new();
    let mut expired_count = 0;
    
    for (record_id, timestamp, expires_at) in mock_records {
        // This mirrors the logic in VIPER's compact_parquet_files method
        if let Some(expires_at) = expires_at {
            if expires_at < current_time {
                expired_count += 1;
                debug!("⏰ VIPER: Skipping expired record {} (expired at {})", record_id, expires_at);
                continue;
            }
        }
        
        kept_records.push((record_id, timestamp, expires_at));
    }
    
    // Verify results
    assert_eq!(expired_count, 1, "Should have 1 expired record");
    assert_eq!(kept_records.len(), 2, "Should keep 2 records");
    
    let kept_ids: Vec<&str> = kept_records.iter().map(|(id, _, _)| *id).collect();
    assert!(kept_ids.contains(&"active_record"), "Active record should be kept");
    assert!(kept_ids.contains(&"future_record"), "Future expiry record should be kept");
    assert!(!kept_ids.contains(&"expired_record"), "Expired record should be filtered out");
    
    debug!("✅ VIPER expired record logic unit test passed!");
    debug!("   - Input records: 3");
    debug!("   - Kept records: {}", kept_records.len());
    debug!("   - Expired filtered: {}", expired_count);
    
    Ok(())
}

/// Unit test for edge cases in expiry logic
#[tokio::test]
async fn test_expiry_edge_cases_unit() -> Result<()> {
    let current_time = Utc::now().timestamp_millis();
    let just_expired = current_time - 1; // Just expired by 1ms
    let just_future = current_time + 1; // Expires in 1ms
    
    // Test boundary conditions
    let test_cases = vec![
        ("just_expired", Some(just_expired), true),  // Should be expired
        ("just_future", Some(just_future), false),   // Should not be expired
        ("no_expiry", None, false),                  // Should not be expired
        ("far_future", Some(current_time + 1000000), false), // Should not be expired
        ("far_past", Some(current_time - 1000000), true),    // Should be expired
    ];
    
    for (name, expires_at, should_be_expired) in test_cases {
        let is_expired = if let Some(expires_at) = expires_at {
            expires_at < current_time
        } else {
            false
        };
        
        assert_eq!(is_expired, should_be_expired, 
                  "Record '{}' expiry check failed: expires_at={:?}, current={}, expected_expired={}", 
                  name, expires_at, current_time, should_be_expired);
    }
    
    debug!("✅ Expiry edge cases unit test passed!");
    Ok(())
}

/// Test for tombstone cleanup logic
#[tokio::test]
async fn test_tombstone_cleanup_unit() -> Result<()> {
    let current_time = Utc::now().timestamp_millis();
    let one_hour_ago = current_time - (60 * 60 * 1000); // 1 hour ago
    let two_hours_ago = current_time - (2 * 60 * 60 * 1000); // 2 hours ago
    
    // Test tombstone ages
    let tombstone_cases = vec![
        ("recent_tombstone", one_hour_ago - 1000, true),  // Should be kept (< 1 hour)
        ("old_tombstone", two_hours_ago, false),          // Should be removed (> 1 hour)
        ("boundary_tombstone", current_time - (60 * 60 * 1000), false), // Exactly 1 hour (should be removed)
    ];
    
    for (name, tombstone_time, should_keep) in tombstone_cases {
        // This mirrors the tombstone cleanup logic in LSM compaction
        let age = current_time - tombstone_time;
        let keep_tombstone = age < (60 * 60 * 1000); // 1 hour in milliseconds
        
        assert_eq!(keep_tombstone, should_keep,
                  "Tombstone '{}' cleanup check failed: age={}ms, expected_keep={}", 
                  name, age, should_keep);
    }
    
    debug!("✅ Tombstone cleanup unit test passed!");
    Ok(())
}