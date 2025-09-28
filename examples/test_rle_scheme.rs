use proximadb::storage::engines::core::ops::proximaencoder::*;

fn main() {
    println!("Testing RLE Scheme Selection\n");
    
    // Test 1: Sparse data with long runs of zeros
    let mut sparse = vec![0.0f32; 1000];
    sparse[100] = 1.0;
    sparse[500] = 2.0;
    sparse[900] = 3.0;
    
    let scheme = analyze_and_choose_scheme_f32(&sparse);
    println!("Sparse data (997 zeros, 3 non-zeros): {:?}", scheme);
    
    // Test 2: Constant data
    let constant = vec![42.0f32; 1000];
    let scheme2 = analyze_and_choose_scheme_f32(&constant);
    println!("Constant data (all 42s): {:?}", scheme2);
    
    // Test 3: Sequential data
    let sequential: Vec<f32> = (0..1000).map(|i| i as f32).collect();
    let scheme3 = analyze_and_choose_scheme_f32(&sequential);
    println!("Sequential data (0..1000): {:?}", scheme3);
    
    // Test encoding sizes
    println!("\nEncoding sizes:");
    
    let encoder1 = ProximaEncoder::new(scheme.clone());
    let encoded1 = encoder1.encode_f32(&sparse, None).unwrap();
    println!("Sparse: {} floats -> {} bytes (compression: {:.1}x)", 
             sparse.len(), encoded1.len(), 
             (sparse.len() * 4) as f64 / encoded1.len() as f64);
    
    let encoder2 = ProximaEncoder::new(scheme2.clone());
    let encoded2 = encoder2.encode_f32(&constant, None).unwrap();
    println!("Constant: {} floats -> {} bytes (compression: {:.1}x)", 
             constant.len(), encoded2.len(),
             (constant.len() * 4) as f64 / encoded2.len() as f64);
    
    let encoder3 = ProximaEncoder::new(scheme3.clone());
    let encoded3 = encoder3.encode_f32(&sequential, None).unwrap();
    println!("Sequential: {} floats -> {} bytes (compression: {:.1}x)", 
             sequential.len(), encoded3.len(),
             (sequential.len() * 4) as f64 / encoded3.len() as f64);
}
