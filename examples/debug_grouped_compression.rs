use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    // Test with a small constant pattern that should compress well
    let dimension = 64;
    let count = 10;

    // Create constant vectors (all 42.0)
    let mut records = Vec::new();
    for i in 0..count {
        let record = VectorRecord {
            id: format!("vec_{}", i),
            vector: vec![42.0; dimension],
            metadata: HashMap::new(),
            quantized_vector: vec![],
            expires_at: None,
            source: None,
            timestamp: 0,
            updated_at: None,
            version: None,
        };
        records.push(record);
    }

    println!("Testing GroupedField compression with constant data");
    println!("Vectors: {}, Dimension: {}", count, dimension);

    // Test without compression first
    let config_none = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
        algorithm: CompressionAlgorithm::None,
        compression_level: 1,
        enable_vector_compression: false,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    println!("\nTest 1: GroupedField + None");
    let block = ProximaDataBlock::new(records.clone(), config_none.clone());
    let serialized = block.serialize_with_config(&config_none)?;
    println!("  Serialized size: {} bytes", serialized.len());

    let deserialized = ProximaDataBlock::deserialize(&serialized)?;
    if deserialized.records.len() == count {
        println!("  ✅ Deserialization successful");
    } else {
        println!("  ❌ Deserialization failed: wrong count");
    }

    // Test with LZ4 compression
    let config_lz4 = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    println!("\nTest 2: GroupedField + LZ4");
    let block = ProximaDataBlock::new(records.clone(), config_lz4.clone());
    let serialized = block.serialize_with_config(&config_lz4)?;
    println!("  Serialized size: {} bytes", serialized.len());

    match ProximaDataBlock::deserialize(&serialized) {
        Ok(deserialized) => {
            if deserialized.records.len() == count {
                println!("  ✅ Deserialization successful");

                // Verify data
                let all_correct = deserialized.records.iter().all(|r| {
                    r.vector.iter().all(|&v| (v - 42.0).abs() < 0.0001)
                });

                if all_correct {
                    println!("  ✅ Data verification successful");
                } else {
                    println!("  ❌ Data verification failed");
                }
            } else {
                println!("  ❌ Deserialization failed: wrong count ({} != {})",
                    deserialized.records.len(), count);
            }
        },
        Err(e) => {
            println!("  ❌ Deserialization failed: {}", e);
            println!("  Error details: {:?}", e);
        }
    }

    Ok(())
}