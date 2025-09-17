//! Test SST compression with quantization features
//! 
//! This test validates that SST engine with integrated quantization
//! achieves better compression through block-based quantization.

use anyhow::Result;
use proximadb::proto::proximadb_v1::{VectorRecord, Collection, CollectionConfig, CompressionConfig};
use proximadb::storage::engines::impls::sst::SstStorage;
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::core::SstConfig;
use std::sync::Arc;
use tempfile::TempDir;
use tracing::info;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

// Use the unified test utilities for test environment only
mod common;
use common::integration_test_helpers::UnifiedTestEnvironment;

/// Test SST compression with integrated quantization features
#[tokio::test]
async fn test_sst_quantization_compression() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    info!("\n{}", "=".repeat(80));
    info!("🎯 TESTING SST WITH INTEGRATED QUANTIZATION");
    info!("{}", "=".repeat(80));
    
    // Test parameters
    let num_vectors = 5000;
    let dimension = 768; // BERT-like dimensions
    let num_clusters = 20;
    
    // Generate highly clustered vectors for better compression
    let vectors = generate_highly_clustered_vectors(num_vectors, dimension, num_clusters);
    
    // Test with different compression algorithms
    let compression_algorithms = vec![
        ("none", 0),
        ("snappy", 1),
        ("zstd", 3),
        ("lz4", 1),
    ];
    
    for (algorithm, level) in &compression_algorithms {
        info!("\n{}", "-".repeat(80));
        info!("📦 Testing {} compression (level {})", algorithm, level);
        
        // Create temporary directory
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path().to_str().unwrap();
        
        // Setup SST with specific configuration
        let mut sst_config = SstConfig::default();
        sst_config.block_size_kb = 256; // 256KB blocks for better quantization grouping
        sst_config.storage_config.as_ref().and_then(|s| s.compression.as_ref()) = algorithm.to_string();
        sst_config.compression_level = *level;
        
        // Create filesystem factory
        let fs_config = FilesystemConfig {
            default_fs: Some(format!("file://{}", base_path)),
        };
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
        
        // Create SST storage engine
        let distance_compute = Arc::new(
            proximadb::compute::distance_computation::engine::UnifiedDistanceCompute::new()
        );
        let sst_storage = SstStorage::new(
            sst_config.clone(),
            filesystem,
            distance_compute,
        ).await?;
        
        // Create collection config with compression
        let compression_config = CompressionConfig {
            algorithm: algorithm.to_string(),
            level: Some(*level),
            block_size: Some(sst_config.block_size_kb as i32),
            dictionary_size: None,
            window_size: None,
            strategy: None,
        };
        
        let collection_config = Collection {
            id: "test_collection".to_string(),
            name: "test_collection".to_string(),
            dimension: dimension as u32,
            distance_metric: "cosine".to_string(),
            storage_assignment: Some(proximadb::proto::proximadb_v1::StorageAssignment {
                base_location: format!("file://{}", base_path),
                assignment_id: "test".to_string(),
                tier: "hot".to_string(),
                region: "local".to_string(),
                rack_id: None,
                node_id: None,
                device_id: None,
                created_at: 0,
                last_modified: 0,
            }),
            config: Some(CollectionConfig {
                compression: Some(compression_config),
                ..Default::default()
            }),
            ..Default::default()
        };
        
        // Create flush parameters with vectors
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors.clone(),
            collection_config: Some(collection_config),
            force: true,
            synchronous: true,
            ..Default::default()
        };
        
        // Perform flush using production SST code
        let flush_result = sst_storage.do_flush(&flush_params).await?;
        
        info!("\n  📊 Results for {}:", algorithm);
        info!("    • Entries flushed: {}", flush_result.entries_flushed);
        info!("    • Bytes written: {:.2} MB", flush_result.bytes_written as f64 / (1024.0 * 1024.0));
        info!("    • Files created: {}", flush_result.files_created);
        
        // Calculate compression ratio
        let uncompressed_size = num_vectors * dimension * 4; // FP32
        let compression_ratio = if flush_result.bytes_written > 0 {
            uncompressed_size as f64 / flush_result.bytes_written as f64
        } else {
            1.0
        };
        info!("    • Compression ratio: {:.2}x", compression_ratio);
        
        // Verify data was written
        assert!(flush_result.success, "Flush should succeed");
        assert!(flush_result.entries_flushed > 0, "Should flush entries");
        assert!(flush_result.bytes_written > 0, "Should write bytes");
        
        if algorithm != &"none" {
            // With compression enabled, should achieve better ratio
            assert!(
                compression_ratio > 1.5,
                "{} compression should achieve >1.5x ratio (got {:.2}x)",
                algorithm,
                compression_ratio
            );
        }
    }
    
    info!("\n✅ SST QUANTIZATION COMPRESSION TEST COMPLETE");
    Ok(())
}


