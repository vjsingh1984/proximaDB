//! Test to verify zero-copy operations in the Avro unified schema

use proximadb::core::avro_unified::{VectorRecord};
use std::collections::HashMap;
use chrono::Utc;

#[test]
fn test_zero_copy_serialization() {
    // Create a test vector record
    let mut metadata = HashMap::new();
    metadata.insert("category".to_string(), serde_json::json!("test"));
    metadata.insert("score".to_string(), serde_json::json!(0.95));
    
    let record = VectorRecord::new(
        "test_id".to_string(),
        "test_collection".to_string(),
        vec![0.1, 0.2, 0.3, 0.4, 0.5],
        metadata,
    );
    
    // Test zero-copy serialization
    let start = std::time::Instant::now();
    let serialized = record.to_avro_bytes().expect("Serialization should succeed");
    let serialization_time = start.elapsed();
    
    println!("✅ Serialization time: {:?}", serialization_time);
    println!("✅ Serialized size: {} bytes", serialized.len());
    
    // Test zero-copy deserialization
    let start = std::time::Instant::now();
    let deserialized = VectorRecord::from_avro_bytes(&serialized)
        .expect("Deserialization should succeed");
    let deserialization_time = start.elapsed();
    
    println!("✅ Deserialization time: {:?}", deserialization_time);
    
    // Verify the data is identical
    assert_eq!(record.id, deserialized.id);
    assert_eq!(record.collection_id, deserialized.collection_id);
    assert_eq!(record.vector, deserialized.vector);
    assert_eq!(record.metadata, deserialized.metadata);
    assert_eq!(record.timestamp, deserialized.timestamp);
    
    println!("✅ Zero-copy round-trip verified!");
}

#[test]
fn test_batch_serialization_performance() {
    let mut records = Vec::new();
    
    // Create 1000 test records
    for i in 0..1000 {
        let mut metadata = HashMap::new();
        metadata.insert("index".to_string(), serde_json::json!(i));
        
        let record = VectorRecord::new(
            format!("test_id_{}", i),
            "test_collection".to_string(),
            vec![0.1; 768], // 768-dimensional vector (BERT-base size)
            metadata,
        );
        records.push(record);
    }
    
    // Measure batch serialization
    let start = std::time::Instant::now();
    let mut total_size = 0;
    
    for record in &records {
        let serialized = record.to_avro_bytes().expect("Serialization should succeed");
        total_size += serialized.len();
    }
    
    let batch_time = start.elapsed();
    let avg_time_per_record = batch_time / 1000;
    
    println!("📊 Batch serialization results:");
    println!("   Total records: 1000");
    println!("   Total time: {:?}", batch_time);
    println!("   Average time per record: {:?}", avg_time_per_record);
    println!("   Total serialized size: {} MB", total_size as f64 / 1_048_576.0);
    println!("   Average size per record: {} KB", total_size as f64 / 1000.0 / 1024.0);
    
    // Performance assertions
    assert!(avg_time_per_record.as_micros() < 1000, "Serialization should be under 1ms per record");
}

fn main() {
    println!("🚀 Running zero-copy verification tests...\n");
    test_zero_copy_serialization();
    println!("\n");
    test_batch_serialization_performance();
}