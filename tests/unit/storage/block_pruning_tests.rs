//! Block Pruning Tests
//!
//! Comprehensive tests for block pruning modes in SST and SWIFT engines.
//! Tests all modes: None (force_exact), SQRT, Ratio, Fixed
//! Tests min_keep and max_keep constraints and edge cases.

use proximadb::core::search::{BlockPruneConfig, BlockPruneMode};
use proximadb::storage::engines::impls::sst::IndexEntry;
use proximadb::storage::engines::impls::sst::VectorFormat;
use std::collections::HashMap;

/// Helper to create a test IndexEntry with a given centroid
fn create_test_entry(id: usize, centroid: Vec<f32>) -> IndexEntry {
    IndexEntry {
        key: format!("block_{}", id),
        offset: 0,
        size: 0,
        block_id: id,
        block_offset: 0,
        compressed: false,
        block_centroid: centroid,
        block_centroid_fp16: None,
        metadata_min_values: HashMap::new(),
        metadata_max_values: HashMap::new(),
        metadata_null_counts: HashMap::new(),
        block_key_bloom: None,
        block_metadata_bloom: None,
        vector_format: VectorFormat::Variable,
        zorder_code: None,
    }
}

#[test]
fn test_block_prune_none_mode() {
    // None mode (force_exact=true) should return ALL blocks
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),
        create_test_entry(1, vec![2.0, 2.0]),
        create_test_entry(2, vec![5.0, 5.0]),
        create_test_entry(3, vec![10.0, 10.0]),
    ];

    let prune_config = BlockPruneConfig {
        force_exact: true, // None mode
        mode: BlockPruneMode::Sqrt,
        ratio: 0.2,
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(selected.len(), 4, "None mode should return all 4 blocks");
    assert_eq!(selected, vec![0, 1, 2, 3], "Should return all block indices");
}

#[test]
fn test_block_prune_sqrt_mode() {
    // SQRT mode should return sqrt(n) blocks
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),   // Closest
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![2.0, 2.0]),
        create_test_entry(3, vec![5.0, 5.0]),   // Farthest
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Sqrt,
        ratio: 0.2,
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    // sqrt(4) = 2, should select 2 closest blocks
    assert_eq!(selected.len(), 2, "SQRT mode should return sqrt(4)=2 blocks");
    assert_eq!(selected, vec![0, 1], "Should return 2 closest blocks");
}

#[test]
fn test_block_prune_ratio_mode() {
    // Ratio mode should return ratio * n blocks
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),   // Closest
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![1.0, 1.0]),
        create_test_entry(3, vec![2.0, 2.0]),
        create_test_entry(4, vec![5.0, 5.0]),   // Farthest
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Ratio,
        ratio: 0.4, // Keep 40% = 2 blocks
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    // 0.4 * 5 = 2, should select 2 closest blocks
    assert_eq!(selected.len(), 2, "Ratio mode should return ratio*n=2 blocks");
    assert_eq!(selected, vec![0, 1], "Should return 2 closest blocks");
}

#[test]
fn test_block_prune_fixed_mode() {
    // Fixed mode should return exactly k blocks
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![1.0, 1.0]),
        create_test_entry(3, vec![2.0, 2.0]),
        create_test_entry(4, vec![5.0, 5.0]),
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Fixed(3),
        ratio: 0.2,
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(selected.len(), 3, "Fixed mode should return exactly 3 blocks");
    assert_eq!(selected, vec![0, 1, 2], "Should return 3 closest blocks");
}

#[test]
fn test_block_prune_min_keep_constraint() {
    // min_keep should override mode if mode returns fewer blocks
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![1.0, 1.0]),
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Fixed(1), // Mode says 1 block
        ratio: 0.2,
        min_keep: 3, // But min_keep says at least 3
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(
        selected.len(),
        3,
        "min_keep=3 should override Fixed(1), returning all 3 blocks"
    );
    assert_eq!(selected, vec![0, 1, 2]);
}

