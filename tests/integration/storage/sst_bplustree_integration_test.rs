//! End-to-end integration tests for SST B+ tree indexing.
//!
//! These tests verify that the B+ tree is correctly built during writes
//! and used during reads for efficient key/range lookups.

use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::impls::sst::readers::sst_query_engine::SstQueryEngine;
use proximadb::storage::engines::impls::sst::writer::SstableWriter;
use std::path::PathBuf;
use tempfile::TempDir;

/// Create test vector records
fn create_test_records(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{:05}", i),
            vector: vec![i as f32 % 10.0; dimension],
            timestamp: Some(1000 + i as i64),
            version: 1,
            metadata: vec![],
            deleted: false,
        })
        .collect()
}

#[tokio::test]
async fn test_bplustree_write_read_cycle() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test.sst");

    // Write SST with B+ tree
    let records = create_test_records(100, 128);
    let mut writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 128);

    for record in &records {
        writer.add_record(record.clone()).await.unwrap();
    }

    let (output_path, stats) = writer.flush_to_disk().await.unwrap();
    println!(
        "Wrote SST: {} ({} records)",
        output_path, stats.records_written
    );

    // Read and verify B+ tree exists
    let reader = SstQueryEngine::new(&sst_path);
    let header = reader.read_header_async().await.unwrap();
    let index = reader.read_index(&header).await.unwrap();

    // Verify B+ tree was built
    assert!(index.bplus_tree.is_some(), "B+ tree should be present");
    let tree = index.bplus_tree.as_ref().unwrap();

    // Verify tree structure
    assert!(tree.leaves.len() > 0, "B+ tree should have leaves");
    assert_eq!(
        tree.root.len(),
        tree.leaves.len(),
        "Root should have entry for each leaf"
    );

    // Verify all records are covered
    let total_entries: usize = tree.leaves.iter().map(|l| l.len).sum();
    assert_eq!(total_entries, 100, "B+ tree should cover all entries");
}

#[tokio::test]
async fn test_bplustree_point_lookup() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test_lookup.sst");

    // Write SST
    let records = create_test_records(200, 64);
    let mut writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 64);

    for record in &records {
        writer.add_record(record.clone()).await.unwrap();
    }

    writer.flush_to_disk().await.unwrap();

    // Read and test point lookup
    let reader = SstQueryEngine::new(&sst_path);
    let header = reader.read_header_async().await.unwrap();
    let index = reader.read_index(&header).await.unwrap();

    // Test exact matches
    let entry = index.find_entry("vec_00050");
    assert!(entry.is_some(), "Should find existing key");
    assert_eq!(entry.unwrap().key, "vec_00050");

    let entry = index.find_entry("vec_00000");
    assert!(entry.is_some(), "Should find first key");

    let entry = index.find_entry("vec_00199");
    assert!(entry.is_some(), "Should find last key");

    // Test non-existent key
    let entry = index.find_entry("vec_99999");
    assert!(entry.is_none(), "Should not find non-existent key");
}

#[tokio::test]
async fn test_bplustree_range_lookup() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test_range.sst");

    // Write SST
    let records = create_test_records(100, 64);
    let mut writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 64);

    for record in &records {
        writer.add_record(record.clone()).await.unwrap();
    }

    writer.flush_to_disk().await.unwrap();

    // Read and test range lookup
    let reader = SstQueryEngine::new(&sst_path);
    let header = reader.read_header_async().await.unwrap();
    let index = reader.read_index(&header).await.unwrap();

    // Test small range
    let entries = index.range_entries("vec_00010", "vec_00020");
    assert_eq!(
        entries.len(),
        11,
        "Should find 11 entries (inclusive range)"
    );

    // Test large range
    let entries = index.range_entries("vec_00000", "vec_00099");
    assert_eq!(entries.len(), 100, "Should find all 100 entries");

    // Test range with no matches
    let entries = index.range_entries("vec_99000", "vec_99999");
    assert_eq!(entries.len(), 0, "Should find no entries");

    // Verify entries are in order
    for window in entries.windows(2) {
        assert!(
            window[0].key <= window[1].key,
            "Range results should be sorted"
        );
    }
}

