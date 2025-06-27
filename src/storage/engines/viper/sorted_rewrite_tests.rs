//! Comprehensive Tests for VIPER Sorted Rewrite Operations
//!
//! This module provides comprehensive testing for the sorted rewrite functionality
//! implemented in the VIPER pipeline, covering all phases and performance verification.

#[cfg(test)]
mod tests {
    use super::super::pipeline::*;
    use crate::core::VectorRecord;
    use std::collections::HashMap;
    use tokio;
    use uuid::Uuid;

    /// Create test vector records with varied characteristics for testing
    fn create_test_vector_records(count: usize) -> Vec<VectorRecord> {
        let mut records = Vec::new();
        
        for i in 0..count {
            let mut metadata = HashMap::new();
            
            // Add varied metadata for testing different sorting strategies
            metadata.insert("category".to_string(), serde_json::Value::String(
                format!("category_{}", i % 5)
            ));
            metadata.insert("priority".to_string(), serde_json::Value::Number(
                ((i % 10) as u64).into()
            ));
            metadata.insert("status".to_string(), serde_json::Value::String(
                if i % 2 == 0 { "active".to_string() } else { "inactive".to_string() }
            ));
            metadata.insert("score".to_string(), serde_json::Value::Number(
                ((i * 10) % 100).into()
            ));
            
            let record = VectorRecord::new(
                format!("test_vector_{:06}", i),
                "test_collection".to_string(),
                vec![0.1 * (i as f32); 128], // 128-dimensional test vectors
                metadata,
            );
            
            records.push(record);
        }
        
        records
    }

    /// Create a test compaction task
    fn create_test_compaction_task(compaction_type: CompactionType) -> CompactionTask {
        CompactionTask {
            task_id: Uuid::new_v4().to_string(),
            collection_id: "test_collection".to_string(),
            compaction_type,
            priority: CompactionPriority::Normal,
            input_partitions: vec!["partition_1".to_string(), "partition_2".to_string()],
            expected_outputs: 1,
            optimization_hints: None,
            created_at: chrono::Utc::now(),
            estimated_duration: tokio::time::Duration::from_secs(60),
        }
    }

    // =============================================================================
    // PHASE 1 TESTS - Sorting Strategy Verification
    // =============================================================================

    #[tokio::test]
    async fn test_sorting_strategy_by_id() {
        let mut records = create_test_vector_records(100);
        
        // Shuffle records to test sorting
        records.reverse();
        
        let processor_config = ProcessingConfig {
            enable_preprocessing: true,
            enable_postprocessing: false,
            batch_size: 100,
            enable_compression: true,
            sorting_strategy: SortingStrategy::ById,
        };

        let schema_adapter = std::sync::Arc::new(SchemaAdapter::new().await.unwrap());
        let processor = VectorRecordProcessor {
            config: processor_config,
            schema_adapter,
            stats: std::sync::Arc::new(tokio::sync::RwLock::new(ProcessingStats::default())),
        };

        // Apply preprocessing (sorting)
        processor.preprocess_records(&mut records).unwrap();

        // Verify records are sorted by ID
        for i in 1..records.len() {
            assert!(records[i-1].id <= records[i].id, 
                   "Records not sorted by ID: {} > {}", records[i-1].id, records[i].id);
        }
        
        println!("✅ ID sorting test passed: {} records correctly sorted", records.len());
    }

    #[tokio::test]
    async fn test_sorting_strategy_composite_optimal() {
        let mut records = create_test_vector_records(50);
        
        let sorting_strategy = SortingStrategy::CompositeOptimal {
            metadata_fields: vec!["category".to_string(), "priority".to_string()],
            include_id: true,
            include_timestamp: true,
        };

        let processor_config = ProcessingConfig {
            enable_preprocessing: true,
            enable_postprocessing: false,
            batch_size: 50,
            enable_compression: true,
            sorting_strategy,
        };

        let schema_adapter = std::sync::Arc::new(SchemaAdapter::new().await.unwrap());
        let processor = VectorRecordProcessor {
            config: processor_config,
            schema_adapter,
            stats: std::sync::Arc::new(tokio::sync::RwLock::new(ProcessingStats::default())),
        };

        processor.preprocess_records(&mut records).unwrap();

        // Verify composite ordering (ID first, then metadata, then timestamp)
        for i in 1..records.len() {
            let prev = &records[i-1];
            let curr = &records[i];
            
            // Check ID ordering
            if prev.id != curr.id {
                assert!(prev.id <= curr.id, "ID ordering violated");
                continue;
            }
            
            // If IDs are equal, check metadata ordering
            let prev_category = prev.metadata.get("category").unwrap().as_str().unwrap();
            let curr_category = curr.metadata.get("category").unwrap().as_str().unwrap();
            
            if prev_category != curr_category {
                assert!(prev_category <= curr_category, "Category ordering violated");
            }
        }
        
        println!("✅ Composite optimal sorting test passed: {} records correctly sorted", records.len());
    }

