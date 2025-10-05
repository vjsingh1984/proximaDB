//! NOVA Core Tests - Consolidated from inline test modules
//!
//! This module contains core engine functionality tests migrated from inline
//! #[cfg(test)] modules in NOVA engine source files.
//!
//! Sources:
//! - optimized_operations.rs (2 tests)
//! - nova_meta_reader.rs (2 tests)
//! - mod.rs (2 tests)
//! - engine.rs (2 tests)
//! - unified_strategy_reader.rs (1 test)
//!
//! Total: 11 tests

use std::sync::Arc;
use crate::storage::engines::impls::nova::*;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::compute::distance_computation::DistanceMetric;

// ============================================================================
// OPTIMIZED OPERATIONS TESTS (from optimized_operations.rs)
// ============================================================================

#[tokio::test]
async fn test_optimized_viper_operations() {
    // Initialize hardware capabilities
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let ops = optimized_operations::OptimizedNovaOperations::new().unwrap();
    // Test compute mode selection
    let small_vectors = vec![vec![0.0; 128]; 10];
    let mode = ops.select_compute_mode(&small_vectors);
    assert_eq!(mode, crate::compute::distance_computation::DistanceMode::Normalized);
    let large_vectors = vec![vec![0.0; 768]; 1000];
    let mode = ops.select_compute_mode(&large_vectors);
    assert_eq!(mode, crate::compute::distance_computation::DistanceMode::Normalized);
}

#[test]
fn test_projection_mask() {
    let config = columnar_search::ColumnarSearchConfig {
        enable_progressive_search: true,
        max_candidates: 10000,
        search_mode: columnar_search::SearchMode::Progressive,
        ..Default::default()
    };

    let ops = optimized_operations::OptimizedNovaOperations::new().unwrap();
    let projection = ops.build_projection_mask(&config);
    assert!(projection.contains(&"id".to_string()));
    assert!(projection.contains(&"vector".to_string()));
    // Progressive search is always enabled in config
    assert!(projection.contains(&"vector_binary".to_string()));
    assert!(projection.contains(&"vector_int8".to_string()));
    assert!(projection.contains(&"vector_pq".to_string()));
}

// ============================================================================
// NOVA META READER TESTS (from nova_meta_reader.rs)
// ============================================================================

#[tokio::test]
async fn test_vector_bounds_check() {
    let reader = nova_meta_reader::NovaMetaReader::new(Arc::new(FilesystemFactory::new(
        Default::default()
    ).await.unwrap()));

    let vector = vec![1.0, 2.0, 3.0];
    let min_values = vec![0.0, 1.0, 2.0];
    let max_values = vec![2.0, 3.0, 4.0];

    assert!(reader.vector_in_bounds(&vector, &min_values, &max_values));

    let out_of_bounds = vec![3.0, 4.0, 5.0];
    assert!(!reader.vector_in_bounds(&out_of_bounds, &min_values, &max_values));
}

#[tokio::test]
async fn test_euclidean_distance() {
    let reader = nova_meta_reader::NovaMetaReader::new(Arc::new(FilesystemFactory::new(
        Default::default()
    ).await.unwrap()));

    let v1 = vec![1.0, 2.0, 3.0];
    let v2 = vec![4.0, 5.0, 6.0];

    let distance = reader.euclidean_distance(&v1, &v2);
    assert!((distance - 5.196).abs() < 0.01); // sqrt(27) ≈ 5.196
}

// ============================================================================
// MOD.RS TESTS (from mod.rs)
// ============================================================================

#[test]
fn test_create_vector_schema() {
    use crate::storage::engines::core::formats::columnar::QuantizationConfig;

    let config = QuantizationConfig::default();
    let filterable = vec!["category".to_string(), "price".to_string()];

    let schema = create_vector_schema(768, &config, &filterable);

    // Check core fields
    assert!(schema.field_with_name("id").is_ok());
    assert!(schema.field_with_name("vector").is_ok());
    assert!(schema.field_with_name("timestamp").is_ok());

    // Check quantized fields based on actual config
    // Default config has binary and int8 enabled, but not PQ
    if config.enable_binary.unwrap_or(false) {
        assert!(schema.field_with_name("vector_binary").is_ok());
    }
    if config.enable_int8.unwrap_or(false) {
        assert!(schema.field_with_name("vector_int8").is_ok());
    }
    // PQ is disabled by default, so this field won't exist
    if config.enable_pq.unwrap_or(false) {
        assert!(schema.field_with_name("vector_pq").is_ok());
    }

    // Check metadata fields
    assert!(schema.field_with_name("category").is_ok());
    assert!(schema.field_with_name("price").is_ok());
}

#[test]
fn test_quantization_config() {
    use crate::storage::engines::core::formats::columnar::QuantizationConfig;

    let config = QuantizationConfig::default();

    // Test protobuf QuantizationConfig default values
    // Default proto values are all false/0
    assert!(!config.enabled);  // Proto bools default to false
    assert_eq!(config.strategy, 0);  // Proto enums default to 0 (SMART_DEFAULTS)
    assert!(config.custom_levels.is_empty());  // Proto repeated fields default to empty
    assert!(!config.enable_progressive_search);
    assert_eq!(config.binary_filter_selectivity, 0.0);
    assert_eq!(config.int8_ranking_selectivity, 0.0);
    assert_eq!(config.pq_ranking_selectivity, 0.0);
}

// ============================================================================
// ENGINE.RS TESTS (from engine.rs)
// ============================================================================

#[tokio::test]
async fn test_nova_engine_creation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let engine = engine::NovaEngine::new().await.unwrap();
    assert_eq!(engine.engine_name(), "NOVA");
    assert_eq!(engine.engine_version(), "1.0.0");
}

#[tokio::test]
async fn test_nova_feature_support() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let engine = engine::NovaEngine::new().await.unwrap();
    assert!(engine.supports_feature("id_lookup"));
    assert!(engine.supports_feature("columnar_search"));
    assert!(engine.supports_feature("predicate_pushdown"));
    assert!(engine.supports_feature("projection"));
    assert!(!engine.supports_feature("unknown_feature"));
}

// ============================================================================
// UNIFIED STRATEGY READER TESTS (from unified_strategy_reader.rs)
// ============================================================================

#[test]
fn test_nova_strategy_to_pruning() {
    use crate::storage::engines::core::read_strategy::ReadAccessStrategy;
    use unified_strategy_reader::UnifiedNOVAReader;

    let direct = ReadAccessStrategy::DirectStream;
    let pruning = UnifiedNOVAReader::to_nova_pruning_strategy(&direct);
    assert!(matches!(pruning, zone_maps::PruningStrategy::NoPruning));

    let search = ReadAccessStrategy::CachedSearch { prefetch_metadata: true };
    let pruning = UnifiedNOVAReader::to_nova_pruning_strategy(&search);
    assert!(matches!(pruning, zone_maps::PruningStrategy::Hierarchical(_)));
}
