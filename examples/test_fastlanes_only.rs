use proximadb::storage::engines::core::ops::fastlanes_encoding::{FastLanesEncoder, FastLanesDecoder, FastLanesScheme};

fn main() -> anyhow::Result<()> {
    // Create test data - first 64 values
    let mut test_floats = Vec::new();
    for i in 0..64 {
        test_floats.push((i as f32) * 0.1);
    }
    
    println!("Original data (first 20):");
    for i in 0..20 {
        println!("  [{}]: {}", i, test_floats[i]);
    }
    
    // Encode using Delta scheme
    let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { base: 0 });
    let encoded = encoder.encode_f32(&test_floats)?;
    println!("\nEncoded size: {} bytes", encoded.len());
    
    // Decode
    let decoder = FastLanesDecoder::new(FastLanesScheme::Delta { base: 0 });
    let decoded = decoder.decode_f32(&encoded, test_floats.len())?;
    
    println!("\nDecoded data (first 20):");
    for i in 0..20 {
        println!("  [{}]: {} (diff: {})", i, decoded[i], (test_floats[i] - decoded[i]).abs());
    }
    
    // Check for errors
    let mut errors = 0;
    for i in 0..test_floats.len() {
        if (test_floats[i] - decoded[i]).abs() > 1e-5 {
            println!("ERROR at [{}]: orig={}, decoded={}", i, test_floats[i], decoded[i]);
            errors += 1;
        }
    }
    
    if errors == 0 {
        println!("\n✅ All values match!");
    } else {
        println!("\n❌ {} values don't match", errors);
    }
    
    Ok(())
}
