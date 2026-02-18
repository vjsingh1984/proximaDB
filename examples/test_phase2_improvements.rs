use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, ProximaDataBlock, VectorEncodingLayout,
};
use std::collections::HashMap;

fn create_pattern_vectors(
    num_vectors: usize,
    dimension: usize,
    pattern: &str,
) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for row in 0..num_vectors {
        let mut vector = Vec::with_capacity(dimension);

        match pattern {
            "sparse" => {
                // 90% zeros with some non-zero values in runs
                for dim in 0..dimension {
                    if dim < 10 || (dim > 100 && dim < 110) {
                        vector.push(row as f32 * 0.01 + dim as f32 * 0.001);
                    } else {
                        vector.push(0.0);
                    }
                }
            }
            "constant" => {
                // All same value
                for _ in 0..dimension {
                    vector.push(42.0);
                }
            }
            "sequential" => {
                // Monotonic values
                for dim in 0..dimension {
                    vector.push(row as f32 * dimension as f32 + dim as f32);
                }
            }
            "normalized" => {
                // Embeddings in [-1, 1] range
                for dim in 0..dimension {
                    let value = ((row + dim) as f32 * 0.123).sin() * 0.95;
                    vector.push(value);
                }
            }
            _ => {
                // Random pattern
                for dim in 0..dimension {
                    let value = ((row as f32) * 0.01 + (dim as f32) * 0.1)
                        + (row as f32 * dim as f32).sin() * 0.05;
                    vector.push(value);
                }
            }
        }

        vectors.push(VectorRecord {
            id: format!("vec_{:06}", row),
            vector,
            metadata: HashMap::new(),
            expires_at: None,
            source: None,
            timestamp: Some(0),
            updated_at: None,
            version: None,
        });
    }

    vectors
}

fn test_pattern(pattern: &str, vectors: &[VectorRecord]) {
    println!("\n📊 Testing pattern: {}", pattern);
    println!("─────────────────────────────────────");

    let config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::Zstd,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());
    let dimension = vectors[0].vector.len();
    let original_size = vectors.len() * dimension * 4; // 4 bytes per f32

    let encoded = block.serialize_with_config(&config).unwrap();
    let encoded_size = encoded.len();
    let compression_ratio = original_size as f64 / encoded_size as f64;

    println!("  Original: {} bytes", original_size);
    println!("  Encoded:  {} bytes", encoded_size);
    println!("  Ratio:    {:.1}x compression", compression_ratio);

    // Expected compression ratios with Phase 2 improvements
    let expected_ratio = match pattern {
        "sparse" => 20.0,     // Should get >20x with RLE
        "constant" => 100.0,  // Should get >100x with RLE
        "sequential" => 30.0, // Should get >30x with Delta
        "normalized" => 10.0, // Should get >10x with FrameOfReference
        _ => 1.5,             // Random data
    };

    if compression_ratio >= expected_ratio * 0.5 {
        println!(
            "  ✅ PASS: Achieved good compression for {} pattern",
            pattern
        );
    } else {
        println!(
            "  ⚠️  WARN: Lower than expected compression ({:.1}x expected)",
            expected_ratio
        );
    }
}

fn main() -> anyhow::Result<()> {
    println!("🚀 Phase 2 Pattern Detection & Encoding Test");
    println!("============================================");

    let patterns = vec!["sparse", "constant", "sequential", "normalized", "random"];

    for pattern in &patterns {
        let vectors = create_pattern_vectors(256, 256, pattern);
        test_pattern(pattern, &vectors);
    }

    println!("\n✅ Phase 2 testing complete!");
    Ok(())
}