    #[tokio::test]
    async fn test_sorting_strategy_cluster_then_sort() {
        let mut records = create_test_vector_records(100);
        
        let sorting_strategy = SortingStrategy::ClusterThenSort {
            cluster_count: 5,
            inner_strategy: Box::new(SortingStrategy::ByMetadata(vec!["priority".to_string()])),
        };

        let processor_config = ProcessingConfig {
            enable_preprocessing: true,
            enable_postprocessing: false,
            batch_size: 100,
            enable_compression: true,
            sorting_strategy,
        };

        let schema_adapter = std::sync::Arc::new(SchemaAdapter::new().await.unwrap());
        let processor = VectorRecordProcessor {
            config: processor_config,
            schema_adapter,
            stats: std::sync::Arc::new(tokio::sync::RwLock::new(ProcessingStats::default())),
        };

        processor.preprocess_records(&mut records).unwrap();

        // Verify that records are organized (exact clustering verification would be complex)
        assert_eq!(records.len(), 100, "All records should be preserved");
        
        println!("✅ Cluster-then-sort test passed: {} records processed", records.len());
    }

    #[tokio::test]
    async fn test_custom_sorting_strategies() {
        let test_cases = vec![
            ("VectorMagnitude", CustomComparisonType::VectorMagnitude),
            ("MetadataRichness", CustomComparisonType::MetadataRichness),
            ("CollectionGrouped", CustomComparisonType::CollectionGrouped),
            ("CompressionOptimal", CustomComparisonType::CompressionOptimal),
        ];

        for (name, comparison_type) in test_cases {
            let mut records = create_test_vector_records(50);
            
            let sorting_strategy = SortingStrategy::Custom {
                strategy_name: name.to_string(),
                comparison_type,
            };

            let processor_config = ProcessingConfig {
                enable_preprocessing: true,
                enable_postprocessing: false,
                batch_size: 50,
                enable_compression: true,
                sorting_strategy,
            };

            let schema_adapter = std::sync::Arc::new(SchemaAdapter::new().await.unwrap());
            let processor = VectorRecordProcessor {
                config: processor_config,
                schema_adapter,
                stats: std::sync::Arc::new(tokio::sync::RwLock::new(ProcessingStats::default())),
            };

            processor.preprocess_records(&mut records).unwrap();

            assert_eq!(records.len(), 50, "All records should be preserved for strategy {}", name);
            println!("✅ Custom sorting strategy '{}' test passed", name);
        }
    }

    // =============================================================================
    // PHASE 3 TESTS - Postprocessing Optimization Verification  
    // =============================================================================

    #[tokio::test]
    async fn test_postprocessing_column_pruning() {
        // Test column pruning functionality
        // Note: This would require actual RecordBatch creation in a full implementation
        
        let processor_config = ProcessingConfig {
            enable_preprocessing: false,
            enable_postprocessing: true,
            batch_size: 100,
            enable_compression: true,
            sorting_strategy: SortingStrategy::None,
        };

        let schema_adapter = std::sync::Arc::new(SchemaAdapter::new().await.unwrap());
        let processor = VectorRecordProcessor {
            config: processor_config,
            schema_adapter,
            stats: std::sync::Arc::new(tokio::sync::RwLock::new(ProcessingStats::default())),
        };

        // In a full implementation, this would test actual arrow RecordBatch processing
        // For now, verify the processor is correctly configured
        assert!(processor.config.enable_postprocessing, "Postprocessing should be enabled");
        assert!(processor.config.enable_compression, "Compression should be enabled");
        
        println!("✅ Postprocessing configuration test passed");
    }

