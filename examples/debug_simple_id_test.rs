use proximadb::storage::engines::core::formats::fastlanes_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, FastLanesDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn create_simple_test_vectors() -> Vec<VectorRecord> {
    vec![
        VectorRecord {
            id: "test_A".to_string(),
            vector: vec![1.0, 2.0],
            metadata: HashMap::new(),
            quantized_vector: vec![],
            expires_at: None,
            source: Some("test".to_string()),
            timestamp: 1000,
            updated_at: None,
            version: None,
        },
        VectorRecord {
            id: "test_B".to_string(),
            vector: vec![3.0, 4.0],
            metadata: HashMap::new(),
            quantized_vector: vec![],
            expires_at: None,
            source: Some("test".to_string()),
            timestamp: 2000,
            updated_at: None,
            version: None,
        },
    ]
}

fn main() {
    println!("🚀 Simple ID Test");
    println!("=================");

    let vectors = create_simple_test_vectors();

    println!("📊 Original Data:");
    for (i, record) in vectors.iter().enumerate() {
        println!("   [{}] ID: '{}', Vector: {:?}", i, record.id, record.vector);
    }

    let config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::None,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    println!("\n🔬 Testing Round-trip...");
    let block = FastLanesDataBlock::new(vectors.clone(), config.clone());

    match block.serialize_with_config(&config) {
        Ok(encoded) => {
            println!("✅ Serialized: {} bytes", encoded.len());

            match FastLanesDataBlock::deserialize(&encoded) {
                Ok(decoded_block) => {
                    println!("✅ Deserialized: {} vectors", decoded_block.records.len());

                    println!("\n📊 Decoded Data:");
                    for (i, record) in decoded_block.records.iter().enumerate() {
                        println!("   [{}] ID: '{}', Vector: {:?}", i, record.id, record.vector);
                    }

                    println!("\n🔍 Comparison:");
                    for (i, (orig, decoded)) in vectors.iter().zip(decoded_block.records.iter()).enumerate() {
                        let id_match = orig.id == decoded.id;
                        let vector_match = orig.vector.iter().zip(decoded.vector.iter())
                            .all(|(a, b)| (a - b).abs() < 1e-6);

                        println!("   [{}] ID: {} '{}' vs '{}' | Vector: {}",
                                i,
                                if id_match { "✅" } else { "❌" },
                                orig.id, decoded.id,
                                if vector_match { "✅" } else { "❌" });
                    }
                }
                Err(e) => println!("❌ Deserialization failed: {}", e),
            }
        }
        Err(e) => println!("❌ Serialization failed: {}", e),
    }
}