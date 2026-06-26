//! SWIFT Reader Tests - Consolidated
//!
//! Sources:
//! - src/storage/engines/impls/swift/unified_reader.rs
//! - src/storage/engines/impls/swift/parquet_strategy_reader.rs
//! - src/storage/engines/impls/swift/progressive_search.rs
//! - src/storage/engines/impls/swift/hierarchical_blocks.rs
//! - src/storage/engines/impls/swift/id_index.rs

use super::super::super::swift::*;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use crate::storage::engines::core::read_strategy::ReadAccessStrategy;
use std::sync::Arc;
use std::collections::BinaryHeap;
use proximadb_distance_kernel::{UnifiedDistanceCompute, DistanceMetric};

// =====================================================
// TESTS FROM unified_reader.rs
// =====================================================

#[tokio::test]
async fn test_range_coalescing() {
    // Test that nearby reads are coalesced
    // This would require actual test infrastructure with mock filesystem
    // For now, this is a placeholder showing the test structure
}

#[tokio::test]
async fn test_hierarchical_pruning() {
    // Test that pruning reduces I/O
    // This would require actual test infrastructure
}

// =====================================================
// TESTS FROM parquet_strategy_reader.rs
// =====================================================

#[tokio::test]
async fn test_swift_strategy_selection() {
    let factory = Arc::new(FilesystemFactory::create(FilesystemConfig::default()).await.unwrap());

    // Compaction should use DirectStream
    let compaction_reader = proximablocks_compact_strategy_reader::UnifiedSWIFTReader::for_compaction(
        factory.clone(),
        "test_collection".to_string(),
    ).unwrap();
    assert_eq!(compaction_reader.strategy(), &ReadAccessStrategy::DirectStream);
    assert!(!compaction_reader.is_using_cache());

    // Search should use CachedSearch
    let search_reader = proximablocks_compact_strategy_reader::UnifiedSWIFTReader::for_search(
        factory.clone(),
        "test_collection".to_string(),
    ).unwrap();
    matches!(search_reader.strategy(), ReadAccessStrategy::CachedSearch { .. });
    assert!(search_reader.is_using_cache());
}

#[tokio::test]
async fn test_config_updates_with_strategy() {
    let factory = Arc::new(FilesystemFactory::create(FilesystemConfig::default()).await.unwrap());
    let mut reader = proximablocks_compact_strategy_reader::UnifiedSWIFTReader::for_search(
        factory,
        "test".to_string(),
    ).unwrap();

    // Initially configured for search (cached)
    assert!(reader.config.cache_metadata);
    assert_eq!(reader.config.streaming_threshold_mb, 5);

    // Change to direct stream
    reader.set_strategy(ReadAccessStrategy::DirectStream);
    assert!(!reader.config.cache_metadata);
    assert_eq!(reader.config.streaming_threshold_mb, 0); // Always stream
}

// =====================================================
// TESTS FROM progressive_search.rs
// =====================================================

// Test disabled - Candidate struct is private in progressive_search module
// #[test]
// fn test_candidate_ordering() {
//     use progressive_search::Candidate;
//
//     let mut heap = BinaryHeap::new();
//
//     heap.push(Candidate {
//         superblock_idx: 0,
//         block_idx: 0,
//         vector_idx: 0,
//         similarity: 10.0,
//     });
//
//     heap.push(Candidate {
//         superblock_idx: 0,
//         block_idx: 0,
//         vector_idx: 1,
//         similarity: 5.0,
//     });
//
//     heap.push(Candidate {
//         superblock_idx: 0,
//         block_idx: 0,
//         vector_idx: 2,
//         similarity: 15.0,
//     });
//
//     // Should pop in order: 5.0, 10.0, 15.0
//     assert_eq!(heap.pop().unwrap().similarity, 5.0);
//     assert_eq!(heap.pop().unwrap().similarity, 10.0);
//     assert_eq!(heap.pop().unwrap().similarity, 15.0);
// }

#[test]
fn test_distance_computation() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![0.0, 1.0, 0.0];

    let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let euclidean_result = compute.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    assert!((euclidean_result.distance - 1.414).abs() < 0.01);

    let cosine_result = compute.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    assert!((cosine_result.distance - 1.0).abs() < 0.01); // Orthogonal vectors

    let dot_result = compute.calculate_distance(&a, &b, &DistanceMetric::DotProduct);
    assert_eq!(dot_result.distance, 0.0); // Orthogonal vectors
}

// =====================================================
// TESTS FROM hierarchical_blocks.rs
// =====================================================

#[test]
fn test_bitset_operations() {
    use hierarchical_blocks::BitSet;

    let mut bs1 = BitSet::new(100);
    let mut bs2 = BitSet::new(100);

    bs1.set(10);
    bs1.set(20);
    bs1.set(30);

    bs2.set(20);
    bs2.set(30);
    bs2.set(40);

    // Test intersection
    let intersection = bs1.intersect(&bs2);
    assert!(intersection.test(20));
    assert!(intersection.test(30));
    assert!(!intersection.test(10));
    assert!(!intersection.test(40));
    assert_eq!(intersection.count(), 2);

    // Test union
    let union = bs1.union(&bs2);
    assert!(union.test(10));
    assert!(union.test(20));
    assert!(union.test(30));
    assert!(union.test(40));
    assert_eq!(union.count(), 4);
}

