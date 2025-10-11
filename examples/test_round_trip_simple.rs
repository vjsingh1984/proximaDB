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
            // This helps test compression effectiveness
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
            timestamp: Some(1640995200 + (row as i64 * 60)),
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
) -> anyhow::Result<bool> {
    print!("Testing {:<35} ... ", test_name);
    use std::io::{self, Write};
    io::stdout().flush().unwrap();

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

    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());

    // Calculate original size
    let dimension = vectors[0].vector.len();
    let original_size = vectors.len() * dimension * 4; // 4 bytes per f32

    // Test encoding
    let encoded = block.serialize_with_config(&config)?;
    let encoded_size = encoded.len();
    let compression_ratio = original_size as f64 / encoded_size as f64;

    // Test decoding
    let decoded_block = ProximaDataBlock::deserialize(&encoded)?;

    // Verify data integrity
    if decoded_block.records.len() != vectors.len() {
        println!("❌ FAIL (count mismatch: {} vs {})", vectors.len(), decoded_block.records.len());
        return Ok(false);
    }

    for (i, (original, decoded)) in vectors.iter().zip(decoded_block.records.iter()).enumerate() {
        if original.vector.len() != decoded.vector.len() {
            println!("❌ FAIL (vec {} dim mismatch)", i);
            return Ok(false);
        }

        for (j, (orig_val, dec_val)) in original.vector.iter().zip(decoded.vector.iter()).enumerate() {
            let diff = (orig_val - dec_val).abs();
            if diff > 1e-5 {
                println!("❌ FAIL (vec[{}][{}]: {} vs {})", i, j, orig_val, dec_val);
                return Ok(false);
            }
        }
    }

    println!("✅ PASS [{:6} bytes, {:.2}x ratio]", encoded_size, compression_ratio);
    Ok(true)
}

fn main() -> anyhow::Result<()> {
    println!("🚀 Proxima Round-Trip Testing");
    println!("========================================");

    // Use consistent test data: 256 vectors × 256 dimensions (realistic high-dimensional dataset)
    const NUM_VECTORS: usize = 256;
    const DIMENSION: usize = 256;

    println!("\n📊 Test Configuration:");
    println!("   Vectors: {}", NUM_VECTORS);
    println!("   Dimensions: {}", DIMENSION);
    println!("   Total data: {} floats ({:.2} KB)",
             NUM_VECTORS * DIMENSION,
             (NUM_VECTORS * DIMENSION * 4) as f64 / 1024.0);

    let test_vectors = create_test_vectors(NUM_VECTORS, DIMENSION);

    // Define all test combinations
    let test_scenarios = vec![
        // Test all encoding strategies with different compression algorithms
        (VectorEncodingLayout::FullVector, CompressionAlgorithm::None, "FullVector + None"),
        (VectorEncodingLayout::FullVector, CompressionAlgorithm::Lz4, "FullVector + LZ4"),
        (VectorEncodingLayout::FullVector, CompressionAlgorithm::Zstd, "FullVector + Zstd"),
        (VectorEncodingLayout::FullVector, CompressionAlgorithm::Snappy, "FullVector + Snappy"),

        // Field-Encoded & Compressed strategies (field-level compression)
        (VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector, CompressionAlgorithm::None, "TransposeFieldEncoded + None"),
        (VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector, CompressionAlgorithm::Lz4, "TransposeFieldEncoded + LZ4"),
        (VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector, CompressionAlgorithm::Zstd, "TransposeFieldEncoded + Zstd"),
        (VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector, CompressionAlgorithm::Snappy, "TransposeFieldEncoded + Snappy"),

        (VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector, CompressionAlgorithm::None, "GroupedFieldEncoded + None"),
        (VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector, CompressionAlgorithm::Lz4, "GroupedFieldEncoded + LZ4"),
        (VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector, CompressionAlgorithm::Zstd, "GroupedFieldEncoded + Zstd"),
        (VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector, CompressionAlgorithm::Snappy, "GroupedFieldEncoded + Snappy"),

        // Block-Compressed strategies (block-level compression)
        (VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector, CompressionAlgorithm::None, "TransposeBlockCompressed + None"),
        (VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector, CompressionAlgorithm::Lz4, "TransposeBlockCompressed + LZ4"),
        (VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector, CompressionAlgorithm::Zstd, "TransposeBlockCompressed + Zstd"),
        (VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector, CompressionAlgorithm::Snappy, "TransposeBlockCompressed + Snappy"),

        (VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector, CompressionAlgorithm::None, "GroupedBlockCompressed + None"),
        (VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector, CompressionAlgorithm::Lz4, "GroupedBlockCompressed + LZ4"),
        (VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector, CompressionAlgorithm::Zstd, "GroupedBlockCompressed + Zstd"),
        (VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector, CompressionAlgorithm::Snappy, "GroupedBlockCompressed + Snappy"),

        // Test Auto selection
        (VectorEncodingLayout::Auto, CompressionAlgorithm::None, "Auto + None"),
        (VectorEncodingLayout::Auto, CompressionAlgorithm::Lz4, "Auto + LZ4"),
        (VectorEncodingLayout::Auto, CompressionAlgorithm::Zstd, "Auto + Zstd"),
    ];

    println!("\n=== Running {} Round-Trip Tests ===", test_scenarios.len());
    println!("────────────────────────────────────────────────────────");

    let mut results = Vec::new();
    let mut success_count = 0;

    for (layout, algorithm, name) in &test_scenarios {
        match test_round_trip(&test_vectors, *layout, *algorithm, name) {
            Ok(true) => {
                success_count += 1;
                results.push((name.to_string(), true, None));
            },
            Ok(false) => {
                results.push((name.to_string(), false, Some("Data verification failed".to_string())));
            },
            Err(e) => {
                results.push((name.to_string(), false, Some(e.to_string())));
            },
        }
    }

    // Print summary table
    println!("\n📈 TEST RESULTS SUMMARY");
    println!("════════════════════════════════════════════════════════");
    println!("{:<30} │ {:<10} │ {}", "Strategy", "Status", "Notes");
    println!("───────────────────────────────┼────────────┼──────────");

    for (name, success, error) in &results {
        let status = if *success { "✅ PASS" } else { "❌ FAIL" };
        let notes = match error {
            Some(err) => err.as_str(),
            None => "OK",
        };
        println!("{:<30} │ {:<10} │ {}", name, status, notes);
    }

    println!("════════════════════════════════════════════════════════");
    println!("\n📊 Overall Results: {}/{} tests passed ({:.1}%)",
             success_count,
             test_scenarios.len(),
             (success_count as f64 / test_scenarios.len() as f64) * 100.0);

    if success_count == test_scenarios.len() {
        println!("\n🎉 ALL ROUND-TRIPS SUCCESSFUL!");
    } else {
        println!("\n⚠️  {} tests failed - check details above",
                 test_scenarios.len() - success_count);
    }

    Ok(())
}