/// Generate highly clustered vectors for compression testing
fn generate_highly_clustered_vectors(count: usize, dim: usize, clusters: usize) -> Vec<VectorRecord> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut vectors = Vec::with_capacity(count);
    let vectors_per_cluster = count / clusters;
    
    for cluster_id in 0..clusters {
        // Generate tight cluster center
        let mut center = vec![0.0f32; dim];
        
        // Create distinct pattern for each cluster
        for i in 0..dim {
            center[i] = if i % clusters == cluster_id {
                1.0
            } else if (i + cluster_id) % 3 == 0 {
                0.5
            } else {
                0.0
            };
        }
        
        // Generate vectors very close to center (for better compression)
        for i in 0..vectors_per_cluster {
            let mut vector = center.clone();
            
            // Add very small noise (improves compression)
            for val in &mut vector {
                *val += rng.gen_range(-0.001..0.001);
            }
            
            // Normalize
            let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in &mut vector {
                    *val /= norm;
                }
            }
            
            let global_idx = cluster_id * vectors_per_cluster + i;
            vectors.push(VectorRecord {
                id: Some(format!("vec_{}", global_idx)),
                vector,
                metadata: vec![],
                timestamp: chrono::Utc::now().timestamp() as u32,
                updated_at: Some(chrono::Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            });
        }
    }
    
    // Fill remaining if needed
    while vectors.len() < count {
        let vector = vec![0.0f32; dim];
        vectors.push(VectorRecord {
            id: Some(format!("vec_{}", vectors.len())),
            vector,
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: Some(chrono::Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        });
    }
    
    vectors
}

/// Test compression ratios with different block sizes using production SST
#[tokio::test]
async fn test_compression_with_different_block_sizes() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    info!("\n{}", "=".repeat(80));
    info!("📊 TESTING COMPRESSION WITH DIFFERENT BLOCK SIZES");
    info!("{}", "=".repeat(80));
    
    let num_vectors = 2000;
    let dimension = 512;
    let vectors = generate_highly_clustered_vectors(num_vectors, dimension, 10);
    
    let block_sizes = vec![256, 512, 1024, 2048];
    
    for block_size_kb in block_sizes {
        info!("\n{}", "-".repeat(80));
        info!("Testing {}KB blocks with ZSTD compression", block_size_kb);
        
        // Create temporary directory
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path().to_str().unwrap();
        
        // Setup SST with specific block size
        let mut sst_config = SstConfig::default();
        sst_config.block_size_kb = block_size_kb;
        sst_config.storage_config.as_ref().and_then(|s| s.compression.as_ref()) = "zstd".to_string();
        sst_config.compression_level = 3;
        
        // Create filesystem factory
        let fs_config = FilesystemConfig {
            default_fs: Some(format!("file://{}", base_path)),
        };
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
        
        // Create SST storage engine
        let distance_compute = Arc::new(
            proximadb::compute::distance_computation::engine::UnifiedDistanceCompute::new()
        );
        let sst_storage = SstStorage::new(
            sst_config.clone(),
            filesystem,
            distance_compute,
        ).await?;
        
        // Create collection config with compression
        let compression_config = CompressionConfig {
            algorithm: "zstd".to_string(),
            level: Some(3),
            block_size: Some(block_size_kb as i32),
            dictionary_size: None,
            window_size: None,
            strategy: None,
        };
        
        let collection_config = Collection {
            id: "test_collection".to_string(),
            name: "test_collection".to_string(),
            dimension: dimension as u32,
            distance_metric: "cosine".to_string(),
            storage_assignment: Some(proximadb::proto::proximadb_v1::StorageAssignment {
                base_location: format!("file://{}", base_path),
                assignment_id: "test".to_string(),
                tier: "hot".to_string(),
                region: "local".to_string(),
                rack_id: None,
                node_id: None,
                device_id: None,
                created_at: 0,
                last_modified: 0,
            }),
            config: Some(CollectionConfig {
                compression: Some(compression_config),
                ..Default::default()
            }),
            ..Default::default()
        };
        
        // Create flush parameters
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors.clone(),
            collection_config: Some(collection_config),
            force: true,
            synchronous: true,
            ..Default::default()
        };
        
        // Perform flush
        let flush_result = sst_storage.do_flush(&flush_params).await?;
        
        info!("  • Block size: {} KB", block_size_kb);
        info!("  • Compressed size: {:.2} MB", flush_result.bytes_written as f64 / (1024.0 * 1024.0));
        
        // Calculate compression ratio
        let uncompressed_size = num_vectors * dimension * 4; // FP32
        let compression_ratio = if flush_result.bytes_written > 0 {
            uncompressed_size as f64 / flush_result.bytes_written as f64
        } else {
            1.0
        };
        info!("  • Compression ratio: {:.2}x", compression_ratio);
        info!("  • Files created: {}", flush_result.files_created);
        
        // Smaller blocks should generally achieve better compression due to locality
        if block_size_kb == 256 {
            assert!(
                compression_ratio > 5.0,
                "256KB blocks with ZSTD should achieve >5x compression (got {:.2}x)",
                compression_ratio
            );
        }
    }
    
    info!("\n✅ BLOCK SIZE COMPRESSION TEST COMPLETE");
    Ok(())
}