#[test]
fn test_metadata_index() {
    use hierarchical_blocks::MetadataIndex;
    use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;

    let mut index = MetadataIndex::new();
    index.filterable_columns.insert("category".to_string());
    index.filterable_columns.insert("price".to_string());

    // Create test block
    let block = ProximaDataBlock {
        encoding_marker: 0x00,
        encoding_metadata: None,
        block_id: 0,
        encoded_vectors: None,
        vector_layout: crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
        records: vec![VectorRecord {
            id: "1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: {
                let mut meta = std::collections::HashMap::new();
                meta.insert("category".to_string(), crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("electronics".to_string())),
                });
                meta.insert("price".to_string(), crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(99.99)),
                });
                meta
            },
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            source: Some("test".to_string()),
        }],
        quantized_vectors: None,
        quantization_level: None,
        quantized_section: None,
        metadata: crate::storage::engines::core::formats::proximablocks::block_structures::ProximaBlockMetadata {
            record_count: 1,
            size_bytes: 0,
            compressed_size: 0,
            timestamp: Some(0),
            compaction_level: 0,
            has_deletes: false,
            has_updates: false,
            version_range: (0, 0),
            column_stats: std::collections::HashMap::new(),
            quantization_stats: crate::storage::engines::core::formats::proximablocks::block_structures::QuantizationStatistics::default(),
            data_checksum: 0,
            metadata_checksum: 0,
        },
        compression_config: crate::storage::engines::core::formats::proximablocks::block_structures::BlockCompressionConfig {
            algorithm: proximadb_compression::CompressionAlgorithm::Lz4,
            compression_level: 1,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 8192,
            dictionary_compression: false,
            vector_layout: crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            metadata_algorithm: None,
        },
        compression_algorithm: proximadb_compression::CompressionAlgorithm::Lz4,
        uncompressed_size: 0,
        bloom_filter: None,
        block_bloom_filter: None,
        id_range: ("1".to_string(), "1".to_string()),
        timestamp_range: (0, 0),
        statistics: crate::storage::engines::core::formats::proximablocks::block_structures::BlockStatistics {
            read_count: 0,
            write_count: 0,
            search_count: 0,
            cache_hits: 0,
            cache_misses: 0,
            avg_read_time_ms: 0.0,
            avg_search_time_ms: 0.0,
            last_accessed_at: 0,
        },
        metadata_stats: None,
        has_deletes: false,
    };

    // Index the block
    index.index_block(0, 0, &block).unwrap();

    // Test finding blocks with specific value
    let matches = index
        .find_blocks_with_value("category", &serde_json::json!("electronics"))
        .unwrap();
    assert!(matches.test(0));

    // Test finding blocks in range
    let matches = index
        .find_blocks_in_range("price", &serde_json::json!(50.0), &serde_json::json!(150.0))
        .unwrap();
    assert!(matches.test(0));
}

// =====================================================
// TESTS FROM id_index.rs
// =====================================================

#[test]
fn test_id_index_basic_operations() {
    use id_index::{IdIndex, BlockLocation};

    let index = IdIndex::new();

    // Insert some IDs
    for i in 0..1000 {
        let id = format!("id_{:04}", i);
        let location = BlockLocation {
            superblock_idx: (i / 100) as u32,
            block_idx: ((i % 100) / 10) as u32,
            offset_in_block: (i % 10) as u32,
            size_bytes: 1024,
        };
        index.insert(id, location).unwrap();
    }

    // Test lookup
    let loc = index.lookup("id_0500").unwrap();
    assert_eq!(loc.superblock_idx, 5);
    assert_eq!(loc.block_idx, 0);
    assert_eq!(loc.offset_in_block, 0);

    // Test batch lookup
    let ids = vec![
        "id_0100".to_string(),
        "id_0200".to_string(),
        "id_0999".to_string(),
    ];
    let locs = index.lookup_batch(&ids);
    assert_eq!(locs.len(), 3);
    assert!(locs[0].is_some());
    assert!(locs[1].is_some());
    assert!(locs[2].is_some());

    // Test range query
    let range_results = index.range_query("id_0100", "id_0110");
    assert_eq!(range_results.len(), 11);

    // Test stats
    let stats = index.stats();
    assert_eq!(stats.unique_ids, 1000);
    assert!(stats.tree_height > 0);
}

#[test]
fn test_two_level_index() {
    use id_index::{TwoLevelIdIndex, BlockRange, DenseIdIndex};
    use std::collections::BTreeMap;

    let mut index = TwoLevelIdIndex::new(100);

    // Add sparse entries
    index.sparse_index.insert(
        "id_0000".to_string(),
        BlockRange {
            start_id: "id_0000".to_string(),
            end_id: "id_0099".to_string(),
            dense_index_id: 0,
        },
    );

    // Add dense index
    let mut dense = DenseIdIndex {
        start_id: "id_0000".to_string(),
        end_id: "id_0099".to_string(),
        entries: BTreeMap::new(),
    };

    for i in 0..100 {
        dense.entries.insert(format!("id_{:04}", i), i);
    }

    index.dense_indexes.push(dense);

    // Test lookup
    assert_eq!(index.lookup("id_0050"), Some(50));
    assert_eq!(index.lookup("id_0099"), Some(99));
    assert_eq!(index.lookup("id_0100"), None);
}
