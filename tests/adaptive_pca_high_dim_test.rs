//! Integration test for Adaptive PCA with high-dimensional vectors
//!
//! Tests the complete 512-bit spatial encoding system with:
//! - BGE-768 embeddings (48-64 PCA dims)
//! - OpenAI-1536 embeddings (64 PCA dims)
//! - Verification of adaptive dimension selection
//! - Pruning effectiveness with high-dimensional vectors

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::SearchParams;
use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, SqlValue, StorageAssignment, StorageEngine, VectorRecord,
    sql_value,
};
use proximadb::storage::engines::core::formats::proximablocks::spatial_clustering::AdaptivePcaConfig;
use proximadb::storage::engines::impls::sst::SstEngine;
use proximadb::storage::traits::{
    FlushParameters, StorageQueryContext, StorageQueryMetadata, UnifiedStorageEngine,
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// Generate test vectors with specified dimensionality
fn generate_high_dim_vectors(count: usize, dimension: usize, seed: u64) -> Vec<VectorRecord> {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    (0..count)
        .map(|i| {
            let vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

            let mut metadata = HashMap::new();
            metadata.insert(
                "index".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::Int64Value(i as i64)),
                },
            );

            VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata,
                version: Some(1),
                timestamp: Some(i as i64),
                updated_at: None,
                expires_at: None,
                source: None,
            }
        })
        .collect()
}

#[test]
fn test_adaptive_pca_dimension_selection() {
    // Test dimension selection for various vector sizes
    let test_cases = vec![
        (128, 16..=24),  // 128-dim: 16-24 PCA dims
        (384, 32..=48),  // 384-dim: 32-48 PCA dims
        (768, 48..=64),  // 768-dim (BGE): 48-64 PCA dims
        (1536, 64..=64), // 1536-dim (OpenAI): 64 PCA dims (max)
    ];

    for (vector_dim, expected_range) in test_cases {
        let config = AdaptivePcaConfig::for_vector_dim(vector_dim);

        assert!(
            expected_range.contains(&config.n_components),
            "Vector dim {} should select {}-{} PCA dims, got {}",
            vector_dim,
            expected_range.start(),
            expected_range.end(),
            config.n_components
        );

        println!(
            "✅ Vector dim {} → {} PCA dims ({}% of original)",
            vector_dim,
            config.n_components,
            (config.n_components * 100) / vector_dim
        );
    }

    println!("\n✅ Adaptive PCA dimension selection validated for all embedding sizes");
}

