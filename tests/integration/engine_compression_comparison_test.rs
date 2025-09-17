//! Comprehensive compression comparison between VIPER and SST engines
//!
//! Tests compression effectiveness for both engines with:
//! - Dense vectors (random ML embeddings)
//! - Sparse vectors (90% zeros)
//! - Multiple algorithms
//! - Performance impact measurements

mod common {
    include!("../common/mod.rs");
}
use common::integration_test_helpers::UnifiedTestEnvironment;

use anyhow::Result;
use proximadb::StorageEngine;
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::CompressionAlgorithm as ProtoCompressionAlgorithm;
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use std::time::Instant;
use tracing::info;

/// Create dense vectors (similar to ML embeddings)
fn create_dense_vectors(count: usize, dim: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector: Vec<f32> = (0..dim)
                .map(|j| ((i * 7 + j * 13) % 100) as f32 / 100.0)
                .collect();
            VectorRecord {
                id: Some(format!("dense_{}", i)),
                vector,
                ..Default::default()
            }
        })
        .collect()
}

/// Create sparse vectors (90% zeros)
fn create_sparse_vectors(count: usize, dim: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let mut vector = vec![0.0; dim];
            // Only 10% non-zero values
            for j in 0..dim / 10 {
                let idx = (i * 7 + j * 13) % dim;
                vector[idx] = ((i + j) % 100) as f32 / 10.0;
            }
            VectorRecord {
                id: Some(format!("sparse_{}", i)),
                vector,
                ..Default::default()
            }
        })
        .collect()
}

