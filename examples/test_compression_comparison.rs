use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value};
use std::collections::HashMap;

fn create_test_vectors(num_vectors: usize, dimension: usize, pattern: &str) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for row in 0..num_vectors {
        let mut vector = Vec::with_capacity(dimension);

        match pattern {
            "sparse" => {
                // Mostly zeros with some non-zero values
                for dim in 0..dimension {
                    if dim % 10 == 0 {
                        vector.push((row as f32 * 0.01 + dim as f32 * 0.001));
                    } else {
                        vector.push(0.0);
                    }
                }
            },
            "normalized" => {
                // Normalized embeddings in range [-1, 1]
                for dim in 0..dimension {
                    let value = ((row + dim) as f32 * 0.123).sin() * 0.8;
                    vector.push(value);
                }
            },
            "sequential" => {
                // Sequential/monotonic values
                for dim in 0..dimension {
                    vector.push(row as f32 * dimension as f32 + dim as f32);
                }
            },
            _ => {
                // Random pattern (default)
                for dim in 0..dimension {
                    let value = ((row as f32) * 0.01 + (dim as f32) * 0.1) +
                               (row as f32 * dim as f32).sin() * 0.05;
                    vector.push(value);
                }
            }
        }

        vectors.push(VectorRecord {
            id: format!("vec_{:06}", row),
            vector,
            metadata: HashMap::new(),
            quantized_vector: vec![],
            expires_at: None,
            source: None,
            timestamp: 0,
            updated_at: None,
            version: None,
        });
    }

    vectors
}

fn measure_compression(
    vectors: &[VectorRecord],
    layout: VectorEncodingLayout,
    algorithm: CompressionAlgorithm,
) -> (usize, f64, f64) {
    let config = BlockCompressionConfig {
        vector_layout: layout,
        algorithm,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());

    // Calculate original size
    let dimension = vectors[0].vector.len();
    let original_size = vectors.len() * dimension * 4; // 4 bytes per f32

    // Test encoding
    let start = std::time::Instant::now();
    let encoded = block.serialize_with_config(&config).unwrap();
    let encode_time = start.elapsed().as_secs_f64() * 1000.0; // ms

    let encoded_size = encoded.len();
    let compression_ratio = original_size as f64 / encoded_size as f64;

    (encoded_size, compression_ratio, encode_time)
}

fn main() -> anyhow::Result<()> {
    println!("🔬 Proxima Compression Analysis");
    println!("==================================\n");

    let test_configs = vec![
        (256, 256, "random"),
        (256, 256, "sparse"),
        (256, 256, "normalized"),
        (256, 256, "sequential"),
        (1024, 768, "normalized"),  // BERT-like
        (1024, 1536, "normalized"), // GPT-like
    ];

    for (num_vectors, dimension, pattern) in test_configs {
        println!("📊 Test: {} vectors × {} dimensions ({})", num_vectors, dimension, pattern);
        println!("─────────────────────────────────────────");

        let vectors = create_test_vectors(num_vectors, dimension, pattern);
        let original_size = num_vectors * dimension * 4;
        println!("Original size: {:.2} MB\n", original_size as f64 / 1_048_576.0);

        // Test different strategies
        let strategies = vec![
            ("FullVector", VectorEncodingLayout::FullVector),
            ("GroupedField", VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector),
            ("TransposeField", VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
        ];

        let algorithms = vec![
            ("None", CompressionAlgorithm::None),
            ("LZ4", CompressionAlgorithm::Lz4),
            ("Zstd", CompressionAlgorithm::Zstd),
        ];

        println!("{:<20} {:<10} {:>12} {:>10} {:>10}",
                 "Strategy", "Algorithm", "Size (KB)", "Ratio", "Time (ms)");
        println!("{}", "─".repeat(65));

        for (strategy_name, layout) in &strategies {
            for (algo_name, algorithm) in &algorithms {
                let (size, ratio, time) = measure_compression(&vectors, layout.clone(), algorithm.clone());

                println!("{:<20} {:<10} {:>12.1} {:>10.2}x {:>10.2}",
                         strategy_name,
                         algo_name,
                         size as f64 / 1024.0,
                         ratio,
                         time);
            }
        }
        println!();
    }

    println!("✅ Compression analysis complete!");
    Ok(())
}