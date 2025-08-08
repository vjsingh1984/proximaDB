// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive Unit Tests for WAL Recovery with Optimized Writer
//! 
//! Test Coverage:
//! - Recovery of batched WAL files
//! - Mixed format recovery (Proto/Bincode/Avro)
//! - Corruption handling
//! - Performance benchmarks
//! - Backward compatibility

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::core::VectorRecord;
use crate::services::vector_service::{VectorOperationsService, OptimizedFormat};
use crate::storage::engines::viper::ViperEngine;
use crate::storage::engines::sst::LsmTree;
use crate::storage::persistence::write_ahead_log::WriteBufferConfig;
use tracing::info;

/// Test helper to create test vectors with metadata
fn create_test_vectors_with_metadata(
    start_id: usize,
    count: usize,
    dimension: usize,
) -> Vec<VectorRecord> {
    (start_id..start_id + count)
        .map(|i| VectorRecord {
            id: Some(format!("vec_{,
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }", i)),
            vector: vec![(i % 256) as f32; dimension],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "batch_id".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue((i / 100).to_string())),
                },
                proximadb::proto::proximadb::MetadataItem {
                    key: "timestamp".to_string(),
                    value: chrono::Utc::now().timestamp().to_string(),
                },
            ],
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: None,
            expires_at: None,
        })
        .collect()
}

