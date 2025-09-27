use proximadb::storage::engines::core::formats::proxima_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{:06}", i),
            vector: (0..dimension).map(|d| (i as f32 + d as f32) * 0.1).collect(),
            metadata: HashMap::new(),
            quantized_vector: vec![], // bytes field, not Option
            expires_at: None,
            source: None,
            timestamp: chrono::Utc::now().timestamp(),
            updated_at: None,
            version: None,
        })
        .collect()
}

fn main() {
    println!("🚀 Testing Block-Level Compression for FullVector Layout");
    println!("========================================================");

    let vectors = create_test_vectors(1000, 768);
    let uncompressed_size = vectors.len() * 768 * 4; // f32 = 4 bytes

    // Test with compression DISABLED (should get 1.0x like before)
    let config_no_compression = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: false,  // DISABLED
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    // Test with compression ENABLED (should get 3-6x with our optimization)
    let config_with_compression = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: true,  // ENABLED
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    // Create Proxima block
    let block = ProximaDataBlock::new(vectors, config_no_compression.clone());

    // Test without compression
    match block.serialize_with_config(&config_no_compression) {
        Ok(encoded_no_compression) => {
            let ratio_no_compression = uncompressed_size as f64 / encoded_no_compression.len() as f64;
            println!("❌ FullVector WITHOUT block compression:");
            println!("   Original size: {:.2} MB", uncompressed_size as f64 / 1_000_000.0);
            println!("   Encoded size:  {:.2} MB", encoded_no_compression.len() as f64 / 1_000_000.0);
            println!("   Compression ratio: {:.2}x", ratio_no_compression);
        }
        Err(e) => println!("❌ Encoding without compression failed: {}", e),
    }

    // Test with compression
    match block.serialize_with_config(&config_with_compression) {
        Ok(encoded_with_compression) => {
            let ratio_with_compression = uncompressed_size as f64 / encoded_with_compression.len() as f64;
            println!("\n✅ FullVector WITH block compression:");
            println!("   Original size: {:.2} MB", uncompressed_size as f64 / 1_000_000.0);
            println!("   Encoded size:  {:.2} MB", encoded_with_compression.len() as f64 / 1_000_000.0);
            println!("   Compression ratio: {:.2}x", ratio_with_compression);

            if ratio_with_compression > 2.0 {
                println!("\n🎉 SUCCESS: Block-level compression is working!");
                println!("   FullVector encoding now achieves meaningful compression");
            } else {
                println!("\n⚠️  Block-level compression may not be fully activated");
            }
        }
        Err(e) => println!("❌ Encoding with compression failed: {}", e),
    }

    // Compare with TransposeVector for reference
    let transpose_config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    match block.serialize_with_config(&transpose_config) {
        Ok(encoded_transpose) => {
            let ratio_transpose = uncompressed_size as f64 / encoded_transpose.len() as f64;
            println!("\n📊 TransposeVector reference (for comparison):");
            println!("   Compression ratio: {:.2}x", ratio_transpose);
        }
        Err(e) => println!("❌ TransposeVector encoding failed: {}", e),
    }

    println!("\n🎯 Expected Results:");
    println!("   FullVector without compression: ~1.0x (no benefit)");
    println!("   FullVector with block compression: 3-6x (economies of scale)");
    println!("   TransposeVector: 15-20x (pattern optimization)");
}