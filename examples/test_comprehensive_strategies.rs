use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, VectorEncodingLayout, ProximaDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value};
use std::collections::HashMap;

/// Static test data: 32 dimensions x 64 rows as requested
fn create_static_test_vectors() -> Vec<VectorRecord> {
    let mut vectors = Vec::new();

    // Create 64 vectors with 32 dimensions each
    for row in 0..64 {
        let mut vector = Vec::with_capacity(32);

        // Create patterns in the data for better compression testing
        for dim in 0..32 {
            let value = match dim % 4 {
                0 => (row as f32 + dim as f32) * 0.1,           // Linear pattern
                1 => ((row * dim) as f32).sin() * 0.5,          // Sine pattern
                2 => if row % 2 == 0 { 1.0 } else { -1.0 },     // Binary pattern
                3 => (dim as f32 / 32.0) + (row as f32 * 0.01), // Mixed pattern
                _ => unreachable!(),
            };
            vector.push(value);
        }

        vectors.push(VectorRecord {
            id: format!("static_vec_{:03}", row),
            vector,
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("row_index".to_string(), SqlValue {
                    value: Some(sql_value::Value::Int64Value(row as i64))
                });
                meta.insert("created_by".to_string(), SqlValue {
                    value: Some(sql_value::Value::StringValue("comprehensive_test".to_string()))
                });
                if row % 10 == 0 {
                    meta.insert("special_marker".to_string(), SqlValue {
                        value: Some(sql_value::Value::StringValue("checkpoint".to_string()))
                    });
                }
                meta
            },
            quantized_vector: vec![],
            expires_at: None,
            source: None,
            timestamp: 1640995200 + (row as i64 * 3600), // Static timestamps
            updated_at: None,
            version: None,
        });
    }

    vectors
}

