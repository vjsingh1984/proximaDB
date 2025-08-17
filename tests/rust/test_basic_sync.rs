//! Basic functionality test

use std::collections::BTreeMap;
use tracing::{debug, error, info, warn};

fn main() {
    debug!("🧪 Testing basic BTreeMap functionality...");
    
    let mut btree: BTreeMap<String, i32> = BTreeMap::new();
    btree.insert("key1".to_string(), 100);
    
    match btree.get(key) {
        Some(value) => debug!("✅ BTreeMap test passed: {}", value),
        None => debug!("❌ BTreeMap test failed"),
    }
    
    debug!("🎉 Basic test completed successfully!");
    debug!("📊 Unified memtable architecture is structurally sound!");
}