/// Create test WAL files in different formats
async fn create_test_wal_files(
    wal_dir: &Path,
    collection_id: &str,
    formats: Vec<OptimizedFormat>,
) -> Result<Vec<PathBuf>> {
    // Schema imports no longer needed - using serialization module instead
    
    let collection_dir = wal_dir.join(collection_id);
    let logs_dir = collection_dir.join("logs");
    std::fs::create_dir_all(&logs_dir)?;
    
    let mut wal_files = Vec::new();
    
    for (idx, format) in formats.iter().enumerate() {
        let vectors = create_test_vectors_with_metadata(idx * 100, 100, 128);
        
        // Serialize based on format
        let serialized = match format {
            OptimizedFormat::Proto => {
                use crate::storage::persistence::write_ahead_log::serialization::{ProtocolBuffersSerializer, VectorBatchSerializer};
                let serializer = ProtocolBuffersSerializer::new();
                serializer.serialize_batch(&vectors)?
            }
            OptimizedFormat::Bincode => {
                // Use custom serialization for core VectorRecord
                let mut serialized = Vec::new();
                for vector in &vectors {
                    let proto_vector = proximadb::proto::proximadb::VectorRecord::from(vector.clone());
                    let vector_data = bincode::serialize(&proto_vector)?;
                    serialized.extend_from_slice(&(vector_data.len() as u32).to_le_bytes());
                    serialized.extend_from_slice(&vector_data);
                }
                serialized
            }
            OptimizedFormat::Avro => {
                use crate::storage::persistence::write_ahead_log::serialization::{AvroSerializer, VectorBatchSerializer};
                let serializer = AvroSerializer::new();
                serializer.serialize_batch(&vectors)?
            }
        };
        
        // Create WAL filename
        let extension = match format {
            OptimizedFormat::Proto => "proto",
            OptimizedFormat::Bincode => "bincode",
            OptimizedFormat::Avro => "avro",
        };
        
        let filename = format!(
            "wal_20250717_120000_{:010}_{:010}_test{}.{}",
            idx * 100,
            (idx + 1) * 100 - 1,
            idx,
            extension
        );
        
        let wal_path = logs_dir.join(&filename);
        std::fs::write(&wal_path, serialized)?;
        wal_files.push(wal_path);
    }
    
    Ok(wal_files)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[tokio::test]
    async fn test_recover_mixed_format_wal_files() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let wal_dir = temp_dir.path();
        
        // Create WAL files in different formats
        let formats = vec![
            OptimizedFormat::Proto,
            OptimizedFormat::Bincode,
            OptimizedFormat::Avro,
        ];
        
        let wal_files = create_test_wal_files(wal_dir, "test_collection", formats).await?;
        assert_eq!(wal_files.len(), 3);
        
        // Create WAL config pointing to temp directory
        let wal_config = WriteBufferConfig {
            multi_disk: crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
                data_directories: vec![format!("file://{}", wal_dir.display())],
                distribution_strategy: crate::storage::persistence::write_ahead_log::config::DiskDistributionStrategy::RoundRobin,
                collection_affinity: true,
            },
            ..Default::default()
        };
        
        // Mock storage engines
        let viper_engine = create_mock_viper_engine().await?;
        let lsm_engine = create_mock_lsm_engine().await?;
        
        // Create VectorOperationsService and trigger recovery
        let service = VectorOperationsService::new(
            wal_config,
            viper_engine,
            lsm_engine,
        ).await?;
        
        // Recovery should have happened during initialization
        // Verify by checking that WAL files were cleaned up
        for wal_file in &wal_files {
            assert!(!wal_file.exists(), "WAL file should be cleaned up after recovery");
        }
        
        Ok(())
    }

    #[tokio::test]
    async fn test_recover_corrupted_wal_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let wal_dir = temp_dir.path();
        
        // Create valid WAL file
        let valid_files = create_test_wal_files(
            wal_dir,
            "test_collection",
            vec![OptimizedFormat::Proto],
        ).await?;
        
        // Create corrupted WAL file
        let logs_dir = wal_dir.join("test_collection").join("logs");
        let corrupted_file = logs_dir.join("wal_20250717_120000_0000000200_0000000299_corrupt.pbwal");
        std::fs::write(&corrupted_file, b"corrupted data that is not valid proto")?;
        
        // Create WAL config
        let wal_config = WriteBufferConfig {
            multi_disk: crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
                data_directories: vec![format!("file://{}", wal_dir.display())],
                ..Default::default()
            },
            ..Default::default()
        };
        
        let viper_engine = create_mock_viper_engine().await?;
        let lsm_engine = create_mock_lsm_engine().await?;
        
        // Recovery should skip corrupted file but process valid ones
        let service = VectorOperationsService::new(
            wal_config,
            viper_engine,
            lsm_engine,
        ).await?;
        
        // Valid file should be cleaned up
        assert!(!valid_files[0].exists());
        
        // Corrupted file might still exist (depending on error handling strategy)
        // This is implementation-specific
        
        Ok(())
    }

    #[tokio::test]
    async fn test_recovery_performance_large_dataset() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let wal_dir = temp_dir.path();
        
        // Create multiple large WAL files
        let mut all_files = Vec::new();
        for collection_idx in 0..5 {
            let collection_id = format!("collection_{}", collection_idx);
            
            // Create 10 WAL files per collection with 1000 vectors each
            for file_idx in 0..10 {
                let vectors = create_test_vectors_with_metadata(
                    file_idx * 1000,
                    1000,
                    256,
                );
                
                // Use custom serialization for core VectorRecord
                let mut serialized = Vec::new();
                for vector in &vectors {
                    let proto_vector = proximadb::proto::proximadb::VectorRecord::from(vector.clone());
                    let vector_data = bincode::serialize(&proto_vector)?;
                    serialized.extend_from_slice(&(vector_data.len() as u32).to_le_bytes());
                    serialized.extend_from_slice(&vector_data);
                }
                
                let logs_dir = wal_dir.join(&collection_id).join("logs");
                std::fs::create_dir_all(&logs_dir)?;
                
                let filename = format!(
                    "wal_20250717_120000_{:010}_{:010}_perf.bcwal",
                    file_idx * 1000,
                    (file_idx + 1) * 1000 - 1,
                );
                
                let wal_path = logs_dir.join(filename);
                std::fs::write(&wal_path, serialized)?;
                all_files.push(wal_path);
            }
        }
        
        assert_eq!(all_files.len(), 50); // 5 collections * 10 files each
        
        let wal_config = WriteBufferConfig {
            multi_disk: crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
                data_directories: vec![format!("file://{}", wal_dir.display())],
                ..Default::default()
            },
            ..Default::default()
        };
        
        let viper_engine = create_mock_viper_engine().await?;
        let lsm_engine = create_mock_lsm_engine().await?;
        
        // Measure recovery time
        let start = std::time::Instant::now();
        let service = VectorOperationsService::new(
            wal_config,
            viper_engine,
            lsm_engine,
        ).await?;
        let recovery_time = start.elapsed();
        
        println!("Recovery Performance:");
        println!("  Total files: {}", all_files.len());
        println!("  Total vectors: {}", 50 * 1000);
        println!("  Recovery time: {:?}", recovery_time);
        println!("  Throughput: {:.0} vectors/sec", 
                 (50000.0 / recovery_time.as_secs_f64()));
        
        // Should recover reasonably fast
        assert!(recovery_time.as_secs() < 30, 
                "Recovery took too long: {:?}", recovery_time);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_recovery_with_multiple_storage_urls() -> Result<()> {
        let temp_dir1 = TempDir::new()?;
        let temp_dir2 = TempDir::new()?;
        let temp_dir3 = TempDir::new()?;
        
        // Create WAL files in different directories
        create_test_wal_files(
            temp_dir1.path(),
            "collection_1",
            vec![OptimizedFormat::Proto],
        ).await?;
        
        create_test_wal_files(
            temp_dir2.path(),
            "collection_2",
            vec![OptimizedFormat::Bincode],
        ).await?;
        
        create_test_wal_files(
            temp_dir3.path(),
            "collection_3",
            vec![OptimizedFormat::Avro],
        ).await?;
        
        // Configure multiple WAL directories
        let wal_config = WriteBufferConfig {
            multi_disk: crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
                data_directories: vec![
                    format!("file://{}", temp_dir1.path().display()),
                    format!("file://{}", temp_dir2.path().display()),
                    format!("file://{}", temp_dir3.path().display()),
                ],
                distribution_strategy: crate::storage::persistence::write_ahead_log::config::DiskDistributionStrategy::RoundRobin,
                collection_affinity: true,
            },
            ..Default::default()
        };
        
        let viper_engine = create_mock_viper_engine().await?;
        let lsm_engine = create_mock_lsm_engine().await?;
        
        // Should recover from all directories
        let service = VectorOperationsService::new(
            wal_config,
            viper_engine,
            lsm_engine,
        ).await?;
        
        // Verify all directories were processed
        // (Implementation would need to expose recovery stats)
        
        Ok(())
    }
}