fn test_configuration(
    vectors: &[VectorRecord],
    layout: VectorEncodingLayout,
    algorithm: CompressionAlgorithm,
    enable_compression: bool,
    test_name: &str,
) -> anyhow::Result<(usize, f64, bool)> {
    println!("\n🧪 Testing: {}", test_name);
    println!("   Layout: {:?}", layout);
    println!("   Algorithm: {:?}", algorithm);
    println!("   Compression: {}", if enable_compression { "ENABLED" } else { "DISABLED" });

    let config = BlockCompressionConfig {
        vector_layout: layout,
        algorithm,
        compression_level: 1,
        enable_vector_compression: enable_compression,
        enable_metadata_compression: false, // Keep consistent
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    let block = ProximaDataBlock::new(vectors.to_vec(), config.clone());

    // Test encoding
    let encoded = block.serialize_with_config(&config)?;
    let original_size = vectors.len() * 32 * 4; // 32 dimensions * 4 bytes per f32
    let compression_ratio = original_size as f64 / encoded.len() as f64;

    println!("   Original: {:.2} KB ({} bytes)", original_size as f64 / 1024.0, original_size);
    println!("   Encoded:  {:.2} KB ({} bytes)", encoded.len() as f64 / 1024.0, encoded.len());
    println!("   Ratio:    {:.2}x", compression_ratio);

    // Test round-trip decoding
    let decoded_block = ProximaDataBlock::deserialize(&encoded)?;
    let round_trip_success = decoded_block.records.len() == vectors.len();

    if round_trip_success {
        println!("   ✅ Round-trip: SUCCESS ({} vectors decoded)", decoded_block.records.len());

        // Verify first few vectors for data integrity
        let mut data_match = true;
        for (i, (original, decoded)) in vectors.iter().zip(decoded_block.records.iter()).take(3).enumerate() {
            if original.id != decoded.id {
                println!("   ❌ ID mismatch at {}: {} vs {}", i, original.id, decoded.id);
                data_match = false;
                break;
            }
            if original.vector.len() != decoded.vector.len() {
                println!("   ❌ Vector length mismatch at {}", i);
                data_match = false;
                break;
            }

            // Check dimensions with tolerance for compression artifacts
            for (j, (orig_val, dec_val)) in original.vector.iter().zip(decoded.vector.iter()).enumerate() {
                if (orig_val - dec_val).abs() > 1e-5 {
                    println!("   ❌ Vector value mismatch at {}[{}]: {} vs {}", i, j, orig_val, dec_val);
                    data_match = false;
                    break;
                }
            }
            if !data_match { break; }
        }

        if data_match {
            println!("   ✅ Data integrity: VERIFIED");
        }
    } else {
        println!("   ❌ Round-trip: FAILED");
        return Err(anyhow::anyhow!("Round-trip decoding failed"));
    }

    Ok((encoded.len(), compression_ratio, round_trip_success))
}

fn main() -> anyhow::Result<()> {
    println!("🚀 Comprehensive Proxima Strategy Testing");
    println!("===========================================");
    println!("📊 Test Data: 64 vectors × 32 dimensions (static patterns)");

    let vectors = create_static_test_vectors();
    let original_size = vectors.len() * 32 * 4;

    println!("📏 Original data size: {:.2} KB ({} bytes)", original_size as f64 / 1024.0, original_size);
    println!("🧮 Testing all combinations...\n");

    let layouts = vec![
        VectorEncodingLayout::FullVector,
        VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
        VectorEncodingLayout::Auto,
    ];

    let algorithms = vec![
        CompressionAlgorithm::None,
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Zstd,
    ];

    let compression_settings = vec![true, false];

    let mut results = Vec::new();
    let mut best_ratio = 0.0;
    let mut best_config = String::new();

    // Test all combinations
    for layout in &layouts {
        for algorithm in &algorithms {
            for &enable_compression in &compression_settings {
                let test_name = format!(
                    "{:?} + {:?} + {}",
                    layout,
                    algorithm,
                    if enable_compression { "COMPRESS" } else { "NO_COMPRESS" }
                );

                match test_configuration(&vectors, *layout, *algorithm, enable_compression, &test_name) {
                    Ok((encoded_size, ratio, success)) => {
                        results.push((test_name.clone(), encoded_size, ratio, success));
                        if success && ratio > best_ratio {
                            best_ratio = ratio;
                            best_config = test_name;
                        }
                    }
                    Err(e) => {
                        println!("   ❌ FAILED: {}", e);
                        results.push((test_name, 0, 0.0, false));
                    }
                }
            }
        }
    }

    // Summary report
    println!("\n📈 COMPREHENSIVE RESULTS SUMMARY");
    println!("=================================");

    println!("\n🏆 BEST PERFORMING CONFIGURATION:");
    println!("   Config: {}", best_config);
    println!("   Ratio: {:.2}x compression", best_ratio);

    println!("\n📊 ALL RESULTS:");
    println!("{:<40} {:>12} {:>10} {:>8}", "Configuration", "Size (bytes)", "Ratio", "Success");
    println!("{}", "-".repeat(75));

    for (config, size, ratio, success) in &results {
        let success_marker = if *success { "✅" } else { "❌" };
        println!("{:<40} {:>12} {:>10.2}x {:>6}", config, size, ratio, success_marker);
    }

    // Analysis
    println!("\n🔍 ANALYSIS:");

    let full_vector_results: Vec<_> = results.iter()
        .filter(|(config, _, _, success)| config.contains("FullVector") && *success)
        .collect();
    let transpose_results: Vec<_> = results.iter()
        .filter(|(config, _, _, success)| config.contains("TransposeField") && *success)
        .collect();

    if !full_vector_results.is_empty() {
        let avg_full_vector = full_vector_results.iter().map(|(_, _, ratio, _)| ratio).sum::<f64>() / full_vector_results.len() as f64;
        println!("   FullVector average ratio: {:.2}x", avg_full_vector);
    }

    if !transpose_results.is_empty() {
        let avg_transpose = transpose_results.iter().map(|(_, _, ratio, _)| ratio).sum::<f64>() / transpose_results.len() as f64;
        println!("   TransposeField average ratio: {:.2}x", avg_transpose);
    }

    // Identify compression benefit
    let no_compression_results: Vec<_> = results.iter()
        .filter(|(config, _, _, success)| config.contains("NO_COMPRESS") && *success)
        .collect();
    let with_compression_results: Vec<_> = results.iter()
        .filter(|(config, _, _, success)| config.contains("COMPRESS") && *success)
        .collect();

    if !no_compression_results.is_empty() && !with_compression_results.is_empty() {
        let avg_no_compress = no_compression_results.iter().map(|(_, _, ratio, _)| ratio).sum::<f64>() / no_compression_results.len() as f64;
        let avg_with_compress = with_compression_results.iter().map(|(_, _, ratio, _)| ratio).sum::<f64>() / with_compression_results.len() as f64;

        println!("   Without compression: {:.2}x average", avg_no_compress);
        println!("   With compression: {:.2}x average", avg_with_compress);
        println!("   Compression benefit: {:.1}% improvement", ((avg_with_compress / avg_no_compress) - 1.0) * 100.0);
    }

    println!("\n✅ Comprehensive testing complete!");
    println!("   Total configurations tested: {}", results.len());
    println!("   Successful configurations: {}", results.iter().filter(|(_, _, _, success)| *success).count());

    Ok(())
}