use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value};
use std::collections::HashMap;

fn create_test_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    for row in 0..num_vectors {
        let mut vector = Vec::with_capacity(dimension);
        for dim in 0..dimension {
            // Create a pattern that tests different value ranges
            let value = ((row as f32) * 0.01 + (dim as f32) * 0.1) +
                       (row as f32 * dim as f32).sin() * 0.05;
            vector.push(value);
        }

        vectors.push(VectorRecord {
            id: format!("vec_{:06}", row),
            vector,
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("row_index".to_string(), SqlValue {
                    value: Some(sql_value::Value::Int64Value(row as i64))
                });
                meta.insert("category".to_string(), SqlValue {
                    value: Some(sql_value::Value::StringValue(format!("cat_{}", row % 10)))
                });
                meta
            },
            expires_at: None,
            source: Some(format!("batch_{}", row / 100)),
            timestamp: 1640995200 + (row as i64 * 60),
            updated_at: None,
            version: None,
        });
    }

    vectors
}

struct CompressionResult {
    strategy: String,
    algorithm: String,
    original_size: usize,
    encoded_size: usize,
    compression_ratio: f64,
    encode_time_ms: f64,
    decode_time_ms: f64,
}

fn test_compression(
    vectors: &[VectorRecord],
    layout: VectorEncodingLayout,
    algorithm: CompressionAlgorithm,
) -> anyhow::Result<CompressionResult> {
    use std::time::Instant;

    let strategy_name = match layout {
        VectorEncodingLayout::FullVector => "FullVector",
        VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector => "TransposeFieldEncoded",
        VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector => "GroupedFieldEncoded",
        VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector => "TransposeBlockCompressed",
        VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector => "GroupedBlockCompressed",
        VectorEncodingLayout::Auto => "Auto",
    };

    let algorithm_name = match algorithm {
        CompressionAlgorithm::None => "None",
        CompressionAlgorithm::Lz4 => "LZ4",
        CompressionAlgorithm::Zstd => "Zstd",
        CompressionAlgorithm::Snappy => "Snappy",
        CompressionAlgorithm::Gzip => "Gzip",
        _ => "Unknown",
    };

    let config = BlockCompressionConfig {
        vector_layout: layout,
        algorithm,
        compression_level: 1,
        enable_vector_compression: true,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None, // Use main algorithm for metadata
    };

    let dimension = vectors[0].vector.len();
    let original_size = vectors.len() * dimension * 4; // 4 bytes per f32

    // Measure encoding time
    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());
    let encode_start = Instant::now();
    let encoded = block.serialize_with_config(&config)?;
    let encode_time_ms = encode_start.elapsed().as_secs_f64() * 1000.0;

    let encoded_size = encoded.len();
    let compression_ratio = original_size as f64 / encoded_size as f64;

    // Measure decoding time
    let decode_start = Instant::now();
    let _decoded_block = ProximaDataBlock::deserialize(&encoded)?;
    let decode_time_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

    Ok(CompressionResult {
        strategy: strategy_name.to_string(),
        algorithm: algorithm_name.to_string(),
        original_size,
        encoded_size,
        compression_ratio,
        encode_time_ms,
        decode_time_ms,
    })
}

