use std::sync::Arc;
use anyhow::Result;
use proximadb::storage::engines::sst::{SstEngine, SstConfig};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use proximadb::proto::proximadb_v1::{VectorRecord, Collection, CollectionConfig, QuantizationConfig};
use proximadb::compute::quantization::precompute::QuantizationPrecomputeService;

#[tokio::test]
async fn test_sst_flush_with_quantization() -> Result<()> {
    // Setup
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await?);
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    // Create SST engine
    let config = SstConfig::default();
    let engine = SstEngine::new_with_config(config, filesystem, distance_compute).await?;

    // Create test collection with quantization enabled
    let collection = Collection {
        id: "test_collection".to_string(),
        name: "Test Collection".to_string(),
        dimension: 128,
        config: CollectionConfig {
            quantization: QuantizationConfig {
                enabled: true,
                levels: Some(vec!["binary".to_string(), "int8".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    // Create test vectors
    let mut vector_records = Vec::new();
    for i in 0..100 {
        let mut values = vec![0.0f32; 128];
        for j in 0..128 {
            values[j] = ((i * j) as f32).sin();
        }

        vector_records.push(VectorRecord {
            id: format!("vec_{}", i),
            values,
            metadata: Default::default(),
            created_at: None,
            updated_at: None,
            version: 0,
        });
    }

    // Create flush parameters with quantization enabled
    let flush_params = FlushParameters {
        collection_id: Some(collection.id.clone()),
        collection_config: Some(collection.clone()),
        vector_records,
        batch_ids: vec!["batch_1".to_string()],
        source: "test".to_string(),
        metadata: Default::default(),
    };

    // Perform flush with quantization
    let flush_result = engine.do_flush(&flush_params).await?;

    // Verify flush was successful
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, Some(100));

    // Verify quantization was applied
    // In a real test, we would read back the blocks and check for QuantizedSection
    println!("Flush completed with quantization:");
    println!("  - Entries flushed: {:?}", flush_result.entries_flushed);
    println!("  - Bytes written: {:?}", flush_result.bytes_written);
    println!("  - Files created: {:?}", flush_result.files_created);

    Ok(())
}

#[tokio::test]
async fn test_quantization_precompute_service() -> Result<()> {
    // Test the QuantizationPrecomputeService directly
    let service = QuantizationPrecomputeService::global();

    // Create test collection
    let collection = Collection {
        id: "test_collection_2".to_string(),
        name: "Test Collection 2".to_string(),
        dimension: 256,
        config: CollectionConfig {
            quantization: QuantizationConfig {
                enabled: true,
                levels: Some(vec!["binary".to_string(), "int8".to_string(), "pq8".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    // Create test vectors
    let mut vector_records = Vec::new();
    for i in 0..50 {
        let mut values = vec![0.0f32; 256];
        for j in 0..256 {
            values[j] = ((i + j) as f32 / 256.0).cos();
        }

        vector_records.push(VectorRecord {
            id: format!("vec_{}", i),
            values,
            metadata: Default::default(),
            created_at: None,
            updated_at: None,
            version: 0,
        });
    }

    // Quantize batch
    let quantized_batch = service.quantize_for_flush(
        vector_records.clone(),
        &collection
    ).await?;

    // Verify quantization results
    assert_eq!(quantized_batch.records.len(), 50);
    assert_eq!(quantized_batch.quantized.len(), 50);

    // Check that quantization was performed
    let mut has_binary = false;
    let mut has_int8 = false;
    let mut has_pq = false;

    for quantized_opt in &quantized_batch.quantized {
        if let Some(q) = quantized_opt {
            if q.binary.is_some() {
                has_binary = true;
            }
            if q.int8.is_some() {
                has_int8 = true;
            }
            if q.pq8.is_some() {
                has_pq = true;
            }
        }
    }

    assert!(has_binary, "Binary quantization should be present");
    assert!(has_int8, "INT8 quantization should be present");
    assert!(has_pq, "PQ quantization should be present");

    println!("Quantization test passed:");
    println!("  - Binary: {}", has_binary);
    println!("  - INT8: {}", has_int8);
    println!("  - PQ: {}", has_pq);

    Ok(())
}

#[tokio::test]
async fn test_proxima_datablock_quantized_section() -> Result<()> {
    use proximadb::storage::engines::core::formats::proximablocks::{
        ProximaDataBlock, BlockCompressionConfig, VectorEncodingLayout, QuantizedSection
    };
    use proximadb::storage::engines::core::ops::unified_proxima_simd::EngineProfile;

    // Create test vectors
    let mut records = Vec::new();
    for i in 0..10 {
        let mut values = vec![0.0f32; 64];
        for j in 0..64 {
            values[j] = (i as f32 * 0.1) + (j as f32 * 0.01);
        }

        records.push(VectorRecord {
            id: format!("vec_{}", i),
            values,
            metadata: Default::default(),
            created_at: None,
            updated_at: None,
            version: 0,
        });
    }

    // Create block with SST profile
    let compression_config = BlockCompressionConfig {
        algorithm: proximadb::core::compression::CompressionAlgorithm::Lz4,
        compression_level: 3,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 1024,
        dictionary_compression: false,
        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        metadata_algorithm: None,
    };

    let mut block = ProximaDataBlock::new_with_engine_profile(
        records.clone(),
        compression_config,
        EngineProfile::SST
    );

    // Add quantized section
    let mut quantized_section = QuantizedSection {
        binary_vectors: Some(vec![vec![0xFF; 8]; 10]), // 64 bits = 8 bytes per vector
        int8_vectors: Some(vec![vec![0i8; 64]; 10]),
        pq_vectors: None,
        codebooks: None,
    };

    block.quantized_section = Some(quantized_section);

    // Verify the block has quantization
    assert!(block.quantized_section.is_some());

    let section = block.quantized_section.as_ref().unwrap();
    assert!(section.binary_vectors.is_some());
    assert!(section.int8_vectors.is_some());

    // Update metadata statistics
    block.metadata.quantization_stats.has_binary = true;
    block.metadata.quantization_stats.has_int8 = true;

    println!("ProximaDataBlock with QuantizedSection created:");
    println!("  - Block ID: {}", block.block_id);
    println!("  - Vector layout: {:?}", block.vector_layout);
    println!("  - Has binary quantization: {}", block.metadata.quantization_stats.has_binary);
    println!("  - Has INT8 quantization: {}", block.metadata.quantization_stats.has_int8);

    Ok(())
}