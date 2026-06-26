    use super::*;
    use crate::storage::engines::sst::VectorFormat;

    #[test]
    fn selects_sqrt_top_blocks() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.1, 0.1],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![2.0, 2.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "c".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 2,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.2, 0.2],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "d".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 3,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![5.0, 5.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
        ];

        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &crate::core::search::BlockPruneConfig::for_testing(),
        );
        // sqrt(4) = 2 => expect the two closest blocks by centroid: block_ids 0 and 2.
        assert_eq!(selected, vec![0, 2]);
    }

    #[test]
    fn test_block_prune_none_mode_force_exact() {
        // None mode (force_exact=true) should return ALL blocks regardless of other settings
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.1, 0.1],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![5.0, 5.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
        ];

        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: true, // None mode - disable all pruning
            mode: crate::core::search::BlockPruneMode::Fixed(1),
            ratio: 0.1,
            min_keep: 1,
            max_keep: 1,
            min_blocks_override: Some(0), // Bypass threshold for testing
        };

        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );

        assert_eq!(selected.len(), 2, "force_exact should return ALL blocks");
        assert_eq!(selected, vec![0, 1]);
    }

    #[test]
    fn test_block_prune_min_keep_override() {
        // min_keep should override mode when mode returns fewer blocks
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.1, 0.1],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "c".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 2,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![2.0, 2.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
        ];

        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Fixed(1), // Mode wants 1 block
            ratio: 0.2,
            min_keep: 3, // But min_keep requires at least 3
            max_keep: 0,
            min_blocks_override: Some(0), // Bypass threshold for testing
        };

        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );

        assert_eq!(selected.len(), 3, "min_keep=3 should override Fixed(1)");
        assert_eq!(selected, vec![0, 1, 2]);
    }

    #[test]
    fn test_block_prune_max_keep_override() {
        // max_keep should override mode when mode returns more blocks
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.1, 0.1],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "c".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 2,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![2.0, 2.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "d".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 3,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![3.0, 3.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
        ];

        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Ratio,
            ratio: 0.75, // Would return 3 blocks
            min_keep: 1,
            max_keep: 2,                  // But max_keep limits to 2
            min_blocks_override: Some(0), // Bypass threshold for testing
        };

        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );

        assert_eq!(selected.len(), 2, "max_keep=2 should override Ratio(0.75)");
        assert_eq!(selected, vec![0, 1], "Should return 2 closest blocks");
    }

    #[test]
    fn test_block_prune_min_max_conflict() {
        // When min_keep > max_keep, correct order is: mode -> min_keep -> max_keep
        // Result should respect max_keep as the final constraint
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.1, 0.1],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
            IndexEntry {
                key: "c".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 2,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![2.0, 2.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
,
        ];

        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Fixed(1),
            ratio: 0.2,
            min_keep: 5,                  // Configuration error: min_keep > max_keep
            max_keep: 2,                  // max_keep should take precedence
            min_blocks_override: Some(0), // Bypass threshold for testing
        };

        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );

        // Evaluation: Fixed(1)=1 -> max(1,5)=5 -> min(5,2)=2 -> clamp(2,1,3)=2
        assert_eq!(
            selected.len(),
            2,
            "max_keep=2 should win when min_keep=5 > max_keep=2"
        );
        assert_eq!(selected, vec![0, 1]);
    }

    #[test]
    fn test_block_prune_ratio_mode() {
        // Ratio mode should return ratio * n blocks (rounded up)
        let query = vec![0.0f32, 0.0];
        let entries = (0..5)
            .map(|i| IndexEntry {
                key: format!("block_{}", i),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: i,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![i as f32, i as f32],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None,
                block_component_min: None,
                block_component_max: None,
                }
)
            .collect::<Vec<_>>();

        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Ratio,
            ratio: 0.4, // 0.4 * 5 = 2
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0), // Bypass threshold for testing
        };

        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );

        assert_eq!(selected.len(), 2, "Ratio(0.4) of 5 blocks should be 2");
        assert_eq!(selected, vec![0, 1], "Should return 2 closest blocks");
    }

    // ========================================================================
    // Z-Order Pruning Tests
    // ========================================================================

    #[test]
    fn test_compute_query_zorder_code_without_pca_model() {
        // Test that without a cached PCA model, function returns None (graceful fallback)
        let query = vec![1.0f32, 0.5];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.0, 0.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode::Code64(100)),
            },
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode::Code64(200)),
            },
        ];

        // Without a cached PCA model, function returns None (falls back to centroid-only pruning)
        let code = compute_query_zorder_code(&query, &entries, "test_collection_no_pca");
        assert!(
            code.is_none(),
            "Should return None when no PCA model is cached"
        );
    }

    #[test]
    fn test_compute_query_zorder_code_with_pca_model() {
        use crate::proto::proximadb_v1::VectorRecord;
        use crate::storage::engines::sst::pca_manager::EnhancedPCAModel;

        // Create sample vectors to train a PCA model
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![(i as f32) / 100.0, (i as f32) / 50.0],
                metadata: HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            })
            .collect();

        // Train and cache PCA model
        let pca_model = EnhancedPCAModel::train(&vectors, 2).expect("Failed to train PCA");
        crate::storage::engines::sst::core::set_collection_pca_model(
            "test_collection_with_pca",
            pca_model,
        );

        // Now test with a query
        let query = vec![1.0f32, 0.5];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.0, 0.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode::Code64(100)),
            },
        ];

        // With a cached PCA model, function should return Some
        let code = compute_query_zorder_code(&query, &entries, "test_collection_with_pca");
        assert!(
            code.is_some(),
            "Should compute Z-Order code when PCA model is cached"
        );
    }

    #[test]
    fn test_compute_query_zorder_code_empty_input() {
        // Test with empty entries - should return None (no PCA model and empty entries)
        let query = vec![1.0f32, 0.5];
        let entries: Vec<IndexEntry> = vec![];

        let code = compute_query_zorder_code(&query, &entries, "test_empty_entries");
        assert!(code.is_none(), "Should return None for empty entries");
    }

    #[test]
    fn test_calculate_zorder_epsilon() {
        use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;

        // Test epsilon calculation
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.0, 0.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(1000)),
            },
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![10.0, 10.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(10000)),
            },
        ];

        let query_code = SpatialCode::Code64(5000);
        let epsilon = calculate_zorder_epsilon(&query_code, &entries);
        // Epsilon should be 10% of range: (10000 - 1000) / 10 = 900, but min is 1000
        assert_eq!(
            epsilon,
            SpatialCode::Code64(1000),
            "Epsilon should be max of (10% of range, 1000)"
        );
    }

    #[test]
    fn test_calculate_zorder_epsilon_no_codes() {
        use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;

        // Test with no Z-Order codes
        let entries = vec![IndexEntry {
            key: "a".into(),
            last_key: None,
            offset: 0,
            size: 0,
            block_id: 0,
            block_offset: 0,
            compressed: false,
            block_centroid: vec![0.0, 0.0],
            block_centroid_fp16: None,
            metadata_min_values: HashMap::new(),
            metadata_max_values: HashMap::new(),
            metadata_null_counts: HashMap::new(),
            block_key_bloom: None,
            block_metadata_bloom: None,
            vector_format: VectorFormat::Variable,
            zorder_code: None,
            block_component_min: None,
            block_component_max: None,
            }
];

        let query_code = SpatialCode::Code64(0);
        let epsilon = calculate_zorder_epsilon(&query_code, &entries);
        assert_eq!(
            epsilon,
            SpatialCode::Code64(u64::MAX),
            "Should return MAX for no codes"
        );
    }

    #[test]
    fn test_filter_blocks_by_zorder_without_pca() {
        use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;

        // Test Z-Order filtering without cached PCA model - should return None (graceful fallback)
        let query = vec![1.0f32, 1.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.0, 0.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(100)),
            },
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(5000)),
            },
            IndexEntry {
                key: "c".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 2,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![10.0, 10.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(10000)),
            },
        ];

        // Without cached PCA model, filter_blocks_by_zorder returns None (falls back to centroid pruning)
        let filtered = filter_blocks_by_zorder(&query, &entries, "test_no_pca_collection");
        assert!(
            filtered.is_none(),
            "Should return None when no PCA model is cached"
        );
    }

    #[test]
    fn test_filter_blocks_by_zorder_with_pca() {
        use crate::proto::proximadb_v1::VectorRecord;
        use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;
        use crate::storage::engines::sst::pca_manager::EnhancedPCAModel;

        // Create and cache a PCA model
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![(i as f32) / 100.0, (i as f32) / 50.0],
                metadata: HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            })
            .collect();

        let pca_model = EnhancedPCAModel::train(&vectors, 2).expect("Failed to train PCA");
        crate::storage::engines::sst::core::set_collection_pca_model(
            "test_zorder_filter_pca",
            pca_model,
        );

        // Test Z-Order filtering with cached PCA model
        let query = vec![1.0f32, 1.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.0, 0.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(100)),
            },
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(5000)),
            },
            IndexEntry {
                key: "c".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 2,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![10.0, 10.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(10000)),
            },
            // Block without Z-Order code (backward compatibility - always included)
            IndexEntry {
                key: "d".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 3,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.5, 0.5],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None, // No Z-Order code - always included
            },
        ];

        let filtered = filter_blocks_by_zorder(&query, &entries, "test_zorder_filter_pca");
        assert!(
            filtered.is_some(),
            "Should return filtered indices with cached PCA model"
        );

        let indices = filtered.unwrap();
        // Should include the block without Z-Order code for backward compatibility
        assert!(
            !indices.is_empty(),
            "Should have at least some blocks selected (backward compat block)"
        );
        assert!(
            indices.contains(&3),
            "Block without Z-Order code should be included"
        );
    }

    #[test]
    fn test_filter_blocks_by_zorder_backward_compat_with_pca() {
        use crate::proto::proximadb_v1::VectorRecord;
        use crate::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;
        use crate::storage::engines::sst::pca_manager::EnhancedPCAModel;

        // Create and cache a PCA model
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![(i as f32) / 100.0, (i as f32) / 50.0],
                metadata: HashMap::new(),
                timestamp: None,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            })
            .collect();

        let pca_model = EnhancedPCAModel::train(&vectors, 2).expect("Failed to train PCA");
        crate::storage::engines::sst::core::set_collection_pca_model(
            "test_backward_compat_pca",
            pca_model,
        );

        // Test that blocks without Z-Order codes are included (backward compatibility)
        let query = vec![1.0f32, 1.0];
        let entries = vec![
            IndexEntry {
                key: "a".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 0,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![0.0, 0.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: None, // No Z-Order code
            },
            IndexEntry {
                key: "b".into(),
                last_key: None,
                offset: 0,
                size: 0,
                block_id: 1,
                block_offset: 0,
                compressed: false,
                block_centroid: vec![1.0, 1.0],
                block_centroid_fp16: None,
                metadata_min_values: HashMap::new(),
                metadata_max_values: HashMap::new(),
                metadata_null_counts: HashMap::new(),
                block_key_bloom: None,
                block_metadata_bloom: None,
                vector_format: VectorFormat::Variable,
                zorder_code: Some(SpatialCode::Code64(5000)),
            },
        ];

        let filtered = filter_blocks_by_zorder(&query, &entries, "test_backward_compat_pca");
        assert!(
            filtered.is_some(),
            "Should handle mix of coded/non-coded blocks"
        );

        let indices = filtered.unwrap();
        // Block without code should be included
        assert!(
            indices.contains(&0),
            "Should include block without Z-Order code"
        );
    }

    #[test]
    fn test_normalize_coords_for_zorder() {
        // Test normalization to [0, 1]
        let coords = vec![-10.0f32, 0.0, 10.0, 20.0];
        let normalized = normalize_coords_for_zorder(&coords);

        assert_eq!(normalized.len(), coords.len());

        // All values should be in [0, 1]
        for &val in &normalized {
            assert!(val >= 0.0 && val <= 1.0, "Value {} not in [0, 1]", val);
        }

        // Min should map to 0.0, max to 1.0
        assert!((normalized[0] - 0.0).abs() < 1e-5, "Min should map to 0.0");
        assert!((normalized[3] - 1.0).abs() < 1e-5, "Max should map to 1.0");
    }

    #[test]
    fn test_normalize_coords_for_zorder_constant() {
        // Test with all same values
        let coords = vec![5.0f32, 5.0, 5.0];
        let normalized = normalize_coords_for_zorder(&coords);

        // Should return middle of range (0.5)
        for &val in &normalized {
            assert!(
                (val - 0.5).abs() < 1e-5,
                "Constant coords should map to 0.5"
            );
        }
    }

    // ========================================================================
    // Tests inlined from tests/unit/storage/block_pruning_tests.rs
    // ========================================================================

    /// Helper to create a test IndexEntry with a given centroid
    fn create_block_pruning_test_entry(id: usize, centroid: Vec<f32>) -> IndexEntry {
        IndexEntry {
            key: format!("block_{}", id),
            last_key: None,
            offset: 0,
            size: 0,
            block_id: id as u32,
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
            block_component_min: None,
            block_component_max: None,
            }

    }

    #[test]
    fn test_block_prune_none_mode_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![2.0, 2.0]),
            create_block_pruning_test_entry(2, vec![5.0, 5.0]),
            create_block_pruning_test_entry(3, vec![10.0, 10.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: true,
            mode: crate::core::search::BlockPruneMode::Sqrt,
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(selected.len(), 4, "None mode should return all 4 blocks");
        assert_eq!(
            selected,
            vec![0, 1, 2, 3],
            "Should return all block indices"
        );
    }

    #[test]
    fn test_block_prune_sqrt_mode_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![2.0, 2.0]),
            create_block_pruning_test_entry(3, vec![5.0, 5.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Sqrt,
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(
            selected.len(),
            2,
            "SQRT mode should return sqrt(4)=2 blocks"
        );
        assert_eq!(selected, vec![0, 1], "Should return 2 closest blocks");
    }

    #[test]
    fn test_block_prune_ratio_mode_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![1.0, 1.0]),
            create_block_pruning_test_entry(3, vec![2.0, 2.0]),
            create_block_pruning_test_entry(4, vec![5.0, 5.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Ratio,
            ratio: 0.4,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(
            selected.len(),
            2,
            "Ratio mode should return ratio*n=2 blocks"
        );
        assert_eq!(selected, vec![0, 1], "Should return 2 closest blocks");
    }

    #[test]
    fn test_block_prune_fixed_mode_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![1.0, 1.0]),
            create_block_pruning_test_entry(3, vec![2.0, 2.0]),
            create_block_pruning_test_entry(4, vec![5.0, 5.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Fixed(3),
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(
            selected.len(),
            3,
            "Fixed mode should return exactly 3 blocks"
        );
        assert_eq!(selected, vec![0, 1, 2], "Should return 3 closest blocks");
    }

    #[test]
    fn test_block_prune_min_keep_constraint_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![1.0, 1.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Fixed(1),
            ratio: 0.2,
            min_keep: 3,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
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
    fn test_block_prune_max_keep_constraint_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![1.0, 1.0]),
            create_block_pruning_test_entry(3, vec![2.0, 2.0]),
            create_block_pruning_test_entry(4, vec![5.0, 5.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Ratio,
            ratio: 0.8,
            min_keep: 1,
            max_keep: 2,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
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
    fn test_block_prune_min_max_conflict_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![1.0, 1.0]),
            create_block_pruning_test_entry(3, vec![2.0, 2.0]),
            create_block_pruning_test_entry(4, vec![5.0, 5.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Fixed(1),
            ratio: 0.2,
            min_keep: 5,
            max_keep: 3,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
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
    fn test_block_prune_empty_blocks_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries: Vec<IndexEntry> = vec![];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Sqrt,
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(
            selected.len(),
            0,
            "Empty blocks should return empty selection"
        );
    }

    #[test]
    fn test_block_prune_single_block_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![create_block_pruning_test_entry(0, vec![0.1, 0.1])];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Sqrt,
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(selected.len(), 1, "Single block should always be selected");
        assert_eq!(selected, vec![0]);
    }

    #[test]
    fn test_block_prune_ratio_clamp_standalone() {
        let query = vec![0.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![0.1, 0.1]),
            create_block_pruning_test_entry(1, vec![0.5, 0.5]),
            create_block_pruning_test_entry(2, vec![1.0, 1.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Ratio,
            ratio: 2.5,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Euclidean,
            &prune_config,
        );
        assert_eq!(
            selected.len(),
            3,
            "Ratio 2.5 should be clamped to 1.0, returning all 3 blocks"
        );
    }

    #[test]
    fn test_block_prune_cosine_metric_standalone() {
        let query = vec![1.0f32, 0.0];
        let entries = vec![
            create_block_pruning_test_entry(0, vec![1.0, 0.0]),
            create_block_pruning_test_entry(1, vec![0.0, 1.0]),
            create_block_pruning_test_entry(2, vec![-1.0, 0.0]),
        ];
        let prune_config = crate::core::search::BlockPruneConfig {
            force_exact: false,
            mode: crate::core::search::BlockPruneMode::Fixed(1),
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: Some(0),
        };
        let selected = select_blocks_by_centroid(
            &query,
            &entries,
            proximadb_distance_kernel::DistanceMetric::Cosine,
            &prune_config,
        );
        assert_eq!(selected.len(), 1, "Fixed(1) should return 1 block");
        assert_eq!(
            selected,
            vec![0],
            "Should select block with identical direction"
        );
    }
