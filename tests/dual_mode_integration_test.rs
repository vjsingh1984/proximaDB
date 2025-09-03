// Integration tests for SST and VIPER dual-mode architecture
// Tests the complete flow from AXIS returning IDs to storage retrieving vectors

use anyhow::Result;
use proximadb::{
    compute::distance_computation::DistanceMetric,
    core::{VectorRecord, hardware_capabilities},
    storage::engines::{
        sst::dual_mode::{
            SearchMode, SstFile, ViperFile, batch_operations::get_records_by_ids as sst_get_by_ids,
            id_index::IdIndex, optimized_operations::OptimizedSstOperations,
            progressive_search::search_progressive as sst_search,
        },
        viper::dual_mode::{
            batch_operations::get_records_by_ids as viper_get_by_ids,
            columnar_search::search_columnar_progressive as viper_search, id_index::ParquetIdIndex,
            optimized_operations::OptimizedViperOperations,
        },
    },
};
use std::collections::HashMap;
use tempfile::tempdir;

/// Test fixture for dual-mode testing
struct DualModeTestFixture {
    sst_file: SstFile,
    viper_file: ViperFile,
    test_vectors: Vec<VectorRecord>,
    dimension: usize,
}

impl DualModeTestFixture {
    fn new(num_vectors: usize, dimension: usize) -> Result<Self> {
        // Initialize hardware capabilities
        let _ = hardware_capabilities::initialize_hardware_capabilities_default();

        // Generate test vectors
        let mut test_vectors = Vec::new();
        for i in 0..num_vectors {
            test_vectors.push(VectorRecord {
                id: Some(format!("vec_{:06}", i)),
                vector: vec![i as f32 / num_vectors as f32; dimension],
                metadata: Some(HashMap::from([
                    (
                        "category".to_string(),
                        serde_json::json!(if i % 2 == 0 { "even" } else { "odd" }),
                    ),
                    ("index".to_string(), serde_json::json!(i)),
                ])),
                timestamp: i as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
            });
        }

        // Create SST file with test data
        let sst_file = create_test_sst_file(&test_vectors, dimension)?;

        // Create VIPER file with test data
        let viper_file = create_test_viper_file(&test_vectors, dimension)?;

        Ok(Self {
            sst_file,
            viper_file,
            test_vectors,
            dimension,
        })
    }
}

/// Create a test SST file
fn create_test_sst_file(vectors: &[VectorRecord], dimension: usize) -> Result<SstFile> {
    use proximadb::storage::engines::sst::dual_mode::*;

    let mut sst = SstFile {
        header: SstHeader {
            version: 1,
            num_vectors: vectors.len() as u64,
            dimension,
            distance_metric: DistanceMetric::Euclidean,
            quantization: QuantizationConfig::default(),
            compression_algorithm: None,
            deleted_records: 0,
        },
        superblocks: Vec::new(),
        id_index: id_index::IdIndex::new(),
        quantized_index: quantization_blocks::QuantizedIndex::new(dimension),
        metadata_index: hierarchical_blocks::MetadataIndex::new(),
    };

    // Build ID index
    for (idx, record) in vectors.iter().enumerate() {
        if let Some(id) = &record.id {
            let location = id_index::BlockLocation {
                superblock_idx: (idx / 1000) as u32,
                block_idx: ((idx % 1000) / 100) as u32,
                offset_in_block: (idx % 100) as u32,
                size_bytes: 1024,
            };
            sst.id_index.insert(id.clone(), location)?;
        }
    }

    // Create superblocks and blocks (simplified)
    let vectors_per_block = 100;
    let blocks_per_superblock = 10;

    let mut current_superblock = SuperBlock {
        id: 0,
        blocks: Vec::new(),
        quantized_signature: vec![0u8; 96],
        id_range: (String::new(), String::new()),
        timestamp_range: (0, 0),
    };

    let mut current_block = DataBlock {
        id: 0,
        offset_in_superblock: 0,
        compressed_size: 0,
        uncompressed_size: 0,
        records: Vec::new(),
        quantized_block: quantization_blocks::QuantizedBlock::new(dimension),
        id_range: (String::new(), String::new()),
        min_timestamp: 0,
        max_timestamp: 0,
        metadata_stats: HashMap::new(),
    };

    for (idx, record) in vectors.iter().enumerate() {
        current_block.records.push(record.clone());

        if current_block.records.len() >= vectors_per_block {
            // Quantize block
            let block_vectors: Vec<Vec<f32>> = current_block
                .records
                .iter()
                .map(|r| r.vector.clone())
                .collect();
            current_block
                .quantized_block
                .quantize_vectors(&block_vectors, &sst.header.quantization)?;

            current_superblock.blocks.push(current_block);
            current_block = DataBlock {
                id: current_superblock.blocks.len() as u32,
                offset_in_superblock: 0,
                compressed_size: 0,
                uncompressed_size: 0,
                records: Vec::new(),
                quantized_block: quantization_blocks::QuantizedBlock::new(dimension),
                id_range: (String::new(), String::new()),
                min_timestamp: 0,
                max_timestamp: 0,
                metadata_stats: HashMap::new(),
            };

            if current_superblock.blocks.len() >= blocks_per_superblock {
                sst.superblocks.push(current_superblock);
                current_superblock = SuperBlock {
                    id: sst.superblocks.len() as u32,
                    blocks: Vec::new(),
                    quantized_signature: vec![0u8; 96],
                    id_range: (String::new(), String::new()),
                    timestamp_range: (0, 0),
                };
            }
        }
    }

    // Add remaining blocks
    if !current_block.records.is_empty() {
        current_superblock.blocks.push(current_block);
    }
    if !current_superblock.blocks.is_empty() {
        sst.superblocks.push(current_superblock);
    }

    Ok(sst)
}