/// Test compression for a specific engine, data type, and algorithm
async fn test_engine_compression(
    engine_type: &str,
    data_type: &str,
    vectors: Vec<VectorRecord>,
    algorithm: &str,
    level: i32,
) -> Result<(u64, u64, f64, u64)> {
    // Test UNCOMPRESSED first
    let env_uncompressed = UnifiedTestEnvironment::new().await?;
    let start = Instant::now();

    // Get dimension from vectors
    let dimension = if !vectors.is_empty() {
        vectors[0].vector.len()
    } else {
        128
    };

    match engine_type {
        "SST" => {
            let engine = env_uncompressed.create_sst_engine().await?;

            // Create collection config once with no compression (block_size_kb defaults to 2MB)
            let collection_uncompressed = env_uncompressed.create_test_collection_with_settings(
                StorageEngine::Sst,
                dimension as i32,
                None, // No compression, uses default 2MB blocks
            );

            let flush_params =
                common::integration_test_helpers::operations::build_sst_flush_params_with_collection(
                    &env_uncompressed,
                    vectors.clone(),
                    collection_uncompressed,
                )
                .await?;

            // Insert vectors directly (simulating memtable)
            for batch in vectors.chunks(100) {
                let _ = engine.do_flush(&flush_params).await?;
            }

            let uncompressed_size =
                get_directory_size(env_uncompressed.get_sst_data_directory().as_path()).await;

            // Now test COMPRESSED
            let env_compressed = UnifiedTestEnvironment::new().await?;
            let engine = env_compressed.create_sst_engine().await?;

            let algorithm_enum = match algorithm {
                "lz4" => ProtoCompressionAlgorithm::CompressionLz4 as i32,
                "zstd" => ProtoCompressionAlgorithm::CompressionZstd as i32,
                "snappy" => ProtoCompressionAlgorithm::CompressionSnappy as i32,
                _ => ProtoCompressionAlgorithm::CompressionNone as i32,
            };

            // Create collection config once with compression
            let compression_config = proximadb::proto::proximadb_v1::CompressionConfig {
                algorithm: algorithm_enum,
                level: Some(level),
                block_size_kb: Some(2048), // 2MB blocks for SST
                ..Default::default()
            };
            let collection_compressed = env_compressed.create_test_collection_with_settings(
                StorageEngine::Sst,
                dimension as i32,
                Some(compression_config),
            );

            let flush_params =
                common::integration_test_helpers::operations::build_sst_flush_params_with_collection(
                    &env_compressed,
                    vectors.clone(),
                    collection_compressed,
                )
                .await?;

            for batch in vectors.chunks(100) {
                let _ = engine.do_flush(&flush_params).await?;
            }

            let compressed_size =
                get_directory_size(env_compressed.get_sst_data_directory().as_path()).await;

            let elapsed = start.elapsed().as_millis() as u64;
            let ratio = compressed_size as f64 / uncompressed_size as f64;

            Ok((compressed_size, uncompressed_size, ratio, elapsed))
        }
        "VIPER" => {
            let mut viper_config = env_uncompressed.viper_config.clone();
            viper_config
                .storage_config
                .as_ref()
                .and_then(|s| s.compression.as_ref()) = "none".to_string();

            let engine = proximadb::storage::engines::impls::viper::ViperEngine::from_core_config(
                viper_config,
                env_uncompressed.filesystem.clone(),
            )
            .await?;

            let flush_params =
                common::integration_test_helpers::operations::build_viper_flush_params_with_compression(
                    &env_uncompressed,
                    vectors.clone(),
                    "none",
                    0,
                )
                .await?;

            for batch in vectors.chunks(100) {
                let _ = engine.do_flush(&flush_params).await?;
            }

            let uncompressed_size =
                get_directory_size(env_uncompressed.get_viper_data_directory().as_path()).await;

            // Now test COMPRESSED
            let env_compressed = UnifiedTestEnvironment::new().await?;
            let mut viper_config = env_compressed.viper_config.clone();
            viper_config
                .storage_config
                .as_ref()
                .and_then(|s| s.compression.as_ref()) = algorithm.to_string();
            viper_config.compression_level = level;

            let engine = proximadb::storage::engines::impls::viper::ViperEngine::from_core_config(
                viper_config,
                env_compressed.filesystem.clone(),
            )
            .await?;

            let algorithm_enum = match algorithm {
                "lz4" => ProtoCompressionAlgorithm::CompressionLz4 as i32,
                "zstd" => ProtoCompressionAlgorithm::CompressionZstd as i32,
                "snappy" => ProtoCompressionAlgorithm::CompressionSnappy as i32,
                _ => ProtoCompressionAlgorithm::CompressionNone as i32,
            };

            let flush_params =
                common::integration_test_helpers::operations::build_viper_flush_params_with_compression(
                    &env_compressed,
                    vectors.clone(),
                    algorithm,
                    level,
                )
                .await?;

            for batch in vectors.chunks(100) {
                let _ = engine.do_flush(&flush_params).await?;
            }

            let compressed_size =
                get_directory_size(env_compressed.get_viper_data_directory().as_path()).await;

            let elapsed = start.elapsed().as_millis() as u64;
            let ratio = compressed_size as f64 / uncompressed_size as f64;

            Ok((compressed_size, uncompressed_size, ratio, elapsed))
        }
        _ => panic!("Unknown engine type: {}", engine_type),
    }
}

/// Get total size of all files in a directory
async fn get_directory_size(path: &std::path::Path) -> u64 {
    use tokio::fs;

    let mut total = 0u64;
    if let Ok(mut entries) = fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    total += metadata.len();
                }
            }
        }
    }
    total
}

