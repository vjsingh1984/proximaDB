use std::path::Path;
use std::env;

// Simple test to check what our implementation returns
fn main() {
    let test_data = b"123456789";
    println!("Testing with: {:?}", std::str::from_utf8(test_data).unwrap());
    
    // Add the project directory to search path  
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    println!("Project dir: {}", manifest_dir);
}