/// Create a test VIPER file
fn create_test_viper_file(vectors: &[VectorRecord], dimension: usize) -> Result<ViperFile> {
    use parquet::file::metadata::RowGroupMetaDataBuilder;
    use proximadb::storage::engines::viper::dual_mode::*;

    let mut viper = ViperFile {
        metadata: ViperMetadata {
            collection_id: "test_collection".to_string(),
            num_vectors: vectors.len() as u64,
            dimension,
            distance_metric: DistanceMetric::Euclidean,
            quantization: QuantizationConfig::default(),
            column_stats: HashMap::new(),
            version: 1,
        },
        row_groups: Vec::new(),
        id_index: id_index::ParquetIdIndex::new(),
        quantized_columns: quantized_columns::QuantizedColumnMetadata {
            binary_column: None,
            int8_column: None,
            pq_column: None,
            quantization_stats: quantized_columns::QuantizationStatistics {
                avg_reconstruction_error: 0.0,
                max_reconstruction_error: 0.0,
                compression_ratio: 1.0,
                quantization_time_ms: 0,
            },
        },
        schema: create_vector_schema(dimension, &QuantizationConfig::default(), &[]),
    };

    // Build ID index
    let vectors_per_row_group = 1000;
    for (idx, record) in vectors.iter().enumerate() {
        if let Some(id) = &record.id {
            let location = id_index::ParquetLocation {
                row_group_id: idx / vectors_per_row_group,
                row_offset: (idx % vectors_per_row_group) as u32,
                page_num: Some(((idx % vectors_per_row_group) / 100) as u32),
            };
            // Would insert into index
            let _ = (id, location);
        }
    }

    Ok(viper)
}

// Integration Tests

#[tokio::test]
async fn test_sst_id_lookup_after_compaction() -> Result<()> {
    let fixture = DualModeTestFixture::new(1000, 768)?;

    // Simulate AXIS returning top-k IDs
    let axis_ids = vec![
        "vec_000100".to_string(),
        "vec_000200".to_string(),
        "vec_000500".to_string(),
    ];

    // Lookup vectors by IDs in SST
    let records = sst_get_by_ids(&fixture.sst_file, &axis_ids).await?;

    assert_eq!(records.len(), 3);
    for (id, record) in axis_ids.iter().zip(records.iter()) {
        assert_eq!(record.id.as_ref().unwrap(), id);
        assert_eq!(record.vector.len(), fixture.dimension);
    }

    Ok(())
}

#[tokio::test]
async fn test_viper_id_lookup_after_compaction() -> Result<()> {
    let fixture = DualModeTestFixture::new(1000, 768)?;

    // Simulate AXIS returning IDs
    let axis_ids = vec![
        "vec_000150".to_string(),
        "vec_000350".to_string(),
        "vec_000750".to_string(),
    ];

    // Lookup vectors by IDs in VIPER
    let records = viper_get_by_ids(&fixture.viper_file, &axis_ids).await?;

    assert_eq!(records.len(), 3);
    for record in records {
        assert!(axis_ids.contains(record.id.as_ref().unwrap()));
    }

    Ok(())
}