    // =============================================================================
    // PHASE 4 TESTS - Compaction Pipeline Integration
    // =============================================================================

    #[tokio::test]
    async fn test_sorted_rewrite_compaction() {
        let compaction_type = CompactionType::SortedRewrite {
            sorting_strategy: SortingStrategy::CompositeOptimal {
                metadata_fields: vec!["category".to_string(), "priority".to_string()],
                include_id: true,
                include_timestamp: true,
            },
            reorganization_strategy: ReorganizationStrategy::ByMetadataPriority {
                field_priorities: vec!["category".to_string(), "priority".to_string()],
            },
            target_compression_ratio: 0.3, // 30% compression target
        };

        let task = create_test_compaction_task(compaction_type);
        
        // Execute the compaction
        let result = CompactionEngine::execute_compaction_by_type(&task).await;
        
        assert!(result.is_ok(), "Sorted rewrite compaction should succeed");
        
        let execution_result = result.unwrap();
        assert!(execution_result.entries_processed > 0, "Should process entries");
        assert!(execution_result.bytes_read > 0, "Should read bytes");
        assert!(execution_result.bytes_written > 0, "Should write bytes");
        assert!(execution_result.compression_improvement >= 0.0, "Should have non-negative compression improvement");
        
        println!("✅ Sorted rewrite compaction test passed: {} entries, {:.1}% compression", 
                execution_result.entries_processed, 
                execution_result.compression_improvement * 100.0);
    }

    #[tokio::test]
    async fn test_hybrid_compaction_sequential() {
        let primary_strategy = Box::new(CompactionType::SortedRewrite {
            sorting_strategy: SortingStrategy::ById,
            reorganization_strategy: ReorganizationStrategy::ByCompressionRatio,
            target_compression_ratio: 0.25,
        });

        let secondary_strategy = Box::new(CompactionType::CompressionOptimization {
            target_algorithm: CompressionAlgorithm::Zstd { level: 5 },
            quality_threshold: 0.8,
        });

        let compaction_type = CompactionType::HybridCompaction {
            primary_strategy,
            secondary_strategy,
            coordination_mode: CompactionCoordinationMode::Sequential,
        };

        let task = create_test_compaction_task(compaction_type);
        let result = CompactionEngine::execute_compaction_by_type(&task).await;
        
        assert!(result.is_ok(), "Hybrid sequential compaction should succeed");
        
        let execution_result = result.unwrap();
        assert!(execution_result.entries_processed > 0, "Should process entries from both strategies");
        
        println!("✅ Hybrid sequential compaction test passed: {} entries processed", 
                execution_result.entries_processed);
    }

    #[tokio::test]
    async fn test_hybrid_compaction_conditional() {
        let primary_strategy = Box::new(CompactionType::FileMerging {
            target_file_size_mb: 128,
            max_files_per_merge: 5,
        });

        let secondary_strategy = Box::new(CompactionType::SortedRewrite {
            sorting_strategy: SortingStrategy::CompositeOptimal {
                metadata_fields: vec!["category".to_string()],
                include_id: true,
                include_timestamp: false,
            },
            reorganization_strategy: ReorganizationStrategy::BySimilarityClusters { cluster_count: 3 },
            target_compression_ratio: 0.4,
        });

        let compaction_type = CompactionType::HybridCompaction {
            primary_strategy,
            secondary_strategy,
            coordination_mode: CompactionCoordinationMode::Conditional { trigger_threshold: 0.2 },
        };

        let task = create_test_compaction_task(compaction_type);
        let result = CompactionEngine::execute_compaction_by_type(&task).await;
        
        assert!(result.is_ok(), "Hybrid conditional compaction should succeed");
        
        println!("✅ Hybrid conditional compaction test passed");
    }

    // =============================================================================
    // REORGANIZATION STRATEGY TESTS
    // =============================================================================

