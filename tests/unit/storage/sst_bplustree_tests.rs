//! Tests for SST B+ tree indexing functionality.

use proximadb::storage::engines::impls::sst::{BPlusTreeIndex, IndexEntry};

/// Helper to create test index entries
fn create_test_entries(count: usize) -> Vec<IndexEntry> {
    (0..count)
        .map(|i| IndexEntry {
            key: format!("key_{:05}", i),
            offset: i as u64 * 1000,
            size: 1000,
            block_id: i as u32,
            block_offset: 0,
            compressed: false,
            block_centroid: vec![i as f32; 8],
            block_centroid_fp16: None,
            metadata_min_values: std::collections::HashMap::new(),
            metadata_max_values: std::collections::HashMap::new(),
            metadata_null_counts: std::collections::HashMap::new(),
            block_key_bloom: None,
            block_metadata_bloom: None,
            vector_format: proximadb::storage::engines::impls::sst::VectorFormat::Fixed { dimension: 8 },
            zorder_code: None,
        })
        .collect()
}

#[test]
fn test_bplustree_build() {
    let entries = create_test_entries(100);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Check structure
    assert_eq!(tree.fanout, 16);
    assert_eq!(tree.leaves.len(), (100 + 15) / 16); // Ceiling division
    assert_eq!(tree.root.len(), tree.leaves.len());

    // Check leaf ranges
    for (i, leaf) in tree.leaves.iter().enumerate() {
        assert_eq!(leaf.start_idx, i * 16);
        assert!(leaf.len <= 16);
        assert!(leaf.len > 0);
    }
}

#[test]
fn test_bplustree_build_small() {
    // Test with fewer entries than fanout
    let entries = create_test_entries(5);
    let tree = BPlusTreeIndex::build(&entries, 16);

    assert_eq!(tree.leaves.len(), 1);
    assert_eq!(tree.leaves[0].len, 5);
    assert_eq!(tree.leaves[0].start_idx, 0);
}

#[test]
fn test_bplustree_build_empty() {
    let entries: Vec<IndexEntry> = vec![];
    let tree = BPlusTreeIndex::build(&entries, 16);

    assert_eq!(tree.leaves.len(), 0);
    assert_eq!(tree.root.len(), 0);
}

#[test]
fn test_leaf_for_key() {
    let entries = create_test_entries(100);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Test exact match
    let leaf = tree.leaf_for_key("key_00032").unwrap();
    assert!(leaf.start_key.as_str() <= "key_00032");
    assert!(leaf.end_key.as_str() >= "key_00032");

    // Test first key
    let leaf = tree.leaf_for_key("key_00000").unwrap();
    assert_eq!(leaf.start_key, "key_00000");

    // Test last key
    let leaf = tree.leaf_for_key("key_00099").unwrap();
    assert_eq!(leaf.end_key, "key_00099");
}

#[test]
fn test_leaf_for_key_not_found() {
    let entries = create_test_entries(100);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Key before first
    let leaf = tree.leaf_for_key("key_00000");
    assert!(leaf.is_some()); // Will return first leaf

    // Key after last
    let leaf = tree.leaf_for_key("key_99999");
    assert!(leaf.is_some()); // Will return last leaf
}

#[test]
fn test_range_leaves() {
    let entries = create_test_entries(100);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Range spanning multiple leaves
    let leaves = tree.range_leaves("key_00010", "key_00040");
    assert!(leaves.len() >= 2); // Should span at least 2 leaves with fanout 16

    // Range within single leaf
    let leaves = tree.range_leaves("key_00000", "key_00010");
    assert!(leaves.len() >= 1);

    // Full range
    let leaves = tree.range_leaves("key_00000", "key_00099");
    assert_eq!(leaves.len(), tree.leaves.len());
}

#[test]
fn test_range_leaves_no_overlap() {
    let entries = create_test_entries(50);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Range with no entries (before all keys)
    let leaves = tree.range_leaves("aaa_00000", "aaa_99999");
    assert_eq!(leaves.len(), 0);

    // Range with no entries (after all keys)
    let leaves = tree.range_leaves("zzz_00000", "zzz_99999");
    assert_eq!(leaves.len(), 0);
}

#[test]
fn test_fanout_minimum() {
    let entries = create_test_entries(100);

    // Request fanout below minimum (should be clamped to 8)
    let tree = BPlusTreeIndex::build(&entries, 2);
    assert_eq!(tree.fanout, 8);

    let tree = BPlusTreeIndex::build(&entries, 0);
    assert_eq!(tree.fanout, 8);
}

#[test]
fn test_bplustree_serialization() {
    let entries = create_test_entries(50);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Serialize
    let serialized = bincode::serialize(&tree).expect("Serialization failed");

    // Deserialize
    let deserialized: BPlusTreeIndex =
        bincode::deserialize(&serialized).expect("Deserialization failed");

    // Verify
    assert_eq!(deserialized.fanout, tree.fanout);
    assert_eq!(deserialized.leaves.len(), tree.leaves.len());
    assert_eq!(deserialized.root.len(), tree.root.len());

    for (orig, deser) in tree.leaves.iter().zip(deserialized.leaves.iter()) {
        assert_eq!(orig.start_key, deser.start_key);
        assert_eq!(orig.end_key, deser.end_key);
        assert_eq!(orig.start_idx, deser.start_idx);
        assert_eq!(orig.len, deser.len);
    }
}

#[test]
fn test_large_fanout() {
    let entries = create_test_entries(1000);
    let tree = BPlusTreeIndex::build(&entries, 128);

    assert_eq!(tree.fanout, 128);
    assert_eq!(tree.leaves.len(), (1000 + 127) / 128);

    // Verify all entries are covered
    let total_covered: usize = tree.leaves.iter().map(|l| l.len).sum();
    assert_eq!(total_covered, 1000);
}

#[test]
fn test_leaf_boundaries() {
    let entries = create_test_entries(100);
    let tree = BPlusTreeIndex::build(&entries, 20);

    // Test that leaves are properly ordered and non-overlapping
    for i in 0..tree.leaves.len() - 1 {
        let current = &tree.leaves[i];
        let next = &tree.leaves[i + 1];

        // Current leaf's end should be before next leaf's start
        assert!(current.end_key <= next.start_key);

        // Indices should not overlap
        assert_eq!(current.start_idx + current.len, next.start_idx);
    }
}

#[test]
fn test_root_pivot_keys() {
    let entries = create_test_entries(100);
    let tree = BPlusTreeIndex::build(&entries, 16);

    // Each root entry's pivot key should match its leaf's start key
    for root_entry in &tree.root {
        let leaf = &tree.leaves[root_entry.leaf_idx];
        assert_eq!(root_entry.pivot_key, leaf.start_key);
    }
}

#[test]
fn test_lookup_performance_characteristic() {
    // Test that tree reduces search space (not an actual perf test, just structure verification)
    let entries = create_test_entries(1000);
    let tree = BPlusTreeIndex::build(&entries, 64);

    // With 1000 entries and fanout 64, should have ~16 leaves
    assert!(tree.leaves.len() <= 20);

    // Searching should only need to scan one leaf (64 entries) instead of all 1000
    let leaf = tree.leaf_for_key("key_00500").unwrap();
    assert!(leaf.len <= 64);
}
