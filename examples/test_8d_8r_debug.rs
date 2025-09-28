use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn create_test_vectors() -> Vec<VectorRecord> {
    let vectors = vec![
        // Row 0: Constant pattern
        vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        // Row 1: Sequential pattern
        vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
        // Row 2: Sparse pattern
        vec![0.0, 0.0, 5.5, 0.0, 0.0, 0.0, 3.2, 0.0],
        // Row 3: Small increments
        vec![10.1, 10.2, 10.3, 10.4, 10.5, 10.6, 10.7, 10.8],
        // Row 4: Larger values
        vec![100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0],
        // Row 5: Mixed pattern
        vec![1.5, 0.0, 3.7, 2.1, 0.0, 4.8, 1.2, 0.0],
        // Row 6: Negative values
        vec![-1.0, -2.0, -3.0, -4.0, -5.0, -6.0, -7.0, -8.0],
        // Row 7: Fractional
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
    ];

    vectors.into_iter().enumerate().map(|(i, vector)| VectorRecord {
        id: format!("test_vec_{}", i),
        vector,
        metadata: HashMap::new(),
        quantized_vector: vec![],
        expires_at: None,
        source: Some(format!("test_batch")),
        timestamp: 1640995200 + (i as i64 * 60),
        updated_at: None,
        version: None,
    }).collect()
}

fn test_strategy(vectors: &[VectorRecord], layout: VectorEncodingLayout, strategy_name: &str) {
    println!("\n🧪 Testing Strategy: {}", strategy_name);
    println!("   Layout: {:?}", layout);

    let config = BlockCompressionConfig {
        vector_layout: layout,
        algorithm: CompressionAlgorithm::None,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());
    match block.serialize_with_config(&config) {
        Ok(encoded) => {
            let original_size = 8 * 8 * 4; // 8 dimensions × 8 rows × 4 bytes per f32
            let compression_ratio = original_size as f64 / encoded.len() as f64;

            println!("   ✅ Success: {} bytes ({:.2}x compression)", encoded.len(), compression_ratio);

            // Test round-trip
            match ProximaDataBlock::deserialize(&encoded) {
                Ok(decoded_block) => {
                    if decoded_block.records.len() == vectors.len() {
                        println!("   ✅ Round-trip: {} vectors decoded successfully", decoded_block.records.len());
                    } else {
                        println!("   ❌ Round-trip: Got {} vectors, expected {}", decoded_block.records.len(), vectors.len());
                    }
                }
                Err(e) => println!("   ❌ Deserialization failed: {}", e),
            }
        }
        Err(e) => println!("   ❌ Serialization failed: {}", e),
    }
}

fn main() {
    println!("🚀 Proxima 8D×8R Debug Test");
    println!("=============================");

    println!("📊 Test Data (8 dimensions × 8 rows):");
    let vectors = create_test_vectors();

    for (i, record) in vectors.iter().enumerate() {
        println!("   Row {}: {:?}", i, record.vector);
    }

    println!("\n🔬 Testing Different Encoding Strategies");
    println!("========================================");

    // Test all encoding strategies
    test_strategy(&vectors, VectorEncodingLayout::FullVector, "FullVector");
    test_strategy(&vectors, VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector, "TransposeField");
    test_strategy(&vectors, VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector, "GroupedField");
    test_strategy(&vectors, VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector, "TransposeBlock");
    test_strategy(&vectors, VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector, "GroupedBlock");
    test_strategy(&vectors, VectorEncodingLayout::Auto, "Auto");

    println!("\n📝 Note: Run with RUST_LOG=debug to see algorithm selection details");
}