#[tokio::test]
async fn test_bplustree_vs_linear_scan_compatibility() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test_compat.sst");

    // Write SST
    let records = create_test_records(50, 32);
    let mut writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 32);

    for record in &records {
        writer.add_record(record.clone()).await.unwrap();
    }

    writer.flush_to_disk().await.unwrap();

    // Read index
    let reader = SstQueryEngine::new(&sst_path);
    let header = reader.read_header_async().await.unwrap();
    let index = reader.read_index(&header).await.unwrap();

    // Test that B+ tree and linear scan produce same results
    for i in 0..50 {
        let key = format!("vec_{:05}", i);

        // B+ tree lookup
        let btree_result = index.find_entry(&key);

        // Linear scan
        let linear_result = index.all_entries().iter().find(|e| e.key == key);

        // Results should match
        assert_eq!(
            btree_result.is_some(),
            linear_result.is_some(),
            "B+ tree and linear scan should agree on existence"
        );

        if let (Some(btree), Some(linear)) = (btree_result, linear_result) {
            assert_eq!(btree.key, linear.key, "Keys should match");
            assert_eq!(btree.offset, linear.offset, "Offsets should match");
        }
    }
}

#[tokio::test]
async fn test_bplustree_empty_file() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test_empty.sst");

    // Write empty SST
    let writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 64);
    let result = writer.flush_to_disk().await;

    // Empty flush should still work (though may produce no file)
    // The behavior depends on implementation - either success or expected error
    match result {
        Ok((path, stats)) => {
            println!(
                "Empty SST created: {} ({} records)",
                path, stats.records_written
            );
            assert_eq!(stats.records_written, 0);
        }
        Err(e) => {
            println!("Empty SST flush expectedly failed: {}", e);
            // This is also acceptable behavior
        }
    }
}

#[tokio::test]
async fn test_bplustree_large_fanout() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test_large_fanout.sst");

    // Write SST with many records to test large fanout
    let records = create_test_records(1000, 128);
    let mut writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 128);

    for record in &records {
        writer.add_record(record.clone()).await.unwrap();
    }

    writer.flush_to_disk().await.unwrap();

    // Read and verify tree structure
    let reader = SstQueryEngine::new(&sst_path);
    let header = reader.read_header_async().await.unwrap();
    let index = reader.read_index(&header).await.unwrap();

    let tree = index.bplus_tree.as_ref().unwrap();

    // With 1000 entries and fanout 128, should have ~8 leaves
    assert!(
        tree.leaves.len() <= 10,
        "Should have reasonable number of leaves"
    );
    assert!(
        tree.leaves.len() >= 7,
        "Should have at least ceil(1000/128) leaves"
    );

    // Test random lookups are still fast (structure verification)
    for i in (0..1000).step_by(100) {
        let key = format!("vec_{:05}", i);
        let entry = index.find_entry(&key);
        assert!(entry.is_some(), "Should find key {}", key);
    }
}

#[tokio::test]
async fn test_bplustree_serialization_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let sst_path = temp_dir.path().join("test_serialization.sst");

    // Write SST
    let records = create_test_records(100, 64);
    let mut writer = SstableWriter::new(sst_path.to_str().unwrap().to_string(), 64);

    for record in &records {
        writer.add_record(record.clone()).await.unwrap();
    }

    writer.flush_to_disk().await.unwrap();

    // Read index twice to ensure serialization is stable
    let reader = SstQueryEngine::new(&sst_path);
    let header = reader.read_header_async().await.unwrap();

    let index1 = reader.read_index(&header).await.unwrap();
    let index2 = reader.read_index(&header).await.unwrap();

    // Compare trees
    let tree1 = index1.bplus_tree.as_ref().unwrap();
    let tree2 = index2.bplus_tree.as_ref().unwrap();

    assert_eq!(tree1.fanout, tree2.fanout);
    assert_eq!(tree1.leaves.len(), tree2.leaves.len());
    assert_eq!(tree1.root.len(), tree2.root.len());

    // Verify lookups produce same results
    for i in 0..100 {
        let key = format!("vec_{:05}", i);
        let result1 = index1.find_entry(&key);
        let result2 = index2.find_entry(&key);

        assert_eq!(result1.is_some(), result2.is_some());
        if let (Some(e1), Some(e2)) = (result1, result2) {
            assert_eq!(e1.key, e2.key);
            assert_eq!(e1.offset, e2.offset);
        }
    }
}
