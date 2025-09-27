use proximadb::storage::engines::core::formats::proxima_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value};
use std::collections::HashMap;

fn create_test_vectors() -> Vec<VectorRecord> {
    let mut vectors = Vec::new();
    for row in 0..3 {
        let mut vector = Vec::with_capacity(4);
        for dim in 0..4 {
            let value = (row as f32 + dim as f32) * 0.1;
            vector.push(value);
        }

        vectors.push(VectorRecord {
            id: format!("test_vec_{}", row),
            vector,
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("row_index".to_string(), SqlValue {
                    value: Some(sql_value::Value::Int64Value(row as i64))
                });
                meta
            },
            quantized_vector: vec![],
            expires_at: None,
            source: None,
            timestamp: 1640995200 + (row as i64 * 3600),
            updated_at: None,
            version: None,
        });
    }

    vectors
}

fn test_round_trip(
    vectors: &[VectorRecord],
    layout: VectorEncodingLayout,
    algorithm: CompressionAlgorithm,
    test_name: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    println!("\n🧪 Testing Round-Trip: {}", test_name);

    let config = BlockCompressionConfig {
        vector_layout: layout,
        algorithm,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
    };

    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());

    // Test encoding
    println!("🔄 Encoding...");
    let encoded = block.serialize_with_config(&config)?;
    println!("✅ Encoded successfully: {} bytes", encoded.len());

    // Test decoding
    println!("🔄 Decoding...");
    let decoded_block = ProximaDataBlock::deserialize(&encoded)?;
    println!("✅ Decoded successfully: {} vectors", decoded_block.records.len());

    // Verify data integrity
    if decoded_block.records.len() != vectors.len() {
        println!("❌ Vector count mismatch: {} vs {}", vectors.len(), decoded_block.records.len());
        return Ok(false);
    }

    println!("✅ Round-trip successful: All data matches!");
    Ok(true)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Proxima Deserialization Fix Test");
    println!("=====================================");

    let vectors = create_test_vectors();
    println!("📊 Test Data: {} vectors × {} dimensions", vectors.len(), vectors[0].vector.len());

    // Test FullVector + None (uncompressed)
    let success = test_round_trip(&vectors, VectorEncodingLayout::FullVector, CompressionAlgorithm::None, "FullVector + None")?;
    if success {
        println!("🎉 SUCCESS: Deserialization fix is working!");
    } else {
        println!("❌ FAILED: Fix still needs work");
    }

    Ok(())
}