use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value};
use std::collections::HashMap;

// Generate different data patterns for testing
fn create_constant_vectors(num_vectors: usize, dimension: usize, value: f32) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();
    for row in 0..num_vectors {
        vectors.push(VectorRecord {
            id: format!("constant_{:06}", row),
            vector: vec![value; dimension],
            metadata: HashMap::new(),
            timestamp: 1640995200 + (row as i64 * 60),
            expires_at: None,
            source: Some("constant_test".to_string()),
            updated_at: None,
            version: None,
        });
    }
    vectors
}

fn create_sparse_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut vectors = Vec::new();

    for row in 0..num_vectors {
        let mut vector = vec![0.0f32; dimension];
        // Only 5% non-zero values
        let non_zero_count = (dimension as f32 * 0.05) as usize;
        for _ in 0..non_zero_count {
            let idx = rng.gen_range(0..dimension);
            vector[idx] = rng.gen_range(-10.0..10.0);
        }

        vectors.push(VectorRecord {
            id: format!("sparse_{:06}", row),
            vector,
            metadata: HashMap::new(),
            timestamp: 1640995200 + (row as i64 * 60),
            expires_at: None,
            source: Some("sparse_test".to_string()),
            updated_at: None,
            version: None,
        });
    }
    vectors
}

fn create_sequential_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for row in 0..num_vectors {
        let mut vector = Vec::with_capacity(dimension);
        let base = (row * dimension) as f32;

        for dim in 0..dimension {
            // Sequential values with small increments
            let value = base + dim as f32 + (dim % 7) as f32 * 0.01;
            vector.push(value);
        }

        vectors.push(VectorRecord {
            id: format!("sequential_{:06}", row),
            vector,
            metadata: HashMap::new(),
            timestamp: 1640995200 + (row as i64 * 60),
            expires_at: None,
            source: Some("sequential_test".to_string()),
            updated_at: None,
            version: None,
        });
    }
    vectors
}

fn create_normalized_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut vectors = Vec::new();

    for row in 0..num_vectors {
        let mut vector = Vec::with_capacity(dimension);
        let mut sum_sq = 0.0f32;

        // Generate random values in [-1, 1]
        for _ in 0..dimension {
            let val = rng.gen_range(-1.0..1.0);
            vector.push(val);
            sum_sq += val * val;
        }

        // Normalize to unit length
        let norm = sum_sq.sqrt();
        if norm > 0.0 {
            for val in &mut vector {
                *val /= norm;
            }
        }

        vectors.push(VectorRecord {
            id: format!("normalized_{:06}", row),
            vector,
            metadata: HashMap::new(),
            timestamp: 1640995200 + (row as i64 * 60),
            expires_at: None,
            source: Some("normalized_test".to_string()),
            updated_at: None,
            version: None,
        });
    }
    vectors
}

fn test_pattern(pattern_name: &str, vectors: Vec<VectorRecord>) {
    println!("\n🎯 Testing Pattern: {}", pattern_name);
    println!("{}", "=".repeat(50));

    let strategies = vec![
        ("FullVector", VectorEncodingLayout::FullVector),
        ("TransposeField", VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
        ("GroupedField", VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector),
    ];

    for (strategy_name, strategy) in strategies {
        println!("\n  📊 Strategy: {}", strategy_name);

        let config = BlockCompressionConfig {
            vector_layout: strategy,
            algorithm: CompressionAlgorithm::Lz4,
            compression_level: 1,
            enable_vector_compression: true,
            enable_metadata_compression: false,
            compression_threshold_bytes: 0,
            dictionary_compression: false,
            metadata_algorithm: None,
        };

        // Test serialization and compression
        let block = ProximaDataBlock::new(vectors.clone(), config.clone());
        match block.serialize_with_config(&config) {
            Ok(serialized) => {
                let original_size = vectors.len() *
                    (if !vectors.is_empty() { vectors[0].vector.len() } else { 0 }) *
                    std::mem::size_of::<f32>();
                let compression_ratio = original_size as f64 / serialized.len() as f64;

                println!("     → {} bytes ({:.1}x compression)", serialized.len(), compression_ratio);

                // Test deserialization
                match ProximaDataBlock::deserialize(&serialized) {
                    Ok(_) => {
                        println!("     ✅ Round-trip successful");
                    }
                    Err(e) => {
                        println!("     ❌ Deserialization failed: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("     ❌ Serialization failed: {}", e);
            }
        }
    }
}

fn main() {
    println!("🚀 Proxima Pattern Detection Test");
    println!("{}", "=".repeat(60));

    const NUM_VECTORS: usize = 100;
    const DIMENSION: usize = 256;

    println!("\n📊 Configuration:");
    println!("   Vectors: {}", NUM_VECTORS);
    println!("   Dimensions: {}", DIMENSION);
    println!("   Total floats: {}", NUM_VECTORS * DIMENSION);

    // Test different patterns
    let constant_vectors = create_constant_vectors(NUM_VECTORS, DIMENSION, 42.0);
    test_pattern("Constant Data (all 42.0)", constant_vectors);

    let sparse_vectors = create_sparse_vectors(NUM_VECTORS, DIMENSION);
    test_pattern("Sparse Data (95% zeros)", sparse_vectors);

    let sequential_vectors = create_sequential_vectors(NUM_VECTORS, DIMENSION);
    test_pattern("Sequential Data (incremental)", sequential_vectors);

    let normalized_vectors = create_normalized_vectors(NUM_VECTORS, DIMENSION);
    test_pattern("Normalized Data (unit vectors)", normalized_vectors);

    println!("\n🎉 Pattern detection test completed!");
    println!("   Run with RUST_LOG=debug to see detailed pattern analysis");
}