fn main() -> anyhow::Result<()> {
    println!("📊 Proxima Compression Metrics Analysis");
    println!("===========================================");

    const NUM_VECTORS: usize = 256;
    const DIMENSION: usize = 256;

    println!("\n📐 Test Configuration:");
    println!("   Vectors: {}", NUM_VECTORS);
    println!("   Dimensions: {}", DIMENSION);
    println!("   Raw data size: {} bytes ({:.2} KB)",
             NUM_VECTORS * DIMENSION * 4,
             (NUM_VECTORS * DIMENSION * 4) as f64 / 1024.0);

    let test_vectors = create_test_vectors(NUM_VECTORS, DIMENSION);

    let strategies = vec![
        VectorEncodingLayout::FullVector,
        VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
        VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector,
        VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector,
    ];

    let algorithms = vec![
        CompressionAlgorithm::None,
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Zstd,
        CompressionAlgorithm::Snappy,
    ];

    let mut results = Vec::new();

    // Run tests
    println!("\n⚙️  Running compression tests...\n");
    for strategy in &strategies {
        for algorithm in &algorithms {
            match test_compression(&test_vectors, *strategy, *algorithm) {
                Ok(result) => {
                    println!("   ✓ {} + {} complete", result.strategy, result.algorithm);
                    results.push(result);
                }
                Err(e) => {
                    println!("   ✗ {:?} + {:?} failed: {}", strategy, algorithm, e);
                }
            }
        }
    }

    // Print detailed results table
    println!("\n📈 DETAILED RESULTS");
    println!("═══════════════════════════════════════════════════════════════════════════════════════════");
    println!("{:<15} │ {:<8} │ {:>10} │ {:>10} │ {:>8} │ {:>10} │ {:>10}",
             "Strategy", "Algorithm", "Original", "Encoded", "Ratio", "Encode(ms)", "Decode(ms)");
    println!("───────────────┼──────────┼────────────┼────────────┼──────────┼────────────┼────────────");

    for result in &results {
        println!("{:<15} │ {:<8} │ {:>10} │ {:>10} │ {:>8.2}x │ {:>10.2} │ {:>10.2}",
                 result.strategy,
                 result.algorithm,
                 result.original_size,
                 result.encoded_size,
                 result.compression_ratio,
                 result.encode_time_ms,
                 result.decode_time_ms);
    }
    println!("═══════════════════════════════════════════════════════════════════════════════════════════");

    // Find best configurations
    println!("\n🏆 BEST CONFIGURATIONS");
    println!("────────────────────────────────────────────────────");

    // Best compression ratio
    if let Some(best_ratio) = results.iter().max_by(|a, b| a.compression_ratio.partial_cmp(&b.compression_ratio).unwrap()) {
        println!("Best Compression Ratio: {} + {} ({:.2}x)",
                 best_ratio.strategy, best_ratio.algorithm, best_ratio.compression_ratio);
    }

    // Fastest encoding
    if let Some(fastest_encode) = results.iter().min_by(|a, b| a.encode_time_ms.partial_cmp(&b.encode_time_ms).unwrap()) {
        println!("Fastest Encoding: {} + {} ({:.2} ms)",
                 fastest_encode.strategy, fastest_encode.algorithm, fastest_encode.encode_time_ms);
    }

    // Fastest decoding
    if let Some(fastest_decode) = results.iter().min_by(|a, b| a.decode_time_ms.partial_cmp(&b.decode_time_ms).unwrap()) {
        println!("Fastest Decoding: {} + {} ({:.2} ms)",
                 fastest_decode.strategy, fastest_decode.algorithm, fastest_decode.decode_time_ms);
    }

    // Best balance (ratio * speed)
    let mut best_balance: Option<(&CompressionResult, f64)> = None;
    for result in &results {
        let balance_score = result.compression_ratio / (result.encode_time_ms + result.decode_time_ms + 0.1);
        if best_balance.is_none() || balance_score > best_balance.as_ref().unwrap().1 {
            best_balance = Some((result, balance_score));
        }
    }

    if let Some((best, _)) = best_balance {
        println!("Best Balance (ratio/time): {} + {}",
                 best.strategy, best.algorithm);
    }

    // Strategy comparison
    println!("\n📊 STRATEGY COMPARISON (averaged across algorithms)");
    println!("────────────────────────────────────────────────────");

    for strategy in &strategies {
        let strategy_name = match strategy {
            VectorEncodingLayout::FullVector => "FullVector",
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector => "TransposeFieldEncoded",
            VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector => "GroupedFieldEncoded",
            VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector => "TransposeBlockCompressed",
            VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector => "GroupedBlockCompressed",
            VectorEncodingLayout::Auto => "Auto",
        };

        let strategy_results: Vec<_> = results.iter()
            .filter(|r| r.strategy == strategy_name)
            .collect();

        if !strategy_results.is_empty() {
            let avg_ratio: f64 = strategy_results.iter()
                .map(|r| r.compression_ratio)
                .sum::<f64>() / strategy_results.len() as f64;

            let avg_size: f64 = strategy_results.iter()
                .map(|r| r.encoded_size as f64)
                .sum::<f64>() / strategy_results.len() as f64;

            println!("{:<20}: Avg ratio {:.2}x, Avg size {:.0} bytes",
                     strategy_name, avg_ratio, avg_size);
        }
    }

    println!("\n✅ Analysis complete!");
    Ok(())
}