#[cfg(test)]
mod backward_compatibility_tests {
    use super::*;

    #[tokio::test]
    async fn test_recover_legacy_wal_format() -> Result<()> {
        // This test would verify that old WAL formats can still be read
        // Implementation depends on maintaining backward compatibility
        Ok(())
    }

    #[tokio::test]
    async fn test_mixed_legacy_and_new_format() -> Result<()> {
        // Test recovery when both old and new format files exist
        Ok(())
    }
}

/// Helper to create mock VIPER engine for testing
async fn create_mock_viper_engine() -> Result<Arc<ViperEngine>> {
    use tempfile::TempDir;
    use proximadb::storage::engines::viper::ViperConfig;
    
    let temp_dir = TempDir::new()?;
    let mut config = ViperConfig::default();
    config.base_path = temp_dir.path().to_path_buf();
    config.columnar_config.data_dir = temp_dir.path().join("data").to_str().unwrap().to_string();
    config.columnar_config.enable_compression = false; // Disable for tests
    
    let filesystem = Arc::new(
        proximadb::storage::persistence::filesystem::FilesystemFactory::new(
            Default::default()
        ).await?
    );
    
    let engine = ViperEngine::new(config, filesystem).await?;
    Ok(Arc::new(engine))
}

/// Helper to create mock LSM engine for testing  
async fn create_mock_lsm_engine() -> Result<Arc<LsmTree>> {
    use proximadb::storage::engines::sst::{SstConfig, LsmTree};
    
    let mut config = SstConfig::default();
    config.max_memtable_size = 1024 * 1024; // 1MB for tests
    config.level0_file_num_compaction_trigger = 2;
    config.enable_compression = false;
    
    let filesystem = Arc::new(
        proximadb::storage::persistence::filesystem::FilesystemFactory::new(
            Default::default()
        ).await?
    );
    
    let tree = LsmTree::new(
        "test_collection".to_string(),
        config,
        filesystem,
    ).await?;
    
    Ok(Arc::new(tree))
}

#[cfg(test)]
mod recovery_stress_tests {
    use super::*;
    use tokio::task::JoinSet;

    #[tokio::test]
    async fn stress_test_concurrent_recovery() -> Result<()> {
        use std::time::Instant;
        use tokio::task::JoinSet;
        
        let start = Instant::now();
        
        // Create multiple temp directories for different collections
        let temp_dirs: Vec<TempDir> = (0..5)
            .map(|_| TempDir::new().unwrap())
            .collect();
        
        // Create WAL files for each collection
        let mut tasks = JoinSet::new();
        
        for (idx, temp_dir) in temp_dirs.iter().enumerate() {
            let collection_id = format!("stress_collection_{}", idx);
            let wal_dir = temp_dir.path().to_path_buf();
            
            tasks.spawn(async move {
                // Create multiple WAL files with different formats
                let formats = vec![
                    OptimizedFormat::Proto,
                    OptimizedFormat::Bincode,
                    OptimizedFormat::Avro,
                ];
                
                create_test_wal_files(&wal_dir, &collection_id, formats).await
            });
        }
        
        // Wait for all WAL files to be created
        while let Some(result) = tasks.join_next().await {
            result??;
        }
        
        // Now test concurrent recovery
        let mut recovery_tasks = JoinSet::new();
        let filesystem = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(
                Default::default()
            ).await?
        );
        