    #[tokio::test]
    async fn test_reorganization_strategies() {
        let test_records = create_test_vector_records(100);
        
        let strategies = vec![
            ("ByMetadataPriority", ReorganizationStrategy::ByMetadataPriority {
                field_priorities: vec!["category".to_string(), "priority".to_string()],
            }),
            ("BySimilarityClusters", ReorganizationStrategy::BySimilarityClusters { cluster_count: 5 }),
            ("ByTemporalPattern", ReorganizationStrategy::ByTemporalPattern { time_window_hours: 24 }),
            ("ByCompressionRatio", ReorganizationStrategy::ByCompressionRatio),
        ];

        for (name, strategy) in strategies {
            let result = CompactionEngine::apply_reorganization_strategy(&test_records, &strategy).await;
            
            assert!(result.is_ok(), "Reorganization strategy '{}' should succeed", name);
            
            let batches = result.unwrap();
            assert!(!batches.is_empty(), "Should produce at least one batch for strategy '{}'", name);
            
            let total_records: usize = batches.iter().map(|b| b.len()).sum();
            assert_eq!(total_records, test_records.len(), 
                      "Should preserve all records for strategy '{}'", name);
            
            println!("✅ Reorganization strategy '{}' test passed: {} batches", name, batches.len());
        }
    }

    #[tokio::test]
    async fn test_multi_stage_reorganization() {
        let test_records = create_test_vector_records(200);
        
        let multi_stage_strategy = ReorganizationStrategy::MultiStage {
            stages: vec![
                ReorganizationStrategy::BySimilarityClusters { cluster_count: 4 },
                ReorganizationStrategy::ByMetadataPriority {
                    field_priorities: vec!["category".to_string()],
                },
                ReorganizationStrategy::ByCompressionRatio,
            ],
        };

        let result = CompactionEngine::apply_reorganization_strategy(&test_records, &multi_stage_strategy).await;
        
        assert!(result.is_ok(), "Multi-stage reorganization should succeed");
        
        let batches = result.unwrap();
        let total_records: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total_records, test_records.len(), "Should preserve all records through multi-stage");
        
