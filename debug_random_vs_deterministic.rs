use proximadb::storage::engines::core::formats::proxima_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn create_deterministic_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..num_vectors).map(|row| VectorRecord {
        id: format!("det_{}", row),
        vector: (0..dimension).map(|dim| {
            ((row as f32) * 0.01 + (dim as f32) * 0.1) +
            (row as f32 * dim as f32).sin() * 0.05
        }).collect(),
        metadata: HashMap::new(),
        quantized_vector: vec![],
        expires_at: None,
        source: None,
        timestamp: row as i64,
        updated_at: None,
        version: None,
    }).collect()
}

fn create_random_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    (0..num_vectors).map(|row| VectorRecord {
        id: format!("rnd_{}", row),
        vector: (0..dimension).map(|_| rng.gen::<f32>()).collect(),
        metadata: HashMap::new(),
        quantized_vector: vec![],
        expires_at: None,
        source: None,
        timestamp: row as i64,
        updated_at: None,
        version: None,
    }).collect()
}

fn test_encoding(vectors: Vec<VectorRecord>, name: &str) -> Result<(), String> {
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

    println!("Testing {}: {} vectors", name, vectors.len());

    let block = ProximaDataBlock::new(vectors.clone(), config.clone());
    let encoded = block.serialize_with_config(&config)
        .map_err(|e| format!("Serialization failed: {}", e))?;

    println!("  Encoded: {} bytes", encoded.len());

    let decoded_block = ProximaDataBlock::deserialize(&encoded)
        .map_err(|e| format!("Deserialization failed: {}", e))?;

    println!("  Decoded: {} vectors", decoded_block.records.len());

    // Check data integrity
    for (i, (orig, dec)) in vectors.iter().zip(decoded_block.records.iter()).take(3).enumerate() {
        if orig.id != dec.id {
            return Err(format!("ID mismatch at {}", i));
        }

        for (j, (a, b)) in orig.vector.iter().zip(dec.vector.iter()).enumerate() {
            if (a - b).abs() > 1e-5 {
                return Err(format!("Vector mismatch at {}[{}]: {} vs {}", i, j, a, b));
            }
        }
    }

    println!("  ✅ Round-trip successful");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Debug: Random vs Deterministic Data Encoding");
    println!("==============================================");

    let det_vectors = create_deterministic_vectors(10, 32);
    let rnd_vectors = create_random_vectors(10, 32);

    test_encoding(det_vectors, "Deterministic")?;
    test_encoding(rnd_vectors, "Random")?;

    println!("\nIf random fails but deterministic passes, we found the issue!");
    Ok(())
}