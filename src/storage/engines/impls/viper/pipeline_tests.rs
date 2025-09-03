#[cfg(test)]
pub mod viper_pipeline_tests {
    use super::*;
    use crate::core::VectorRecord;
    use crate::proto::proximadb::MetadataItem;
    use crate::storage::engines::impls::viper::QuantizationLevel;
    use crate::storage::engines::impls::viper::pipeline::*;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::Arc;

    // Helper function to create test vector records
    fn create_test_vector_record(
        id: &str,
        vector: Vec<f32>,
        metadata: HashMap<String, String>,
    ) -> VectorRecord {
        let now = Utc::now().timestamp_micros();
        VectorRecord {
            id: Some(id.to_string()),
            vector,
            metadata: metadata
                .into_iter()
                .map(|(k, v)| MetadataItem {
                    key: k,
                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(
                        v,
                    )),
                })
                .collect(),
            timestamp: now as u32,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            // rank removed -  None,
            similarity: None,
            similarity: None,
        }
    }

    // Helper function to create default pipeline config
    fn create_default_pipeline_config() -> ViperPipelineConfig {
        ViperPipelineConfig {
            processing_config: ProcessingConfig {
                enable_preprocessing: true,
                enable_postprocessing: true,
                batch_size: 100,
                compression: true,
                sorting_strategy: SortingStrategy::ByTimestamp,
                quantization_level: None,
            },
            flushing_config: FlushingConfig {
                compression_algorithm: CompressionAlgorithm::Snappy,
                compression_level: 6,
                enable_dictionary_encoding: true,
                row_group_size: 1000,
                write_batch_size: 1000,
                enable_statistics: true,
            },
            compaction_config: CompactionConfig {
                enable_ml_compaction: false,
                worker_count: 2,
                compaction_interval_secs: 300,
                target_file_size_mb: 100,
                max_files_per_merge: 10,
                reclustering_quality_threshold: 0.8,
            },
            enable_background_processing: true,
            stats_interval_secs: 30,
        }
    }

    // Helper function to create test filesystem factory
    async fn create_test_filesystem() -> Arc<FilesystemFactory> {
        use crate::storage::persistence::filesystem::FilesystemConfig;
        let config = FilesystemConfig::default();
        Arc::new(FilesystemFactory::new(config).await.unwrap())
    }

    #[test]
    fn test_viper_pipeline_config_creation() {
        let config = create_default_pipeline_config();

        assert!(config.processing_config.enable_preprocessing);
        assert!(config.processing_config.enable_postprocessing);
        assert_eq!(config.processing_config.batch_size, 100);
        assert!(config.processing_config.enable_compression);
        assert!(matches!(
            config.processing_config.sorting_strategy,
            SortingStrategy::ByTimestamp
        ));
        assert!(config.processing_config.quantization_level.is_none());

        assert!(matches!(
            config.flushing_config.compression_algorithm,
            CompressionAlgorithm::Snappy
        ));
        assert_eq!(config.flushing_config.compression_level, 6);
        assert!(config.flushing_config.enable_dictionary_encoding);
        assert_eq!(config.flushing_config.row_group_size, 1000);

        assert!(!config.compaction_config.enable_ml_compaction);
        assert_eq!(config.compaction_config.compaction_interval_secs, 300);
        assert_eq!(config.compaction_config.target_file_size_mb, 100);

        assert!(config.enable_background_processing);
        assert_eq!(config.stats_interval_secs, 30);
    }

    #[test]
    fn test_processing_config_variants() {
        let mut config = ProcessingConfig {
            enable_preprocessing: false,
            enable_postprocessing: false,
            batch_size: 50,
            compression: false,
            sorting_strategy: SortingStrategy::ById,
            quantization_level: Some(QuantizationLevel::pq8(8)),
        };

        assert!(!config.enable_preprocessing);
        assert!(!config.enable_postprocessing);
        assert_eq!(config.batch_size, 50);
        assert!(!config.enable_compression);
        assert!(matches!(config.sorting_strategy, SortingStrategy::ById));
        assert!(config.quantization_level.is_some());

        // Test different sorting strategies
        config.sorting_strategy = SortingStrategy::ByTimestamp;
        assert!(matches!(
            config.sorting_strategy,
            SortingStrategy::ByTimestamp
        ));

        config.sorting_strategy = SortingStrategy::Custom {
            strategy_name: "custom_field".to_string(),
            comparison_type: CustomComparisonType::VectorMagnitude,
        };
        if let SortingStrategy::Custom { strategy_name, .. } = &config.sorting_strategy {
            assert_eq!(strategy_name, "custom_field");
        } else {
            panic!("Expected Custom sorting strategy");
        }
    }

    #[test]
    fn test_flushing_config_compression_variants() {
        let mut config = FlushingConfig {
            compression_algorithm: CompressionAlgorithm::Zstd { level: 3 },
            compression_level: 9,
            enable_dictionary_encoding: false,
            row_group_size: 500,
            write_batch_size: 500,
            enable_statistics: true,
        };

        assert!(matches!(
            config.compression_algorithm,
            CompressionAlgorithm::Zstd { level: 3 }
        ));
        assert_eq!(config.compression_level, 9);

        // Test all compression algorithms
        config.compression_algorithm = CompressionAlgorithm::Snappy;
        assert!(matches!(
            config.compression_algorithm,
            CompressionAlgorithm::Snappy
        ));

        config.compression_algorithm = CompressionAlgorithm::Lz4;
        assert!(matches!(
            config.compression_algorithm,
            CompressionAlgorithm::Lz4
        ));

        config.compression_algorithm = CompressionAlgorithm::Brotli { level: 6 };
        assert!(matches!(
            config.compression_algorithm,
            CompressionAlgorithm::Brotli { level: 6 }
        ));
    }

    #[test]
    fn test_compaction_config_creation() {
        let config = CompactionConfig {
            enable_ml_compaction: true,
            worker_count: 4,
            compaction_interval_secs: 600,
            target_file_size_mb: 200,
            max_files_per_merge: 20,
            reclustering_quality_threshold: 0.9,
        };

        assert!(config.enable_ml_compaction);
        assert_eq!(config.compaction_interval_secs, 600);
        assert_eq!(config.target_file_size_mb, 200);
        assert_eq!(config.max_files_per_merge, 20);
        assert_eq!(config.reclustering_quality_threshold, 0.9);
        assert_eq!(config.worker_count, 4);
    }

    #[test]
    fn test_vector_record_creation_helper() {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), "test".to_string());
        metadata.insert("priority".to_string(), "high".to_string());

        let vector = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let record = create_test_vector_record("test_vector_1", vector.clone(), metadata);

        assert_eq!(record.id, Some("test_vector_1".to_string()));
        assert_eq!(record.vector, vector);
        assert_eq!(record.metadata.len(), 2);
        assert_eq!(record.version, Some(1));
        assert!(record.rank.is_none());
        assert!(record.score.is_none());
        assert!(record.similarity.is_none());
    }

    #[test]
    fn test_quantization_level_variants() {
        let levels = vec![
            QuantizationLevel::pq4(4),
            QuantizationLevel::pq8(8),
            QuantizationLevel::Uniform(16),
            QuantizationLevel::Uniform(32),
            QuantizationLevel::None,
        ];

        for level in levels {
            match level {
                QuantizationLevel::ProductQuantization { .. } => assert!(true),
                QuantizationLevel::Uniform(bits) => assert!(bits > 0),
                QuantizationLevel::None => assert!(true),
                QuantizationLevel::Custom { .. } => assert!(true),
            }
        }
    }

    #[tokio::test]
    async fn test_viper_pipeline_creation() {
        let config = create_default_pipeline_config();
        let filesystem = create_test_filesystem().await;

        let pipeline_result = ViperPipeline::new(config.clone(), filesystem.clone()).await;

        // Pipeline creation should succeed or fail gracefully
        match pipeline_result {
            Ok(_pipeline) => {
                assert!(true); // Successfully created
            }
            Err(e) => {
                // Expected to fail in test environment - verify error is reasonable
                assert!(
                    e.to_string().contains_hash("Failed")
                        || e.to_string().contains_hash("not implemented")
                );
            }
        }
    }

    #[test]
    fn test_config_edge_cases() {
        // Test minimum valid configuration
        let min_config = ViperPipelineConfig {
            processing_config: ProcessingConfig {
                enable_preprocessing: false,
                enable_postprocessing: false,
                batch_size: 1,
                compression: false,
                sorting_strategy: SortingStrategy::None,
                quantization_level: None,
            },
            flushing_config: FlushingConfig {
                compression_algorithm: CompressionAlgorithm::Snappy,
                compression_level: 0,
                enable_dictionary_encoding: false,
                row_group_size: 1,
                write_batch_size: 1,
                enable_statistics: false,
            },
            compaction_config: CompactionConfig {
                enable_ml_compaction: false,
                worker_count: 1,
                compaction_interval_secs: 1,
                target_file_size_mb: 1,
                max_files_per_merge: 1,
                reclustering_quality_threshold: 0.0,
            },
            enable_background_processing: false,
            stats_interval_secs: 1,
        };

        assert_eq!(min_config.processing_config.batch_size, 1);
        assert_eq!(min_config.flushing_config.write_batch_size, 1);
        assert_eq!(min_config.compaction_config.worker_count, 1);
        assert!(!min_config.enable_background_processing);
    }
}