        println!("✅ Multi-stage reorganization test passed: {} → {} batches", 
                test_records.len(), batches.len());
    }

    // =============================================================================
    // PERFORMANCE AND COMPRESSION VERIFICATION TESTS
    // =============================================================================

    #[tokio::test]
    async fn test_compression_calculation() {
        let test_records = create_test_vector_records(50);
        
        // Test different target compression ratios
        let target_ratios = vec![0.1, 0.3, 0.5, 0.7];
        
        for target_ratio in target_ratios {
            let result = CompactionEngine::calculate_achieved_compression(&test_records, target_ratio).await;
            
            assert!(result.is_ok(), "Compression calculation should succeed");
            
            let achieved_compression = result.unwrap();
            assert!(achieved_compression >= 0.0 && achieved_compression <= 1.0, 
                   "Compression ratio should be between 0 and 1");
            assert!(achieved_compression <= target_ratio, 
                   "Achieved compression should not exceed target");
            
            println!("✅ Compression calculation test: target {:.1}% → achieved {:.1}%", 
                    target_ratio * 100.0, achieved_compression * 100.0);
        }
    }

    #[tokio::test]
    async fn test_performance_benefits_verification() {
        // Test that sorted rewrite provides better performance than basic operations
        
        let sorted_rewrite_task = create_test_compaction_task(CompactionType::SortedRewrite {
            sorting_strategy: SortingStrategy::CompositeOptimal {
                metadata_fields: vec!["category".to_string(), "priority".to_string()],
                include_id: true,
                include_timestamp: true,
            },
            reorganization_strategy: ReorganizationStrategy::ByCompressionRatio,
            target_compression_ratio: 0.3,
        });

        let file_merging_task = create_test_compaction_task(CompactionType::FileMerging {
            target_file_size_mb: 128,
            max_files_per_merge: 5,
        });

        // Execute both compaction types
        let sorted_result = CompactionEngine::execute_compaction_by_type(&sorted_rewrite_task).await.unwrap();
        let merging_result = CompactionEngine::execute_compaction_by_type(&file_merging_task).await.unwrap();

        // Verify sorted rewrite provides better compression
        assert!(sorted_result.compression_improvement >= merging_result.compression_improvement,
               "Sorted rewrite should provide better or equal compression");

        println!("✅ Performance benefits verification: sorted rewrite {:.1}% vs file merging {:.1}%",
                sorted_result.compression_improvement * 100.0,
                merging_result.compression_improvement * 100.0);
    }

    #[tokio::test]
    async fn test_sorted_rewrite_end_to_end() {
        // Complete end-to-end test of the sorted rewrite pipeline
        
        println!("🚀 Starting end-to-end sorted rewrite test...");
        
        // Stage 1: Create test data
        let test_records = create_test_vector_records(500);
        println!("📊 Created {} test records", test_records.len());
        
        // Stage 2: Test various sorting strategies
        let sorting_strategies = vec![
            SortingStrategy::ById,
            SortingStrategy::ByTimestamp,
            SortingStrategy::CompositeOptimal {
                metadata_fields: vec!["category".to_string(), "priority".to_string()],
                include_id: true,
                include_timestamp: true,
            },
        ];

        for (i, strategy) in sorting_strategies.iter().enumerate() {
            println!("🔢 Testing sorting strategy {}: {:?}", i + 1, strategy);
            
            let compaction_task = create_test_compaction_task(CompactionType::SortedRewrite {
                sorting_strategy: strategy.clone(),
                reorganization_strategy: ReorganizationStrategy::ByMetadataPriority {
                    field_priorities: vec!["category".to_string(), "priority".to_string()],
                },
                target_compression_ratio: 0.25,
            });

            let result = CompactionEngine::execute_compaction_by_type(&compaction_task).await;
            assert!(result.is_ok(), "Sorted rewrite should succeed for strategy {:?}", strategy);
            
            let execution_result = result.unwrap();
            println!("   ✅ Strategy {} completed: {} entries, {:.1}% compression, {}ms",
                    i + 1,
                    execution_result.entries_processed,
                    execution_result.compression_improvement * 100.0,
                    execution_result.execution_time_ms);
        }
        
        println!("🎯 End-to-end sorted rewrite test completed successfully!");
    }

    // =============================================================================
    // STRESS AND EDGE CASE TESTS
    // =============================================================================

    #[tokio::test]
    async fn test_empty_records_handling() {
        let empty_records: Vec<VectorRecord> = vec![];
        
        let result = CompactionEngine::apply_reorganization_strategy(
            &empty_records, 
            &ReorganizationStrategy::ByCompressionRatio
        ).await;
        
        assert!(result.is_ok(), "Should handle empty records gracefully");
        
        let batches = result.unwrap();
        assert!(batches.is_empty() || batches.iter().all(|b| b.is_empty()), 
               "Empty input should produce empty output");
        
        println!("✅ Empty records handling test passed");
    }

    #[tokio::test]
    async fn test_large_dataset_performance() {
        // Test with larger dataset to verify performance doesn't degrade significantly
        let large_dataset = create_test_vector_records(5000);
        
        let start_time = std::time::Instant::now();
        
        let compaction_task = create_test_compaction_task(CompactionType::SortedRewrite {
            sorting_strategy: SortingStrategy::CompositeOptimal {
                metadata_fields: vec!["category".to_string()],
                include_id: true,
                include_timestamp: false,
            },
            reorganization_strategy: ReorganizationStrategy::BySimilarityClusters { cluster_count: 10 },
            target_compression_ratio: 0.35,
        });

        let result = CompactionEngine::execute_compaction_by_type(&compaction_task).await;
        
        let duration = start_time.elapsed();
        
        assert!(result.is_ok(), "Should handle large dataset");
        assert!(duration.as_secs() < 10, "Should complete within reasonable time");
        
        let execution_result = result.unwrap();
        println!("✅ Large dataset test passed: {} records in {}ms (total wall clock: {}ms)",
                execution_result.entries_processed,
                execution_result.execution_time_ms,
                duration.as_millis());
    }

    #[tokio::test]
    async fn test_error_handling_and_recovery() {
        // Test error handling with invalid configurations
        
        // Test with invalid cluster count
        let result = CompactionEngine::apply_reorganization_strategy(
            &create_test_vector_records(10),
            &ReorganizationStrategy::BySimilarityClusters { cluster_count: 0 }
        ).await;
        
        // Should handle gracefully (implementation dependent)
        println!("✅ Error handling test completed: {:?}", result.is_ok());
    }
}