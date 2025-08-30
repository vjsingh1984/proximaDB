#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::storage::engines::core::ops::fastlanes_encoding::{
        FastLanesEncoder, FastLanesDecoder, FastLanesScheme, FastLanesMetadata
    };
    
    // ============================================================================
    // BASIC ENCODING/DECODING TESTS
    // ============================================================================
    
    #[test]
    fn test_bitpacked_encoding_decoding() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 100.0, 200.0, 300.0];
        let encoder = FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 16 });
        
        // Encode
        let encoded = encoder.encode_f32(&data).expect("Encoding should succeed");
        assert!(encoded.len() < data.len() * 4, "Should compress data");
        
        // Decode
        let decoder = FastLanesDecoder::new(FastLanesScheme::BitPacked { bits: 16 });
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        // Verify
        assert_eq!(data.len(), decoded.len());
        for (original, decoded) in data.iter().zip(decoded.iter()) {
            assert!((original - decoded).abs() < 0.01, "Values should match");
        }
    }
    
    #[test]
    fn test_delta_encoding_decoding() {
        // Sequential data that benefits from delta encoding
        let data: Vec<f32> = (0..1000).map(|i| i as f32 * 0.1).collect();
        let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { 
            base: data[0] as i64 
        });
        
        let encoded = encoder.encode_f32(&data).expect("Encoding should succeed");
        assert!(encoded.len() < data.len() * 4, "Delta should compress sequential data well");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::Delta { 
            base: data[0] as i64 
        });
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        assert_eq!(data.len(), decoded.len());
        for (original, decoded) in data.iter().zip(decoded.iter()) {
            assert!((original - decoded).abs() < 0.001, "Delta values should match");
        }
    }
    
    #[test]
    fn test_frame_of_reference_encoding_decoding() {
        // Data with limited range
        let data: Vec<f32> = vec![100.0, 100.5, 101.0, 99.5, 100.2, 100.8, 99.9, 100.1];
        let min_val = data.iter().cloned().fold(f32::INFINITY, f32::min);
        
        let encoder = FastLanesEncoder::new(FastLanesScheme::FrameOfReference {
            reference: min_val as i64,
            bits: 8,
        });
        
        let encoded = encoder.encode_f32(&data).expect("Encoding should succeed");
        assert!(encoded.len() < data.len() * 4, "FrameOfReference should compress");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
            reference: min_val as i64,
            bits: 8,
        });
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        assert_eq!(data.len(), decoded.len());
        for (original, decoded) in data.iter().zip(decoded.iter()) {
            assert!((original - decoded).abs() < 0.1, "FrameOfReference values should match");
        }
    }
    
    #[test]
    fn test_run_length_encoding_decoding() {
        // Data with many repeated values
        let mut data = vec![1.0f32; 100];
        data.extend(vec![2.0f32; 50]);
        data.extend(vec![3.0f32; 25]);
        
        let encoder = FastLanesEncoder::new(FastLanesScheme::RunLength);
        let encoded = encoder.encode_f32(&data).expect("Encoding should succeed");
        
        // Run-length should compress repeated values very well
        assert!(encoded.len() < 100, "RunLength should compress repeated values");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::RunLength);
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        assert_eq!(data, decoded, "RunLength encoding should preserve values exactly");
    }
    
    #[test]
    fn test_dictionary_encoding_decoding() {
        // Data with limited unique values
        let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
        let mut data = Vec::new();
        for _ in 0..100 {
            for &v in &values {
                data.push(v);
            }
        }
        
        let encoder = FastLanesEncoder::new(FastLanesScheme::Dictionary);
        let encoded = encoder.encode_f32(&data).expect("Encoding should succeed");
        
        // Dictionary should compress when few unique values
        assert!(encoded.len() < data.len() * 2, "Dictionary should compress");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::Dictionary);
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        assert_eq!(data, decoded, "Dictionary encoding should preserve values");
    }
    
    #[test]
    fn test_patched_base_encoding_decoding() {
        // Data with mostly similar values and few outliers
        let mut data = vec![100.0f32; 1000];
        // Add outliers
        data[50] = 1000.0;
        data[500] = 2000.0;
        data[750] = 3000.0;
        
        let encoder = FastLanesEncoder::new(FastLanesScheme::PatchedBase {
            base: 100,
            patch_bits: 16,
        });
        
        let encoded = encoder.encode_f32(&data).expect("Encoding should succeed");
        assert!(encoded.len() < data.len() * 4, "PatchedBase should compress");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::PatchedBase {
            base: 100,
            patch_bits: 16,
        });
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        assert_eq!(data.len(), decoded.len());
        for (i, (original, decoded)) in data.iter().zip(decoded.iter()).enumerate() {
            assert!((original - decoded).abs() < 0.1, 
                "PatchedBase value at index {} should match", i);
        }
    }
    
    // ============================================================================
    // ENGINE-SPECIFIC ENCODING TESTS
    // ============================================================================
    
    #[test]
    fn test_sst_datablock_encoding() {
        use crate::storage::engines::core::formats::row_based::block_structures::RowBasedDataBlock;
        use crate::core::VectorRecord;
        
        // Create sample vectors
        let mut records = Vec::new();
        for i in 0..100 {
            let mut record = VectorRecord::default();
            record.id = Some(format!("vec_{}", i));
            record.vector = vec![i as f32; 128]; // 128-dimensional vectors
            records.push(record);
        }
        
        // Create DataBlock with encoding
        let mut block = RowBasedDataBlock::new(records.clone());
        block.encoding_marker = 0x30; // FrameOfReference
        block.encoding_metadata = Some(FastLanesMetadata {
            scheme: FastLanesScheme::FrameOfReference { 
                reference: 0, 
                bits: 16 
            },
            original_count: 100,
            compressed_size: 0,
            dimension: 128,
        });
        
        // Verify encoding marker
        assert_eq!(block.encoding_marker, 0x30);
        assert!(block.encoding_metadata.is_some());
        
        // Verify optimal encoding selection
        let marker = block.choose_optimal_encoding_marker(&records);
        assert!(marker >= 0x10 && marker <= 0x60, "Should choose FastLanes encoding");
    }
    
    #[test]
    fn test_swift_superblock_hierarchical_encoding() {
        // Test SWIFT's hierarchical encoding with SuperBlocks
        let superblock_marker = 0x81; // SWIFT SuperBlock with Delta encoding
        let child_marker = 0xFF; // Inherit from parent
        
        // Simulate 10K vectors in a SuperBlock
        let superblock_data: Vec<f32> = (0..10000)
            .map(|i| (i as f32) * 0.01)
            .collect();
        
        // Encode at SuperBlock level
        let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { base: 0 });
        let encoded = encoder.encode_f32(&superblock_data).expect("SuperBlock encoding should work");
        
        // Verify compression ratio for 10K vectors
        let original_size = superblock_data.len() * 4;
        let compressed_size = encoded.len();
        let ratio = compressed_size as f32 / original_size as f32;
        
        assert!(ratio < 0.5, "SuperBlock should achieve >50% compression, got {}", ratio);
        
        // Decode and verify
        let decoder = FastLanesDecoder::new(FastLanesScheme::Delta { base: 0 });
        let decoded = decoder.decode_f32(&encoded).expect("Decoding should succeed");
        
        assert_eq!(superblock_data.len(), decoded.len());
    }
    
    #[test]
    fn test_raptor_tensor_encoding() {
        // Test RAPTOR's tensor-optimized encoding
        const DIMENSION: usize = 768; // Typical embedding dimension
        const NUM_VECTORS: usize = 100;
        
        // Create tensor data (row-major)
        let mut tensor_data = Vec::new();
        for i in 0..NUM_VECTORS {
            for d in 0..DIMENSION {
                tensor_data.push((i * DIMENSION + d) as f32 * 0.001);
            }
        }
        
        // Transpose to column-major for SIMD optimization
        let mut columns = vec![Vec::new(); DIMENSION];
        for i in 0..NUM_VECTORS {
            for d in 0..DIMENSION {
                columns[d].push(tensor_data[i * DIMENSION + d]);
            }
        }
        
        // Encode each dimension independently
        let encoder = FastLanesEncoder::new(FastLanesScheme::FrameOfReference {
            reference: 0,
            bits: 16,
        });
        
        let mut total_encoded_size = 0;
        for column in &columns {
            let encoded = encoder.encode_f32(column).expect("Column encoding should work");
            total_encoded_size += encoded.len();
        }
        
        let original_size = tensor_data.len() * 4;
        let compression_ratio = total_encoded_size as f32 / original_size as f32;
        
        assert!(compression_ratio < 0.6, 
            "Tensor encoding should achieve good compression: {}", compression_ratio);
    }
    
    #[test]
    fn test_prism_progressive_encoding() {
        // Test PRISM's multi-resolution progressive encoding
        let data: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        
        // Binary level (1 bit per dimension)
        let binary_data: Vec<u8> = data.iter()
            .map(|&v| if v > 500.0 { 1 } else { 0 })
            .collect();
        assert_eq!(binary_data.len(), data.len());
        
        // INT8 level (8 bits per dimension)
        let int8_data: Vec<i8> = data.iter()
            .map(|&v| ((v / 1000.0) * 127.0) as i8)
            .collect();
        assert_eq!(int8_data.len(), data.len());
        
        // PQ level would be more complex, skip for basic test
        
        // FP32 level (full precision)
        let fp32_encoder = FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 32 });
        let fp32_encoded = fp32_encoder.encode_f32(&data).expect("FP32 encoding should work");
        
        // Verify progressive sizes
        let binary_size = binary_data.len() / 8; // 1 bit per value
        let int8_size = int8_data.len();
        let fp32_size = fp32_encoded.len();
        
        assert!(binary_size < int8_size, "Binary should be smallest");
        assert!(int8_size < fp32_size, "INT8 should be smaller than FP32");
    }
    
    // ============================================================================
    // QUANTIZATION COMBINATION TESTS
    // ============================================================================
    
    #[test]
    fn test_quantized_vector_encoding() {
        // Test encoding of already quantized vectors
        let original: Vec<f32> = (0..256).map(|i| i as f32).collect();
        
        // Quantize to INT8
        let scale = 255.0 / 256.0;
        let quantized: Vec<i8> = original.iter()
            .map(|&v| (v * scale) as i8)
            .collect();
        
        // Encode quantized data as f32 for compatibility
        let quantized_f32: Vec<f32> = quantized.iter()
            .map(|&v| v as f32)
            .collect();
        
        let encoder = FastLanesEncoder::new(FastLanesScheme::Dictionary);
        let encoded = encoder.encode_f32(&quantized_f32).expect("Should encode quantized data");
        
        // Dictionary should work well for quantized data (256 unique values max)
        assert!(encoded.len() < quantized_f32.len() * 2, "Should compress quantized data");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::Dictionary);
        let decoded = decoder.decode_f32(&encoded).expect("Should decode");
        
        // Convert back to INT8 and verify
        let decoded_int8: Vec<i8> = decoded.iter()
            .map(|&v| v as i8)
            .collect();
        
        assert_eq!(quantized, decoded_int8, "Quantized values should match");
    }
    
    #[test]
    fn test_pq_encoded_vectors() {
        // Test Product Quantization encoded vectors
        const SUBVECTORS: usize = 8;
        const CODEBOOK_SIZE: usize = 256;
        
        // Simulate PQ codes (8 subvectors, each with 256 possible codes)
        let pq_codes: Vec<u8> = (0..1000)
            .flat_map(|i| {
                (0..SUBVECTORS).map(|s| ((i + s) % CODEBOOK_SIZE) as u8)
            })
            .collect();
        
        // Encode PQ codes
        let pq_f32: Vec<f32> = pq_codes.iter().map(|&c| c as f32).collect();
        let encoder = FastLanesEncoder::new(FastLanesScheme::Dictionary);
        let encoded = encoder.encode_f32(&pq_f32).expect("Should encode PQ codes");
        
        // Dictionary should be perfect for PQ (exactly 256 values)
        let compression_ratio = encoded.len() as f32 / pq_f32.len() as f32;
        assert!(compression_ratio < 0.5, "PQ codes should compress well: {}", compression_ratio);
    }
    
    // ============================================================================
    // EDGE CASES AND ERROR HANDLING
    // ============================================================================
    
    #[test]
    fn test_empty_data_encoding() {
        let data: Vec<f32> = vec![];
        let encoder = FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 16 });
        
        let encoded = encoder.encode_f32(&data).expect("Should handle empty data");
        assert_eq!(encoded.len(), 0, "Empty data should produce empty encoding");
        
        let decoder = FastLanesDecoder::new(FastLanesScheme::BitPacked { bits: 16 });
        let decoded = decoder.decode_f32(&encoded).expect("Should decode empty data");
        assert_eq!(decoded.len(), 0, "Should decode to empty");
    }
    
    #[test]
    fn test_single_value_encoding() {
        let data = vec![42.0f32];
        
        // Test all encoding schemes with single value
        let schemes = vec![
            FastLanesScheme::BitPacked { bits: 16 },
            FastLanesScheme::Delta { base: 42 },
            FastLanesScheme::FrameOfReference { reference: 42, bits: 1 },
            FastLanesScheme::RunLength,
            FastLanesScheme::Dictionary,
        ];
        
        for scheme in schemes {
            let encoder = FastLanesEncoder::new(scheme.clone());
            let encoded = encoder.encode_f32(&data).expect("Should encode single value");
            
            let decoder = FastLanesDecoder::new(scheme);
            let decoded = decoder.decode_f32(&encoded).expect("Should decode single value");
            
            assert_eq!(decoded, data, "Single value should match");
        }
    }
    
    #[test]
    fn test_large_vector_encoding() {
        // Test with large vectors (typical for embeddings)
        let dimension = 1536; // GPT-3 ada embedding size
        let num_vectors = 10000;
        
        let mut data = Vec::new();
        for i in 0..num_vectors {
            for d in 0..dimension {
                data.push(((i * dimension + d) as f32).sin());
            }
        }
        
        // Test that large data can be encoded/decoded
        let encoder = FastLanesEncoder::new(FastLanesScheme::FrameOfReference {
            reference: -1,
            bits: 16,
        });
        
        let start = std::time::Instant::now();
        let encoded = encoder.encode_f32(&data).expect("Should encode large data");
        let encode_time = start.elapsed();
        
        let start = std::time::Instant::now();
        let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
            reference: -1,
            bits: 16,
        });
        let decoded = decoder.decode_f32(&encoded).expect("Should decode large data");
        let decode_time = start.elapsed();
        
        println!("Large vector encoding: {} vectors, {} dimensions", num_vectors, dimension);
        println!("  Original size: {} MB", data.len() * 4 / 1024 / 1024);
        println!("  Encoded size: {} MB", encoded.len() / 1024 / 1024);
        println!("  Compression ratio: {:.2}%", 
            (encoded.len() as f32 / (data.len() * 4) as f32) * 100.0);
        println!("  Encode time: {:?}", encode_time);
        println!("  Decode time: {:?}", decode_time);
        
        // Verify accuracy
        let mut max_error = 0.0f32;
        for (original, decoded) in data.iter().zip(decoded.iter()) {
            max_error = max_error.max((original - decoded).abs());
        }
        assert!(max_error < 0.01, "Max error should be small: {}", max_error);
    }
    
    // ============================================================================
    // MIXED ENCODING TESTS (Different encodings for different blocks)
    // ============================================================================
    
    #[test]
    fn test_mixed_block_encoding() {
        // Simulate a scenario where different blocks use different encodings
        struct EncodedBlock {
            marker: u8,
            data: Vec<u8>,
        }
        
        let blocks = vec![
            // Block 1: Sequential data (good for Delta)
            {
                let data: Vec<f32> = (0..100).map(|i| i as f32).collect();
                let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { base: 0 });
                let encoded = encoder.encode_f32(&data).unwrap();
                EncodedBlock { marker: 0x20, data: encoded }
            },
            // Block 2: Repeated values (good for RunLength)
            {
                let data = vec![1.0f32; 100];
                let encoder = FastLanesEncoder::new(FastLanesScheme::RunLength);
                let encoded = encoder.encode_f32(&data).unwrap();
                EncodedBlock { marker: 0x60, data: encoded }
            },
            // Block 3: Random values (use BitPacked)
            {
                let data: Vec<f32> = (0..100).map(|i| (i as f32).sin() * 100.0).collect();
                let encoder = FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 16 });
                let encoded = encoder.encode_f32(&data).unwrap();
                EncodedBlock { marker: 0x10, data: encoded }
            },
        ];
        
        // Decode each block based on its marker
        for block in blocks {
            let scheme = match block.marker {
                0x10 => FastLanesScheme::BitPacked { bits: 16 },
                0x20 => FastLanesScheme::Delta { base: 0 },
                0x60 => FastLanesScheme::RunLength,
                _ => panic!("Unknown marker"),
            };
            
            let decoder = FastLanesDecoder::new(scheme);
            let decoded = decoder.decode_f32(&block.data).expect("Should decode mixed blocks");
            assert!(!decoded.is_none(), "Decoded data should not be empty");
        }
    }
    
    // ============================================================================
    // PERFORMANCE AND COMPRESSION TESTS
    // ============================================================================
    
    #[test]
    fn test_compression_ratios() {
        // Test compression ratios for different data patterns
        struct TestCase {
            name: &'static str,
            data: Vec<f32>,
            scheme: FastLanesScheme,
            expected_ratio: f32, // Expected compression ratio (compressed/original)
        }
        
        let test_cases = vec![
            TestCase {
                name: "Sequential data with Delta",
                data: (0..1000).map(|i| i as f32 * 0.1).collect(),
                scheme: FastLanesScheme::Delta { base: 0 },
                expected_ratio: 0.3, // Should compress to ~30%
            },
            TestCase {
                name: "Repeated values with RunLength",
                data: vec![42.0; 1000],
                scheme: FastLanesScheme::RunLength,
                expected_ratio: 0.01, // Should compress to ~1%
            },
            TestCase {
                name: "Limited range with FrameOfReference",
                data: (0..1000).map(|i| 100.0 + (i as f32 * 0.01)).collect(),
                scheme: FastLanesScheme::FrameOfReference { reference: 100, bits: 12 },
                expected_ratio: 0.4, // Should compress to ~40%
            },
            TestCase {
                name: "Few unique values with Dictionary",
                data: (0..1000).map(|i| (i % 10) as f32).collect(),
                scheme: FastLanesScheme::Dictionary,
                expected_ratio: 0.3, // Should compress to ~30%
            },
        ];
        
        for test in test_cases {
            let encoder = FastLanesEncoder::new(test.scheme.clone());
            let encoded = encoder.encode_f32(&test.data).expect("Encoding should work");
            
            let original_size = test.data.len() * 4;
            let compressed_size = encoded.len();
            let actual_ratio = compressed_size as f32 / original_size as f32;
            
            println!("{}: {:.2}% (expected < {:.2}%)", 
                test.name, actual_ratio * 100.0, test.expected_ratio * 100.0);
            
            assert!(actual_ratio < test.expected_ratio * 1.2, 
                "{} compression ratio {:.2} worse than expected {:.2}", 
                test.name, actual_ratio, test.expected_ratio);
            
            // Verify decoding
            let decoder = FastLanesDecoder::new(test.scheme);
            let decoded = decoder.decode_f32(&encoded).expect("Decoding should work");
            assert_eq!(test.data.len(), decoded.len());
        }
    }
    
    // ============================================================================
    // INTEGRATION WITH REAL VECTOR DATA
    // ============================================================================
    
    #[test]
    fn test_real_embedding_vectors() {
        // Simulate real embedding vectors with typical characteristics
        use rand::{thread_rng, Rng};
        let mut rng = thread_rng();
        
        // Create normalized embedding vectors
        let dimension = 384; // Sentence-BERT dimension
        let num_vectors = 100;
        
        let mut vectors = Vec::new();
        for _ in 0..num_vectors {
            let mut vec: Vec<f32> = (0..dimension)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect();
            
            // Normalize to unit length (typical for embeddings)
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            for v in &mut vec {
                *v /= norm;
            }
            
            vectors.extend(vec);
        }
        
        // Test different encoding schemes
        let schemes = vec![
            ("BitPacked", FastLanesScheme::BitPacked { bits: 16 }),
            ("FrameOfReference", FastLanesScheme::FrameOfReference { reference: -1, bits: 16 }),
            ("Delta", FastLanesScheme::Delta { base: 0 }),
        ];
        
        for (name, scheme) in schemes {
            let encoder = FastLanesEncoder::new(scheme.clone());
            let encoded = encoder.encode_f32(&vectors).expect("Should encode embeddings");
            
            let decoder = FastLanesDecoder::new(scheme);
            let decoded = decoder.decode_f32(&encoded).expect("Should decode embeddings");
            
            // Calculate cosine similarity preservation
            let original_sim = cosine_similarity(&vectors[0..dimension], &vectors[dimension..dimension*2]);
            let decoded_sim = cosine_similarity(&decoded[0..dimension], &decoded[dimension..dimension*2]);
            
            let sim_error = (original_sim - decoded_sim).abs();
            println!("{} embedding test: similarity error = {}", name, sim_error);
            assert!(sim_error < 0.01, "Cosine similarity should be preserved");
        }
    }
    
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }
}