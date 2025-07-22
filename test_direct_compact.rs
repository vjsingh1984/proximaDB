use proximadb::storage::engines::viper::compaction::CompactionManager;
use std::sync::Arc;
use std::collections::HashMap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Testing direct compaction logic...");
    
    // Test data
    let mut records = Vec::new();
    let mut row = HashMap::new();
    row.insert("id".to_string(), serde_json::json!("test_vec"));
    row.insert("version".to_string(), serde_json::json!(1));
    records.push(row);
    
    println!("Created {} test records", records.len());
    
    // This is just to check if the module is accessible
    println!("CompactionManager type exists: {}", std::any::type_name::<CompactionManager>());
    
    Ok(())
}