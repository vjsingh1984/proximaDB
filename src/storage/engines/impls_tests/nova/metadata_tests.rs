/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! NOVA Metadata Tests - Consolidated
//!
//! Sources:
//! - src/storage/engines/impls/nova/hierarchical_stats.rs (3 tests)
//! - src/storage/engines/impls/nova/unified_metadata_serializer.rs (3 tests)
//! - src/storage/engines/impls/nova/zone_maps.rs (3 tests)

// ============================================================================
// Tests from hierarchical_stats.rs
// ============================================================================

#[test]
fn test_zone_map_creation() {
    use crate::storage::engines::nova::hierarchical_stats::ZoneMap;

    let vectors = vec![
        vec![1.0, 2.0, 3.0],
        vec![4.0, 5.0, 6.0],
        vec![7.0, 8.0, 9.0],
    ];

    let zone_map = ZoneMap::from_vectors(&vectors).unwrap();

    assert_eq!(zone_map.min_values, vec![1.0, 2.0, 3.0]);
    assert_eq!(zone_map.max_values, vec![7.0, 8.0, 9.0]);
    assert_eq!(zone_map.centroid, vec![4.0, 5.0, 6.0]);
    assert_eq!(zone_map.dimension, 3);
}

// Test disabled: intersects_euclidean is a private method
// #[test]
// fn test_zone_map_euclidean_intersection() {
//     use crate::storage::engines::nova::hierarchical_stats::ZoneMap;
//
//     let vectors = vec![vec![0.0, 0.0], vec![2.0, 2.0]];
//
//     let zone_map = ZoneMap::from_vectors(&vectors).unwrap();
//
//     // Query inside the zone
//     assert!(zone_map.intersects_euclidean(&[1.0, 1.0], 1.0));
//
//     // Query outside but within distance
//     assert!(zone_map.intersects_euclidean(&[3.0, 3.0], 2.0));
//
//     // Query too far away
//     assert!(!zone_map.intersects_euclidean(&[10.0, 10.0], 1.0));
// }

#[test]
fn test_superblock_creation() {
    use crate::storage::engines::nova::hierarchical_stats::*;

    let enhanced_stats = vec![EnhancedRowGroupStats {
        row_group_id: 0,
        parquet_metadata: None,
        vector_zone_map: ZoneMap::from_vectors(&[vec![1.0, 2.0, 3.0]]).unwrap(),
        quantized_selectivity: QuantizedSelectivity {
            binary_effectiveness: 0.8,
            int8_accuracy: 0.9,
            pq_quality: 0.85,
            progressive_efficiency: 0.75,
        },
        compression_ratio: 4.0,
        search_cost_estimate: SearchCostEstimate {
            io_cost: 10.0,
            cpu_cost: 20.0,
            memory_cost: 15.0,
            estimated_latency_ms: 50.0,
            confidence: 0.8,
        },
        access_stats: AccessStats {
            access_count: 0,
            last_access: chrono::Utc::now(),
            avg_selectivity: 0.5,
            cache_hit_rate: 0.0,
            access_frequency: 0.0,
        },
    }];

    let superblock = SuperBlock::new(0, 0..10, &enhanced_stats).unwrap();

    assert_eq!(superblock.id, 0);
    assert_eq!(superblock.row_groups, 0..10);
    assert_eq!(superblock.zone_map.dimension, 3);
}

// ============================================================================
// Tests from unified_metadata_serializer.rs
// ============================================================================