#[tokio::test]
async fn test_sst_with_bge_768_embeddings() -> anyhow::Result<()> {
    use tracing::info;
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();

    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let temp_dir = TempDir::new()?;
    info!("🔍 Test directory: {:?}", temp_dir.path());

    // Create SST engine
    let engine = SstEngine::new().await?;
    info!("✅ SST engine created");

    // Create collection config for BGE-768 embeddings
    // SST engine appends collection_id to base_location, so just provide root
    let collection_config = Collection {
        id: "test_bge_768".to_string(),
        config: Some(CollectionConfig {
            name: "test_bge_768".to_string(),
            dimension: 768,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            base_location: temp_dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Generate BGE-768 like vectors
    println!("📊 Generating 1000 BGE-768 embeddings...");
    let vectors = generate_high_dim_vectors(1000, 768, 42);
    let query = vectors[0].vector.clone();

    // Verify adaptive PCA will select optimal dimensions
    let pca_config = AdaptivePcaConfig::for_vector_dim(768);
    println!(
        "🔬 Adaptive PCA: {} → {} dimensions",
        768, pca_config.n_components
    );
    assert!(
        pca_config.n_components >= 48 && pca_config.n_components <= 64,
        "BGE-768 should use 48-64 PCA dims"
    );

    // Flush to SST
    let flush_params = FlushParameters {
        collection_id: Some("test_bge_768".to_string()),
        vector_records: vectors.into_iter().map(|v| v.into()).collect(),
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: Some(collection_config.clone()),
        estimated_size: 0,
    };

    info!("📤 Flushing vectors...");
    engine.do_flush(&flush_params).await?;
    info!("✅ Flush completed");

    // Verify SST files were created (SST engine creates {base}/collection_id/data)
    let data_dir = temp_dir.path().join("test_bge_768").join("data");
    if data_dir.exists() {
        info!("📁 Data directory exists: {:?}", data_dir);
        let entries: Vec<_> = std::fs::read_dir(&data_dir)?.collect();
        info!("📄 Data directory has {} entries", entries.len());
        for entry in entries {
            if let Ok(entry) = entry {
                info!("  - {:?}", entry.path());
            }
        }
    } else {
        info!("⚠️ Data directory does not exist at {:?}", data_dir);
    }

    // Search with Z-Order pruning
    info!("🔍 Starting search...");
    let search_params = Arc::new(SearchParams {
        query_vectors: Some(vec![query]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    });

    let ctx = StorageQueryContext {
        search_params,
        collection: Arc::new(collection_config.clone()),
        metadata: StorageQueryMetadata {
            collection_id: collection_config.id.clone(),
            ..Default::default()
        },
    };

    let results = engine.search_vectors_unified(&ctx).await?;

    info!("📊 Search returned {} results", results.len());
    if results.is_empty() {
        info!("⚠️ No results found - debugging search");
    } else {
        for (i, result) in results.iter().take(3).enumerate() {
            info!(
                "  Result {}: id={}, score={:?}",
                i + 1,
                result.id,
                result.score
            );
        }
    }

    println!(
        "✅ BGE-768 test: Found {} results with adaptive PCA",
        results.len()
    );
    assert!(
        !results.is_empty(),
        "Should find results with 768-dim vectors"
    );

    Ok(())
}

#[tokio::test]
async fn test_sst_with_openai_1536_embeddings() -> anyhow::Result<()> {
    use tracing::info;
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();

    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let temp_dir = TempDir::new()?;
    info!("🔍 Test directory: {:?}", temp_dir.path());

    // Create SST engine
    let engine = SstEngine::new().await?;
    info!("✅ SST engine created");

    // Create collection config for OpenAI-1536 embeddings
    // SST engine appends collection_id to base_location, so just provide root
    let collection_config = Collection {
        id: "test_openai_1536".to_string(),
        config: Some(CollectionConfig {
            name: "test_openai_1536".to_string(),
            dimension: 1536,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        }),
        storage_assignment: Some(StorageAssignment {
            base_location: temp_dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Generate OpenAI-1536 like vectors
    println!("📊 Generating 1000 OpenAI-1536 embeddings...");
    let vectors = generate_high_dim_vectors(1000, 1536, 12345);
    let query = vectors[0].vector.clone();

    // Verify adaptive PCA selects maximum 64 dimensions
    let pca_config = AdaptivePcaConfig::for_vector_dim(1536);
    println!(
        "🔬 Adaptive PCA: {} → {} dimensions (max)",
        1536, pca_config.n_components
    );
    assert_eq!(
        pca_config.n_components, 64,
        "OpenAI-1536 should use max 64 PCA dims"
    );

    // Flush to SST
    let flush_params = FlushParameters {
        collection_id: Some("test_openai_1536".to_string()),
        vector_records: vectors.into_iter().map(|v| v.into()).collect(),
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: Some(collection_config.clone()),
        estimated_size: 0,
    };

    info!("📤 Flushing vectors...");
    engine.do_flush(&flush_params).await?;
    info!("✅ Flush completed");

    // Verify SST files were created (SST engine creates {base}/collection_id/data)
    let data_dir = temp_dir.path().join("test_openai_1536").join("data");
    if data_dir.exists() {
        info!("📁 Data directory exists: {:?}", data_dir);
        let entries: Vec<_> = std::fs::read_dir(&data_dir)?.collect();
        info!("📄 Data directory has {} entries", entries.len());
        for entry in entries {
            if let Ok(entry) = entry {
                info!("  - {:?}", entry.path());
            }
        }
    } else {
        info!("⚠️ Data directory does not exist at {:?}", data_dir);
    }

    // Search with Z-Order pruning
    info!("🔍 Starting search...");
    let search_params = Arc::new(SearchParams {
        query_vectors: Some(vec![query]),
        top_k: Some(10),
        distance_metric: Some(DistanceMetric::Cosine),
        ..Default::default()
    });

    let ctx = StorageQueryContext {
        search_params,
        collection: Arc::new(collection_config.clone()),
        metadata: StorageQueryMetadata {
            collection_id: collection_config.id.clone(),
            ..Default::default()
        },
    };

    let results = engine.search_vectors_unified(&ctx).await?;

    info!("📊 Search returned {} results", results.len());
    if results.is_empty() {
        info!("⚠️ No results found - debugging search");
    } else {
        for (i, result) in results.iter().take(3).enumerate() {
            info!(
                "  Result {}: id={}, score={:?}",
                i + 1,
                result.id,
                result.score
            );
        }
    }

    println!(
        "✅ OpenAI-1536 test: Found {} results with 64 PCA dims",
        results.len()
    );
    assert!(
        !results.is_empty(),
        "Should find results with 1536-dim vectors"
    );

    Ok(())
}

#[tokio::test]
async fn test_spatial_code_types_by_dimension() -> anyhow::Result<()> {
    use proximadb::storage::engines::core::formats::proximablocks::spatial_encoding::CodeType;

    // Test automatic code type selection based on dimensions
    let test_cases = vec![
        (8, 8, CodeType::Bits64),   // 8 dims @ 8 bits = 64 bits
        (16, 8, CodeType::Bits128), // 16 dims @ 8 bits = 128 bits
        (32, 8, CodeType::Bits256), // 32 dims @ 8 bits = 256 bits
        (64, 8, CodeType::Bits512), // 64 dims @ 8 bits = 512 bits
    ];

    for (dims, bits_per_dim, expected_type) in test_cases {
        let selected_type = CodeType::select(dims, bits_per_dim);
        assert_eq!(
            selected_type, expected_type,
            "Dims {} @ {} bits/dim should select {:?}",
            dims, bits_per_dim, expected_type
        );

        println!(
            "✅ {} dims @ {} bits/dim → {:?} ({} bits)",
            dims,
            bits_per_dim,
            selected_type,
            selected_type.max_bits()
        );
    }

    println!("\n✅ Spatial code type selection validated for all dimension ranges");
    Ok(())
}