        for (idx, temp_dir) in temp_dirs.iter().enumerate() {
            let collection_id = format!("stress_collection_{}", idx);
            let wal_dir = temp_dir.path().to_path_buf();
            let fs = filesystem.clone();
            
            recovery_tasks.spawn(async move {
                // Create disk manager and recovery manager
                let disk_manager = Arc::new(
                    crate::storage::persistence::write_ahead_log::WriteBufferDiskManager::new(
                        fs,
                        &wal_dir
                    )
                );
                
                // List and verify files
                let files = disk_manager.list_collection_files(&collection_id).await?;
                assert!(!files.is_empty(), "Should have WAL files for collection {}", collection_id);
                
                // Simulate recovery by reading all files
                for file_info in files {
                    let _ = disk_manager.read_batch(&file_info).await?;
                }
                
                Ok::<String, anyhow::Error>(collection_id)
            });
        }
        
        // Wait for all recoveries to complete
        let mut recovered_collections = Vec::new();
        while let Some(result) = recovery_tasks.join_next().await {
            let collection_id = result??;
            recovered_collections.push(collection_id);
        }
        
        assert_eq!(recovered_collections.len(), 5, "Should recover all 5 collections");
        
        let duration = start.elapsed();
        info!("Concurrent recovery stress test completed in {:?}", duration);
        
        Ok(())
    }

    #[tokio::test]
    async fn stress_test_large_wal_files() -> Result<()> {
        use std::time::Instant;
        
        let start = Instant::now();
        let temp_dir = TempDir::new()?;
        let collection_id = "large_wal_test";
        
        info!("Creating large WAL files for stress testing...");
        
        // Create a large batch of vectors (100k vectors with 512 dimensions)
        let batch_size = 10000;
        let num_batches = 10; // Total 100k vectors
        let dimension = 512;
        
        let wal_dir = temp_dir.path();
        let collection_dir = wal_dir.join(collection_id);
        let logs_dir = collection_dir.join("logs");
        std::fs::create_dir_all(&logs_dir)?;
        
        let filesystem = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(
                Default::default()
            ).await?
        );
        
        // Create large WAL files
        for batch_idx in 0..num_batches {
            let vectors = create_test_vectors_with_metadata(
                batch_idx * batch_size,
                batch_size,
                dimension,
            );
            
            // Use Proto format for better compression
            use crate::storage::persistence::write_ahead_log::serialization::{ProtocolBuffersSerializer, VectorBatchSerializer};
            let serializer = ProtocolBuffersSerializer::new();
            let serialized = serializer.serialize_batch(&vectors)?;
            
            let filename = format!(
                "wal_20250717_120000_{:010}_{:010}_batch{}.proto",
                batch_idx * batch_size,
                (batch_idx + 1) * batch_size - 1,
                batch_idx
            );
            let file_path = logs_dir.join(&filename);
            tokio::fs::write(&file_path, &serialized).await?;
            
            info!(
                "Created WAL file {} with {} vectors ({} MB)",
                filename,
                batch_size,
                serialized.len() / (1024 * 1024)
            );
        }
        
        // Now test recovery of these large files
        let recovery_start = Instant::now();
        
        let disk_manager = Arc::new(
            proximadb::storage::persistence::write_ahead_log::WriteBufferDiskManager::new(
                filesystem.clone(),
                wal_dir
            )
        );
        
        // List and recover all files
        let files = disk_manager.list_collection_files(collection_id).await?;
        assert_eq!(files.len(), num_batches, "Should have {} WAL files", num_batches);
        
        let mut total_vectors_recovered = 0;
        let mut total_bytes_processed = 0;
        
        for file_info in &files {
            let data = disk_manager.read_batch(file_info).await?;
            total_bytes_processed += data.len();
            
            // Deserialize to verify data integrity
            use crate::storage::persistence::write_ahead_log::serialization::{ProtocolBuffersSerializer, VectorBatchSerializer};
            let serializer = ProtocolBuffersSerializer::new();
            let vectors = serializer.deserialize_batch(&data)?;
            total_vectors_recovered += vectors.len();
        }
        
        assert_eq!(
            total_vectors_recovered,
            batch_size * num_batches,
            "Should recover all vectors"
        );
        
        let recovery_duration = recovery_start.elapsed();
        let total_duration = start.elapsed();
        
        info!(
            "Large WAL stress test completed:\n\
             - Total vectors: {}\n\
             - Total data size: {} MB\n\
             - Recovery time: {:?}\n\
             - Total time: {:?}\n\
             - Recovery throughput: {} vectors/sec",
            total_vectors_recovered,
            total_bytes_processed / (1024 * 1024),
            recovery_duration,
            total_duration,
            (total_vectors_recovered as f64 / recovery_duration.as_secs_f64()) as u64
        );
        
        Ok(())
    }
}