use proximadb::storage::engines::core::formats::fastlanes_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, FastLanesDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn create_random_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    (0..num_vectors).map(|row| VectorRecord {
        id: format!("random_{:06}", row),
        vector: (0..dimension).map(|_| rng.r#gen::<f32>()).collect(),
        metadata: HashMap::new(),
        quantized_vector: vec![],
        expires_at: None,
        source: Some("random_test".to_string()),
        timestamp: 1640995200 + (row as i64 * 60),
        updated_at: None,
        version: None,
    }).collect()
}

fn test_random_data_robustness() {
    println!("🚀 Debug: Random Data FastLanes Test");
    println!("===================================");

    // Small test case to isolate the issue
    let vectors = create_random_vectors(4, 4);

    println!("📊 Test Data (4x4 random):");
    for (i, record) in vectors.iter().enumerate() {
        println!("   Row {}: {:?}", i, record.vector);
    }

    let config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::None, // No compression to isolate FastLanes issue
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    println!("\n🔬 Testing FullVector + None (No compression)");
    println!("============================================");

    // Step 1: Create block (this should show our debug output)
    println!("Step 1: Creating FastLanes block...");
    let block = FastLanesDataBlock::new(vectors.clone(), config.clone());

    // Step 2: Serialize (this should show encoding debug)
    println!("Step 2: Serializing block...");
    match block.serialize_with_config(&config) {
        Ok(encoded) => {
            println!("✅ Serialization successful: {} bytes", encoded.len());

            // Step 3: Deserialize (this should show decoding debug)
            println!("Step 3: Deserializing block...");
            match FastLanesDataBlock::deserialize(&encoded) {
                Ok(decoded_block) => {
                    println!("✅ Deserialization successful: {} vectors", decoded_block.records.len());

                    // Step 4: Verify data integrity
                    println!("Step 4: Verifying data integrity...");
                    let mut mismatch_count = 0;

                    for (i, (orig, dec)) in vectors.iter().zip(decoded_block.records.iter()).enumerate() {
                        if orig.id != dec.id {
                            println!("❌ ID mismatch at row {}: '{}' vs '{}'", i, orig.id, dec.id);
                            mismatch_count += 1;
                            continue;
                        }

                        if orig.vector.len() != dec.vector.len() {
                            println!("❌ Vector length mismatch at row {}: {} vs {}", i, orig.vector.len(), dec.vector.len());
                            mismatch_count += 1;
                            continue;
                        }

                        for (j, (a, b)) in orig.vector.iter().zip(dec.vector.iter()).enumerate() {
                            let diff = (a - b).abs();
                            if diff > 1e-5 {
                                println!("❌ Value mismatch at row {}[{}]: {:.8} vs {:.8} (diff: {:.8})", i, j, a, b, diff);
                                mismatch_count += 1;
                                if mismatch_count >= 5 { // Limit output
                                    println!("... (too many errors, truncating output)");
                                    break;
                                }
                            }
                        }
                        if mismatch_count >= 5 { break; }
                    }

                    if mismatch_count == 0 {
                        println!("🎉 All data integrity checks passed!");
                    } else {
                        println!("❌ Found {} data mismatches!", mismatch_count);
                    }
                }
                Err(e) => {
                    println!("❌ Deserialization failed: {}", e);
                }
            }
        }
        Err(e) => {
            println!("❌ Serialization failed: {}", e);
        }
    }
}

fn main() {
    test_random_data_robustness();
}