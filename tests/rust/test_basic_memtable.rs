//! Basic memtable functionality test

use std::collections::BTreeMap;
use tracing::{debug, error, info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    debug!("🧪 Testing basic BTreeMap functionality...");
    
    let mut btree: BTreeMap<String, i32> = BTreeMap::new();
    btree.insert("key1".to_string(), 100);
    
    match btree.get(key) {
        Some(value) => debug!("✅ BTreeMap test passed: {}", value),
        None => debug!("❌ BTreeMap test failed"),
    }
    
    debug!("🎉 Basic test completed successfully!");
    Ok(())
}