#[tokio::test]
async fn test_engine_compression_comparison() -> Result<()> {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    info!("🔬 COMPREHENSIVE ENGINE COMPRESSION COMPARISON");
    info!("{}", "=".repeat(80));

    let test_configs = vec![
        ("DENSE", create_dense_vectors(1000, 1536)), // GPT-like embeddings
        ("SPARSE", create_sparse_vectors(1000, 1536)), // 90% sparse
    ];

    let algorithms = vec![("zstd", 3), ("lz4", 0), ("snappy", 0)];

    let mut results = Vec::new();

    for (data_type, vectors) in &test_configs {
        for engine in &["SST", "VIPER"] {
            for (algo, level) in &algorithms {
                info!(
                    "Testing {} engine with {} data using {}",
                    engine, data_type, algo
                );

                match test_engine_compression(engine, data_type, vectors.clone(), algo, *level)
                    .await
                {
                    Ok((compressed, uncompressed, ratio, time_ms)) => {
                        results.push((
                            engine.to_string(),
                            data_type.to_string(),
                            algo.to_string(),
                            compressed,
                            uncompressed,
                            ratio,
                            time_ms,
                        ));
                    }
                    Err(e) => {
                        info!("  ⚠️ Failed: {}", e);
                    }
                }
            }
        }
    }

    // Print comprehensive comparison table
    info!("\n📊 ENGINE COMPRESSION COMPARISON RESULTS");
    info!("┌────────┬────────┬────────┬──────────┬──────────┬────────┬──────────┐");
    info!("│ Engine │ Data   │ Algo   │ Compress │ Original │ Ratio  │ Time(ms) │");
    info!("├────────┼────────┼────────┼──────────┼──────────┼────────┼──────────┤");

    for (engine, data, algo, comp, orig, ratio, time) in &results {
        info!(
            "│ {:6} │ {:6} │ {:6} │ {:8} │ {:8} │ {:.3}  │ {:8} │",
            engine, data, algo, comp, orig, ratio, time
        );
    }
    info!("└────────┴────────┴────────┴──────────┴──────────┴────────┴──────────┘");

    // Calculate and display insights
    info!("\n🎯 KEY INSIGHTS:");

    // SST vs VIPER comparison for dense data
    let sst_dense: Vec<_> = results
        .iter()
        .filter(|r| r.0 == "SST" && r.1 == "DENSE")
        .collect();
    let viper_dense: Vec<_> = results
        .iter()
        .filter(|r| r.0 == "VIPER" && r.1 == "DENSE")
        .collect();

    if !sst_dense.is_empty() && !viper_dense.is_empty() {
        let sst_best = sst_dense
            .iter()
            .min_by(|a, b| a.5.partial_cmp(&b.5).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        let viper_best = viper_dense
            .iter()
            .min_by(|a, b| a.5.partial_cmp(&b.5).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();

        info!("Dense Data:");
        info!(
            "  SST best: {} with ratio {:.3} ({}% reduction)",
            sst_best.2,
            sst_best.5,
            ((1.0 - sst_best.5) * 100.0) as i32
        );
        info!(
            "  VIPER best: {} with ratio {:.3} ({}% reduction)",
            viper_best.2,
            viper_best.5,
            ((1.0 - viper_best.5) * 100.0) as i32
        );
    }

    // SST vs VIPER comparison for sparse data
    let sst_sparse: Vec<_> = results
        .iter()
        .filter(|r| r.0 == "SST" && r.1 == "SPARSE")
        .collect();
    let viper_sparse: Vec<_> = results
        .iter()
        .filter(|r| r.0 == "VIPER" && r.1 == "SPARSE")
        .collect();

    if !sst_sparse.is_empty() && !viper_sparse.is_empty() {
        let sst_best = sst_sparse
            .iter()
            .min_by(|a, b| a.5.partial_cmp(&b.5).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        let viper_best = viper_sparse
            .iter()
            .min_by(|a, b| a.5.partial_cmp(&b.5).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();

        info!("\nSparse Data:");
        info!(
            "  SST best: {} with ratio {:.3} ({}% reduction)",
            sst_best.2,
            sst_best.5,
            ((1.0 - sst_best.5) * 100.0) as i32
        );
        info!(
            "  VIPER best: {} with ratio {:.3} ({}% reduction)",
            viper_best.2,
            viper_best.5,
            ((1.0 - viper_best.5) * 100.0) as i32
        );

        // Determine winner
        if viper_best.5 < sst_best.5 {
            info!(
                "  🏆 VIPER achieves {}% better compression for sparse data",
                ((sst_best.5 / viper_best.5 - 1.0) * 100.0) as i32
            );
        } else {
            info!(
                "  🏆 SST achieves {}% better compression for sparse data",
                ((viper_best.5 / sst_best.5 - 1.0) * 100.0) as i32
            );
        }
    }

    Ok(())
}