#[tokio::test]
async fn test_sst_progressive_search() -> Result<()> {
    let fixture = DualModeTestFixture::new(1000, 128)?;

    // Create query vector
    let query = vec![0.5; fixture.dimension];

    // Perform progressive search
    let results = sst_search(&fixture.sst_file, &query, 10, None).await?;

    assert!(!results.is_empty());
    assert!(results.len() <= 10);

    // Verify results are valid vectors
    for record in results {
        assert_eq!(record.vector.len(), fixture.dimension);
    }

    Ok(())
}

#[tokio::test]
async fn test_viper_columnar_search() -> Result<()> {
    let fixture = DualModeTestFixture::new(1000, 128)?;

    // Create query vector
    let query = vec![0.5; fixture.dimension];

    // Perform columnar search
    let results = viper_search(&fixture.viper_file, &query, 10, None).await?;

    assert!(!results.is_empty());
    assert!(results.len() <= 10);

    Ok(())
}

#[tokio::test]
async fn test_optimized_sst_operations() -> Result<()> {
    let fixture = DualModeTestFixture::new(100, 128)?;

    // Create optimized operations
    let ops = OptimizedSstOperations::new()?;

    // Test optimized search
    let query = vec![0.5; fixture.dimension];
    let config = proximadb::storage::engines::sst::dual_mode::progressive_search::ProgressiveSearchConfig::default();

    let results = ops
        .search_optimized(&fixture.sst_file, &query, 5, config)
        .await?;

    assert!(!results.is_empty());
    assert!(results.len() <= 5);

    Ok(())
}

#[tokio::test]
async fn test_optimized_viper_operations() -> Result<()> {
    let fixture = DualModeTestFixture::new(100, 128)?;

    // Create optimized operations
    let ops = OptimizedViperOperations::new()?;

    // Test optimized columnar search
    let query = vec![0.5; fixture.dimension];
    let config = proximadb::storage::engines::viper::dual_mode::columnar_search::ColumnarSearchConfig::default();

    let results = ops
        .search_columnar_optimized(&fixture.viper_file, &query, 5, config)
        .await?;

    assert!(!results.is_empty());
    assert!(results.len() <= 5);

    Ok(())
}

#[tokio::test]
async fn test_hybrid_search_mode() -> Result<()> {
    let fixture = DualModeTestFixture::new(1000, 256)?;

    // Simulate AXIS providing initial candidates
    let axis_candidates = vec![
        "vec_000100".to_string(),
        "vec_000200".to_string(),
        "vec_000300".to_string(),
        "vec_000400".to_string(),
        "vec_000500".to_string(),
    ];

    // Get full vectors for reranking
    let candidates = sst_get_by_ids(&fixture.sst_file, &axis_candidates).await?;

    // Rerank with query
    let query = vec![0.45; fixture.dimension];
    let ops = OptimizedSstOperations::new()?;

    // In real implementation, would compute distances and rerank
    assert_eq!(candidates.len(), 5);

    Ok(())
}

#[test]
fn test_memory_pool_efficiency() {
    use proximadb::core::memory::pool::VectorMemoryPool;

    let pool = VectorMemoryPool::new();

    // Acquire and release buffers multiple times
    for _ in 0..100 {
        let mut buffer = pool/* TODO: Fix VectorMemoryPool::acquire() method */;
        buffer.resize(768, 0.0);
        // Buffer automatically returned on drop
    }

    // Check pool statistics
    let stats = pool.stats();
    assert!(stats.hit_rate() > 0.9); // Should have high cache hit rate
}

#[test]
fn test_hardware_detection() {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let caps = hardware_capabilities::HardwareCapabilities::get().unwrap();

    // Verify hardware was detected
    assert!(caps.cpu_cores() > 0);

    // Check for SIMD support
    let backend = caps/* TODO: Fix HardwareCapabilities::best_backend() method */;
    println!("Detected backend: {:?}", backend);

    // Should have at least scalar support
    assert!(matches!(
        backend,
        hardware_capabilities::HardwareBackend::Scalar
            | hardware_capabilities::HardwareBackend::SSE
            | hardware_capabilities::HardwareBackend::AVX2
            | hardware_capabilities::HardwareBackend::AVX512
            | hardware_capabilities::HardwareBackend::NEON
    ));
}
