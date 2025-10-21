use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, ProximaDataBlock, VectorEncodingLayout,
};
use std::collections::HashMap;
use tracing::{debug, info, trace};

fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt().with_env_filter("debug").init();

    info!("Starting encoding debug test");

    // Create a simple test vector
    let vectors = vec![
        VectorRecord {
            id: "test_0".to_string(),
            vector: vec![0.0, 0.1, 0.2, 0.3],
            metadata: HashMap::new(),
            expires_at: None,
            source: None,
            timestamp: Some(0),
            updated_at: None,
            version: None,
        },
        VectorRecord {
            id: "test_1".to_string(),
            vector: vec![1.0, 1.1, 1.2, 1.3],
            metadata: HashMap::new(),
            expires_at: None,
            source: None,
            timestamp: Some(0),
            updated_at: None,
            version: None,
        },
    ];

    debug!("Original vectors:");
    for (i, v) in vectors.iter().enumerate() {
        debug!("  Vector {}: {:?}", i, v.vector);
    }

    let config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::None,
        compression_level: 1,
        enable_vector_compression: false,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    info!("Creating ProximaDataBlock");
    let block = ProximaDataBlock::new(vectors.clone(), config.clone());

    info!("Serializing block");
    let encoded = block.serialize_with_config(&config)?;
    info!("Encoded size: {} bytes", encoded.len());

    // Print first few bytes
    debug!(
        "First 32 bytes of encoded data: {:02X?}",
        &encoded[..encoded.len().min(32)]
    );

    info!("Deserializing block");
    let decoded_block = ProximaDataBlock::deserialize(&encoded, None)?;

    info!("Comparing vectors");
    for (i, (orig, dec)) in vectors.iter().zip(decoded_block.records.iter()).enumerate() {
        debug!("Vector {} comparison:", i);
        debug!("  Original: {:?}", orig.vector);
        debug!("  Decoded:  {:?}", dec.vector);

        for (j, (o, d)) in orig.vector.iter().zip(dec.vector.iter()).enumerate() {
            if (o - d).abs() > 1e-6 {
                info!("  MISMATCH at [{}][{}]: {} vs {}", i, j, o, d);
                info!("    Binary: {:032b} vs {:032b}", o.to_bits(), d.to_bits());
            }
        }
    }

    Ok(())
}