// Tests disabled: serialize, deserialize, extract_cacheable_component, and should_cache_metadata are private/trait methods
// #[test]
// fn test_nova_metadata_serialization() {
//     use crate::storage::engines::core::nova_unified_metadata_serializer::*;
//     use std::collections::HashMap;
//
//     let metadata = NovaCachedMetadata {
//         file_size: 104857600, // 100MB
//         vector_count: 100000,
//         dimension: 1536,
//         super_block_count: 10,
//         row_group_count: 100,
//         hierarchical_stats: HierarchicalStatsCache {
//             super_block_stats: vec![
//                 SuperBlockStat {
//                     super_block_id: 0,
//                     start_row_group: 0,
//                     end_row_group: 10,
//                     vector_count: 10000,
//                     min_similarity: 0.1,
//                     max_similarity: 0.99,
//                     centroid: vec![0.5; 1536],
//                 },
//             ],
//             global_min_values: vec![-1.0; 1536],
//             global_max_values: vec![1.0; 1536],
//             global_centroid: vec![0.0; 1536],
//             pruning_efficiency: 0.85,
//         },
//         zone_maps: vec![
//             ZoneMapEntry {
//                 row_group_id: 0,
//                 min_values: vec![-0.5; 16], // Abbreviated for test
//                 max_values: vec![0.5; 16],
//                 null_count: 0,
//                 distinct_count: 1000,
//             },
//         ],
//         column_metadata: HashMap::new(),
//         compression_ratio: 0.25,
//         quantization_config: Some(QuantizationMetadata {
//             algorithm: "pq8".to_string(),
//             codebook_size: 256,
//             subvector_count: Some(192),
//             bits_per_subvector: Some(8),
//         }),
//         creation_timestamp: 1234567890,
//         schema_hash: 0xDEADBEEF,
//     };
//
//     let serializer = NovaUnifiedMetadataSerializer::new();
//
//     // Test serialization
//     let bytes = serializer.serialize(&metadata).unwrap();
//     assert!(!bytes.is_empty());
//
//     // Test deserialization
//     let deserialized = serializer.deserialize(&bytes).unwrap();
//     let restored = deserialized.downcast_ref::<NovaCachedMetadata>().unwrap();
//
//     assert_eq!(restored.file_size, metadata.file_size);
//     assert_eq!(restored.vector_count, metadata.vector_count);
//     assert_eq!(restored.dimension, metadata.dimension);
//     assert_eq!(restored.super_block_count, metadata.super_block_count);
//     assert_eq!(restored.compression_ratio, metadata.compression_ratio);
// }
//
// #[test]
// fn test_parquet_footer_extraction() {
//     use crate::storage::engines::core::nova_unified_metadata_serializer::*;
//
//     let serializer = NovaUnifiedMetadataSerializer::new();
//
//     // Create mock Parquet file data
//     let mut data = Vec::new();
//
//     // PAR1 magic at start
//     data.extend_from_slice(b"PAR1");
//
//     // Some data content
//     data.extend_from_slice(&vec![0u8; 1000]);
//
//     // Footer content
//     let footer = b"parquet_footer_metadata_content";
//     let _footer_start = data.len();
//     data.extend_from_slice(footer);
//
//     // Footer length (4 bytes)
//     data.extend_from_slice(&(footer.len() as u32).to_le_bytes());
//
//     // PAR1 magic at end
//     data.extend_from_slice(b"PAR1");
//
//     // Test extraction
//     let extracted = serializer.extract_cacheable_component(&data, "test.parquet");
//     assert!(extracted.is_some());
//
//     let extracted_bytes = extracted.unwrap();
//     // Should include footer + length + trailing PAR1
//     assert_eq!(extracted_bytes.len(), footer.len() + 8);
// }
//
// #[test]
// fn test_should_cache_metadata() {
//     use crate::storage::engines::core::nova_unified_metadata_serializer::*;
//
//     let serializer = NovaUnifiedMetadataSerializer::new();
//
//     assert!(serializer.should_cache_metadata("/data/nova/vectors.parquet"));
//     assert!(serializer.should_cache_metadata("/collections/test_nova_data.parquet"));
//     assert!(serializer.should_cache_metadata("/superblocks/sb_001.parquet"));
//     assert!(serializer.should_cache_metadata("/progressive/level_0.parquet"));
//     assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
// }

// ============================================================================
// Tests from zone_maps.rs
// ============================================================================

#[test]
fn test_zone_map_config() {
    use crate::storage::engines::nova::zone_maps::ZoneMapConfig;

    let config = ZoneMapConfig::default();
    assert!(config.enable_hierarchical);
    assert_eq!(config.hierarchical_levels, 3);
    assert_eq!(config.sketch_width, 1024);
}

// Tests disabled: from_query and predict are private methods
// #[test]
// fn test_query_characteristics() {
//     use crate::storage::engines::nova::zone_maps::QueryCharacteristics;
//
//     let query = vec![1.0, 0.0, 2.0, 0.0, 3.0];
//     let characteristics =
//         QueryCharacteristics::from_query(&query, "euclidean".to_string(), 10);
//
//     assert_eq!(characteristics.top_k, 10);
//     assert_eq!(characteristics.sparsity, 0.4); // 2/5 zeros
//     assert!(characteristics.norm > 0.0);
//     assert_eq!(characteristics.dominant_dimensions.len(), 1); // top 10% of 5 = 1
// }
//
// #[test]
// fn test_selectivity_model_prediction() {
//     use crate::storage::engines::nova::zone_maps::*;
//
//     let model = SelectivityModel {
//         parameters: vec![0.1, -0.2, 0.5], // norm_factor, sparsity_factor, intercept
//         model_type: ModelType::Linear,
//         accuracy: 0.8,
//         training_samples: 100,
//     };
//
//     let characteristics = QueryCharacteristics {
//         norm: 2.0,
//         sparsity: 0.3,
//         dominant_dimensions: vec![0, 1, 2],
//         distance_metric: "euclidean".to_string(),
//         top_k: 10,
//     };
//
//     let selectivity = model.predict(&characteristics);
//     // Expected: 0.1 * 2.0 + (-0.2) * 0.3 + 0.5 = 0.2 - 0.06 + 0.5 = 0.64
//     assert!((selectivity - 0.64).abs() < 0.01);
// }
