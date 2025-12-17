use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, ProximaDataBlock, VectorEncodingLayout,
};
use std::collections::HashMap;

/// Different data patterns to test
#[derive(Debug, Clone)]
enum DataPattern {
    Random,     // Random floats [0, 1)
    Sparse,     // Mostly zeros with few non-zero values
    Sequential, // Sequential values with small deltas
    Normalized, // Values in small range [-1, 1]
    Constant,   // All values the same
    Sine,       // Sine wave pattern (original)
}

impl DataPattern {
    fn name(&self) -> &str {
        match self {
            DataPattern::Random => "Random",
            DataPattern::Sparse => "Sparse",
            DataPattern::Sequential => "Sequential",
            DataPattern::Normalized => "Normalized",
            DataPattern::Constant => "Constant",
            DataPattern::Sine => "Sine",
        }
    }

    /// Generate test data based on pattern
    fn generate(&self, num_vectors: usize, dimension: usize) -> Vec<Vec<f32>> {
        let mut vectors = Vec::with_capacity(num_vectors);

        match self {
            DataPattern::Random => {
                // Random values in [0, 1)
                use rand::Rng;
                let mut rng = rand::thread_rng();
                for _ in 0..num_vectors {
                    let mut vec = Vec::with_capacity(dimension);
                    for _ in 0..dimension {
                        vec.push(rng.r#gen::<f32>());
                    }
                    vectors.push(vec);
                }
            }
            DataPattern::Sparse => {
                // 95% zeros, 5% non-zero values
                use rand::Rng;
                let mut rng = rand::thread_rng();
                for _ in 0..num_vectors {
                    let mut vec = vec![0.0f32; dimension];
                    // Randomly set 5% of values to non-zero
                    let non_zero_count = (dimension as f32 * 0.05) as usize;
                    for _ in 0..non_zero_count {
                        let idx = rng.gen_range(0..dimension);
                        vec[idx] = rng.gen_range(-10.0..10.0);
                    }
                    vectors.push(vec);
                }
            }
            DataPattern::Sequential => {
                // Sequential values with small increments
                for i in 0..num_vectors {
                    let mut vec = Vec::with_capacity(dimension);
                    let base = (i * dimension) as f32;
                    for j in 0..dimension {
                        // Small random delta to avoid perfect sequences
                        let delta = (j as f32) * 0.01 + ((i + j) % 7) as f32 * 0.001;
                        vec.push(base + j as f32 + delta);
                    }
                    vectors.push(vec);
                }
            }
            DataPattern::Normalized => {
                // Normalized vectors with values in [-1, 1]
                use rand::Rng;
                let mut rng = rand::thread_rng();
                for _ in 0..num_vectors {
                    let mut vec = Vec::with_capacity(dimension);
                    let mut sum_sq = 0.0f32;

                    // Generate random values and compute norm
                    for _ in 0..dimension {
                        let val = rng.gen_range(-1.0..1.0);
                        vec.push(val);
                        sum_sq += val * val;
                    }

                    // Normalize to unit length
                    let norm = sum_sq.sqrt();
                    if norm > 0.0 {
                        for val in &mut vec {
                            *val /= norm;
                        }
                    }
                    vectors.push(vec);
                }
            }
            DataPattern::Constant => {
                // All values are the same
                let constant_value = 42.0f32;
                for _ in 0..num_vectors {
                    vectors.push(vec![constant_value; dimension]);
                }
            }
            DataPattern::Sine => {
                // Original sine wave pattern
                for i in 0..num_vectors {
                    let mut vec = Vec::with_capacity(dimension);
                    for j in 0..dimension {
                        let value = ((i * dimension + j) as f32 * 0.1).sin();
                        vec.push(value);
                    }
                    vectors.push(vec);
                }
            }
        }

        vectors
    }
}

fn create_vector_records(vectors: Vec<Vec<f32>>) -> Vec<VectorRecord> {
    vectors
        .into_iter()
        .enumerate()
        .map(|(i, vector)| VectorRecord {
            id: format!("vec_{}", i),
            vector,
            metadata: HashMap::new(),
            timestamp: Some(1640995200 + (i as i64 * 60)),
            expires_at: None,
            source: Some(format!("pattern_test")),
            updated_at: None,
            version: None,
        })
        .collect()
}

fn test_round_trip(
    pattern: &DataPattern,
    layout: VectorEncodingLayout,
    algorithm: CompressionAlgorithm,
    vectors: &[Vec<f32>],
) -> Result<(bool, usize, f64), String> {
    let config = BlockCompressionConfig {
        vector_layout: layout,
        algorithm,
        compression_level: 3,
        enable_vector_compression: algorithm != CompressionAlgorithm::None,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    // Create records
    let records = create_vector_records(vectors.to_vec());
    let original_count = records.len();
    let dimension = if !records.is_empty() {
        records[0].vector.len()
    } else {
        0
    };

    // Create block and serialize
    let block = ProximaDataBlock::new(records.clone(), config.clone());
    let serialized = block
        .serialize_with_config(&config)
        .map_err(|e| format!("Serialization failed: {}", e))?;

    let serialized_size = serialized.len();
    let original_size = original_count * dimension * std::mem::size_of::<f32>();
    let compression_ratio = original_size as f64 / serialized_size as f64;

    // Deserialize
    let deserialized_block = ProximaDataBlock::deserialize(&serialized, None)
        .map_err(|e| format!("Deserialization failed: {}", e))?;

    // Verify
    if deserialized_block.records.len() != original_count {
        return Ok((false, serialized_size, compression_ratio));
    }

    // Check vectors
    for (original, decoded) in records.iter().zip(deserialized_block.records.iter()) {
        if original.id != decoded.id {
            return Ok((false, serialized_size, compression_ratio));
        }
        if original.vector.len() != decoded.vector.len() {
            return Ok((false, serialized_size, compression_ratio));
        }
        for (a, b) in original.vector.iter().zip(decoded.vector.iter()) {
            if (a - b).abs() > 1e-5 {
                return Ok((false, serialized_size, compression_ratio));
            }
        }
    }

    Ok((true, serialized_size, compression_ratio))
}

fn main() {
    println!("🚀 Proxima Pattern-Based Round-Trip Testing");
    println!("{}", "=".repeat(80));

    // Test configuration
    let num_vectors = 256;
    let dimension = 256;

    println!("\n📊 Test Configuration:");
    println!("   Vectors: {}", num_vectors);
    println!("   Dimensions: {}", dimension);
    println!(
        "   Total data: {} floats ({:.2} KB)\n",
        num_vectors * dimension,
        (num_vectors * dimension * 4) as f64 / 1024.0
    );

    // Data patterns to test
    let patterns = vec![
        DataPattern::Random,
        DataPattern::Sparse,
        DataPattern::Sequential,
        DataPattern::Normalized,
        DataPattern::Constant,
        DataPattern::Sine,
    ];

    // Encoding strategies to test
    let strategies = vec![
        ("FullVector", VectorEncodingLayout::FullVector),
        (
            "TransposeField",
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        ),
        (
            "GroupedField",
            VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
        ),
        (
            "TransposeBlock",
            VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector,
        ),
        (
            "GroupedBlock",
            VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector,
        ),
    ];

    // Compression algorithms to test
    let algorithms = vec![
        ("None", CompressionAlgorithm::None),
        ("LZ4", CompressionAlgorithm::Lz4),
        ("Zstd", CompressionAlgorithm::Zstd),
        ("Snappy", CompressionAlgorithm::Snappy),
    ];

    let mut total_tests = 0;
    let mut passed_tests = 0;
    let mut results = Vec::new();

    // Test each pattern
    for pattern in &patterns {
        println!("\n🎯 Testing Pattern: {}", pattern.name());
        println!("{}", "-".repeat(60));

        // Generate data for this pattern
        let vectors = pattern.generate(num_vectors, dimension);

        // Test each strategy with each compression algorithm
        for (strat_name, strategy) in &strategies {
            for (algo_name, algorithm) in &algorithms {
                total_tests += 1;

                let test_name = format!("{} + {}", strat_name, algo_name);
                print!("  {:.<45} ", test_name);

                match test_round_trip(&pattern, *strategy, *algorithm, &vectors) {
                    Ok((true, size, ratio)) => {
                        passed_tests += 1;
                        println!("✅ PASS [{:>7} bytes, {:.2}x]", size, ratio);
                        results.push((
                            pattern.name().to_string(),
                            test_name.clone(),
                            "PASS".to_string(),
                            size,
                            ratio,
                        ));
                    }
                    Ok((false, size, ratio)) => {
                        println!("❌ FAIL [data mismatch, {} bytes]", size);
                        results.push((
                            pattern.name().to_string(),
                            test_name.clone(),
                            "FAIL".to_string(),
                            size,
                            ratio,
                        ));
                    }
                    Err(e) => {
                        println!("❌ ERROR [{}]", e);
                        results.push((
                            pattern.name().to_string(),
                            test_name.clone(),
                            format!("ERROR: {}", e),
                            0,
                            0.0,
                        ));
                    }
                }
            }
        }
    }

    // Print summary table
    println!("\n\n📈 COMPREHENSIVE TEST RESULTS");
    println!("{}", "=".repeat(80));
    println!(
        "{:<12} │ {:<30} │ {:<8} │ {:>10} │ {:>8}",
        "Pattern", "Strategy", "Status", "Size", "Ratio"
    );
    println!("{}", "-".repeat(80));

    for (pattern, strategy, status, size, ratio) in &results {
        let status_icon = if status == "PASS" { "✅" } else { "❌" };
        println!(
            "{:<12} │ {:<30} │ {} {:<6} │ {:>10} │ {:>8.2}x",
            pattern,
            strategy,
            status_icon,
            if status == "PASS" { "PASS" } else { "FAIL" },
            size,
            ratio
        );
    }

    println!("{}", "=".repeat(80));

    // Pattern-specific compression analysis
    println!("\n📊 COMPRESSION ANALYSIS BY PATTERN");
    println!("{}", "=".repeat(80));

    for pattern in &patterns {
        let pattern_name = pattern.name();
        let pattern_results: Vec<_> = results
            .iter()
            .filter(|r| r.0 == pattern_name && r.2 == "PASS".to_string())
            .collect();

        if !pattern_results.is_empty() {
            let best = pattern_results
                .iter()
                .max_by(|a, b| a.4.partial_cmp(&b.4).unwrap())
                .unwrap();

            let worst = pattern_results
                .iter()
                .min_by(|a, b| a.4.partial_cmp(&b.4).unwrap())
                .unwrap();

            println!("\n{} Pattern:", pattern_name);
            println!("  Best:  {} → {:.2}x compression", best.1, best.4);
            println!("  Worst: {} → {:.2}x compression", worst.1, worst.4);
        }
    }

    println!("\n{}", "=".repeat(80));
    println!(
        "📊 Overall Results: {}/{} tests passed ({:.1}%)",
        passed_tests,
        total_tests,
        (passed_tests as f64 / total_tests as f64) * 100.0
    );

    if passed_tests == total_tests {
        println!("\n🎉 ALL TESTS PASSED!");
    } else {
        println!("\n⚠️  Some tests failed. Check results above.");
    }
}
