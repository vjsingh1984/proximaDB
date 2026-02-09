#[cfg(test)]
mod tests {
    use flate2::Compression;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use std::io::{Read, Write};
    use tracing::debug;

    #[test]
    fn test_gzip_compression() {
        // Test gzip compression/decompression
        let original_data = b"This is test data for compression. ".repeat(100);

        // Compress
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&original_data).unwrap();
        let compressed = encoder.finish().unwrap();

        // Verify compression worked
        assert!(compressed.len() < original_data.len());
        assert!(compressed.len() < original_data.len() / 2); // Should compress well

        // Decompress
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original_data);
    }

    #[test]
    fn test_deflate_compression() {
        use flate2::read::DeflateDecoder;
        use flate2::write::DeflateEncoder;

        let original_data = b"Vector data simulation: ".repeat(100);

        // Compress with deflate
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&original_data).unwrap();
        let compressed = encoder.finish().unwrap();

        assert!(compressed.len() < original_data.len());

        // Decompress
        let mut decoder = DeflateDecoder::new(&compressed[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, original_data);
    }

    #[test]
    fn test_zstd_compression() {
        let original_data = b"Large vector payload ".repeat(200);

        // Compress with zstd
        let compressed = zstd::encode_all(&original_data[..], 3).unwrap();

        assert!(compressed.len() < original_data.len());

        // Decompress
        let decompressed = zstd::decode_all(&compressed[..]).unwrap();

        assert_eq!(decompressed, original_data);
    }

    #[test]
    fn test_compression_thresholds() {
        // Small data should not benefit from compression
        let small_data = b"small";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(small_data).unwrap();
        let compressed_small = encoder.finish().unwrap();

        // Compressed might be larger due to headers
        assert!(compressed_small.len() >= small_data.len());

        // Large repetitive data should compress well
        let large_data = b"repetitive data ".repeat(1000);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&large_data).unwrap();
        let compressed_large = encoder.finish().unwrap();

        // Should achieve good compression ratio
        let compression_ratio = compressed_large.len() as f64 / large_data.len() as f64;
        assert!(compression_ratio < 0.1); // Better than 90% compression
    }

    #[test]
    fn test_vector_data_compression() {
        // Simulate vector data (floats as bytes)
        let mut vector_data = Vec::new();
        for i in 0..1000 {
            let value = (i as f32) * 0.1;
            vector_data.extend_from_slice(&value.to_le_bytes());
        }

        // Test different compression algorithms
        let algorithms = vec![
            ("gzip", {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&vector_data).unwrap();
                encoder.finish().unwrap()
            }),
            ("deflate", {
                use flate2::write::DeflateEncoder;
                encoder.write_all(&vector_data).unwrap();
                encoder.finish().unwrap()
            }),
            ("zstd", zstd::encode_all(&vector_data[..], 3).unwrap()),
        ];

        debug!("Vector data compression results:");
        debug!("Original size: {} bytes", vector_data.len());

        for (name, compressed) in algorithms {
            let ratio = (1.0 - compressed.len() as f64 / vector_data.len() as f64) * 100.0;
            debug!(
                "{}: {} bytes ({:.1}% reduction)",
                name,
                compressed.len(),
                ratio
            );

            // Vector data should compress moderately (30-60%)
            assert!(ratio > 30.0 && ratio < 70.0);
        }
    }

    #[test]
    fn test_json_metadata_compression() {
        // Simulate JSON metadata
        let metadata = serde_json::json!({
            "vectors": (0..100).map(|i| {
                serde_json::json!({
                    "id": format!("vec_{}", i),
                    "metadata_info": {
                        "category": format!("category_{}", i % 10),
                        "description": format!("This is a test vector number {}", i),
                        "tags": vec!["test", "compression", "benchmark"],
                        "timestamp": 1234567890 + i
                    }
                })
            }).collect::<Vec<_>>()
        });

        let json_str = serde_json::to_string(&metadata).unwrap();
        let json_bytes = json_str.as_bytes();

        // JSON should compress very well
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(json_bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let ratio = (1.0 - compressed.len() as f64 / json_bytes.len() as f64) * 100.0;

        debug!(
            "JSON compression: {} -> {} bytes ({:.1}% reduction)",
            json_bytes.len(),
            compressed.len(),
            ratio
        );

        assert!(ratio > 70.0); // JSON should compress > 70%
    }
}
