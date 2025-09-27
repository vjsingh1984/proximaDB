use proximadb::storage::engines::core::ops::proxima_encoding::{ProximaEncoder, ProximaScheme};

fn main() {
    println!("🔍 Debug: Source Indices Encoding/Decoding");
    println!("==========================================");

    // Create simple test data: 2 records mapping to indices [0, 1]
    let source_indices = vec![0i64, 1i64];
    println!("Original source indices: {:?}", source_indices);

    // Encode using Proxima with Delta scheme (base 0)
    let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
    match encoder.encode_i64(&source_indices, None) {
        Ok(encoded_data) => {
            println!("✅ Encoded: {} bytes", encoded_data.len());
            println!("Encoded bytes: {:?}", &encoded_data[..std::cmp::min(20, encoded_data.len())]);

            // Decode back
            use proximadb::storage::engines::core::ops::proxima_encoding::ProximaDecoder;
            let decoder = ProximaDecoder::new_from_data(&encoded_data);
            match decoder.decode_i64(&encoded_data, None) {
                Ok(decoded) => {
                    println!("✅ Decoded: {} elements", decoded.len());
                    println!("Decoded values: {:?}", decoded);

                    if decoded == source_indices {
                        println!("🎉 Perfect round-trip!");
                    } else {
                        println!("❌ Round-trip failed!");
                        for (i, (orig, dec)) in source_indices.iter().zip(decoded.iter()).enumerate() {
                            if orig != dec {
                                println!("  Mismatch at [{}]: {} != {}", i, orig, dec);
                            }
                        }
                    }
                }
                Err(e) => println!("❌ Decode failed: {}", e),
            }
        }
        Err(e) => println!("❌ Encode failed: {}", e),
    }
}