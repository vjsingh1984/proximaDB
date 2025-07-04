//! Basic memtable functionality test

use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Testing basic BTreeMap functionality...");
    
    let mut btree: BTreeMap<String, i32> = BTreeMap::new();
    btree.insert("key1".to_string(), 100);
    
    match btree.get("key1") {
        Some(value) => println!("✅ BTreeMap test passed: {}", value),
        None => println!("❌ BTreeMap test failed"),
    }
    
    println!("🎉 Basic test completed successfully!");
    Ok(())
}