#[test]
fn test_block_prune_max_keep_constraint() {
    // max_keep should override mode if mode returns more blocks
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![1.0, 1.0]),
        create_test_entry(3, vec![2.0, 2.0]),
        create_test_entry(4, vec![5.0, 5.0]),
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Ratio,
        ratio: 0.8, // Would return 4 blocks
        min_keep: 1,
        max_keep: 2, // But max_keep limits to 2
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(
        selected.len(),
        2,
        "max_keep=2 should override Ratio(0.8), returning only 2 blocks"
    );
    assert_eq!(selected, vec![0, 1]);
}

#[test]
fn test_block_prune_min_max_conflict() {
    // When min_keep > max_keep, min_keep should be applied first, then max_keep
    // Result should respect max_keep as the final constraint
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![1.0, 1.0]),
        create_test_entry(3, vec![2.0, 2.0]),
        create_test_entry(4, vec![5.0, 5.0]),
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Fixed(1),
        ratio: 0.2,
        min_keep: 5, // min_keep > max_keep (configuration error)
        max_keep: 3, // But max_keep should take precedence
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    // Order: Fixed(1) -> max(1,5)=5 -> min(5,3)=3 -> clamp(3,1,5)=3
    assert_eq!(
        selected.len(),
        3,
        "max_keep=3 should override min_keep=5 when they conflict"
    );
    assert_eq!(selected, vec![0, 1, 2]);
}

#[test]
fn test_block_prune_empty_blocks() {
    // Edge case: no blocks should return empty vec
    let query = vec![0.0f32, 0.0];
    let entries: Vec<IndexEntry> = vec![];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Sqrt,
        ratio: 0.2,
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(selected.len(), 0, "Empty blocks should return empty selection");
}

#[test]
fn test_block_prune_single_block() {
    // Edge case: single block should always be returned
    let query = vec![0.0f32, 0.0];
    let entries = vec![create_test_entry(0, vec![0.1, 0.1])];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Sqrt,
        ratio: 0.2,
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(
        selected.len(),
        1,
        "Single block should always be selected"
    );
    assert_eq!(selected, vec![0]);
}

#[test]
fn test_block_prune_ratio_clamp() {
    // Ratio mode with extreme values should be clamped to [0.0, 1.0]
    let query = vec![0.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![0.1, 0.1]),
        create_test_entry(1, vec![0.5, 0.5]),
        create_test_entry(2, vec![1.0, 1.0]),
    ];

    // Test ratio > 1.0 (should be clamped to 1.0)
    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Ratio,
        ratio: 2.5, // Invalid: > 1.0, should clamp to 1.0
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
        &prune_config,
    );

    assert_eq!(
        selected.len(),
        3,
        "Ratio 2.5 should be clamped to 1.0, returning all 3 blocks"
    );
}

#[test]
fn test_block_prune_cosine_metric() {
    // Test with cosine similarity metric (different from Euclidean)
    let query = vec![1.0f32, 0.0];
    let entries = vec![
        create_test_entry(0, vec![1.0, 0.0]),   // Identical direction (distance=0)
        create_test_entry(1, vec![0.0, 1.0]),   // Orthogonal (distance~1.0)
        create_test_entry(2, vec![-1.0, 0.0]),  // Opposite (distance=2.0)
    ];

    let prune_config = BlockPruneConfig {
        force_exact: false,
        mode: BlockPruneMode::Fixed(1),
        ratio: 0.2,
        min_keep: 1,
        max_keep: 0,
    };

    let selected = proximadb::storage::engines::impls::sst::readers::sst_query_engine::select_blocks_by_centroid(
        &query,
        &entries,
        proximadb::compute::distance_computation::DistanceMetric::Cosine,
        &prune_config,
    );

    assert_eq!(selected.len(), 1, "Fixed(1) should return 1 block");
    assert_eq!(
        selected,
        vec![0],
        "Should select block with identical direction"
    );
}
