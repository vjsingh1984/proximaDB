use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    // Create a simple test vector with known values
    let mut vector = vec![0.0f32; 256];
    for i in 0..256 {
        vector[i] = (i as f32) * 0.1;
    }
    
    let record = VectorRecord {
        id: "test".to_string(),
        vector: vector.clone(),
        metadata: HashMap::new(),
        expires_at: None,
        source: None,
        timestamp: Some(0),
        updated_at: None,
        version: None,
    };
    
    let config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
        algorithm: CompressionAlgorithm::None,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };
    
    let block = ProximaDataBlock::new(vec![record], config.clone());
    let encoded = block.serialize_with_config(&config)?;
    println!("Encoded size: {} bytes", encoded.len());
    
    let decoded_block = ProximaDataBlock::deserialize(&encoded)?;
    let decoded = &decoded_block.records[0].vector;
    
    // Check values
    for i in 0..20 {
        let expected = (i as f32) * 0.1;
        let actual = decoded[i];
        if (expected - actual).abs() > 1e-5 {
            println!("MISMATCH at [{}]: expected={}, actual={}", i, expected, actual);
        } else {
            println!("OK at [{}]: {}", i, actual);
        }
    }
    
    Ok(())
}
