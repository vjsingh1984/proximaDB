use proximadb::storage::engines::core::ops::fastlanes_encoding::{FastLanesEncoder, FastLanesDecoder, FastLanesScheme};

fn main() {
    println!("🔍 Testing FastLanes Fix with count parameter");
    println!("=============================================");

    // Test simple case: [0, 1]
    let data = vec![0i64, 1i64];
    println!("Original data: {:?}", data);

    // Encode
    let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { base: 0 });
    let encoded = encoder.encode_integers(&data, None).unwrap();  // No expected count, will store it
    println!("Encoded: {} bytes", encoded.len());

    // Decode WITH correct count
    let decoder = FastLanesDecoder::new_from_data(&encoded);

    // Test 1: decode_integers with correct count
    match decoder.decode_integers(&encoded, Some(2)) {
        Ok(decoded) => {
            println!("✅ decode_integers(count=2): {:?}", decoded);
            if decoded == data {
                println!("   Perfect match!");
            } else {
                println!("   ❌ Mismatch!");
            }
        }
        Err(e) => println!("❌ decode_integers failed: {}", e),
    }

    // Test 2: What does decode_i64 do?
    // First, let's add the i64 marker
    let mut i64_encoded = vec![0x82]; // i64 marker
    i64_encoded.extend(&encoded);

    match decoder.decode_i64(&i64_encoded, None) {
        Ok(decoded) => {
            println!("decode_i64 (buggy): {:?} ({} elements)", decoded, decoded.len());
        }
        Err(e) => println!("decode_i64 failed: {}", e),
    }

    // Test 3: encode_i64 vs encode_integers
    println!("\n--- Testing encode_i64 vs encode_integers ---");
    match encoder.encode_i64(&data, None) {
        Ok(encoded_i64) => {
            println!("encode_i64 result: {} bytes", encoded_i64.len());
            println!("First bytes: {:?}", &encoded_i64[..std::cmp::min(10, encoded_i64.len())]);

            // Try to decode
            let decoder2 = FastLanesDecoder::new_from_data(&encoded_i64);
            match decoder2.decode_i64(&encoded_i64, None) {
                Ok(decoded) => {
                    println!("decode_i64 result: {:?} ({} elements)", decoded, decoded.len());
                }
                Err(e) => println!("decode_i64 failed: {}", e),
            }
        }
        Err(e) => println!("encode_i64 failed: {}", e),
    }
}