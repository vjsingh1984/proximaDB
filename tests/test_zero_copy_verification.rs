//! Test to verify protobuf serialization operations

use prost::Message;
use proximadb::proto::proximadb::{MetadataItem, VectorRecord, metadata_item};
use tracing::{debug, info};

#[test]
fn test_protobuf_serialization() {
    // Initialize hardware capabilities for test
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Create metadata items using the protobuf structure
    let metadata = vec![
        MetadataItem {
            key: "category".to_string(),
            value: Some(metadata_item::Value::StringValue("test".to_string())),
        },
        MetadataItem {
            key: "score".to_string(),
            value: Some(metadata_item::Value::NumberValue(0.95)),
        },
    ];

    let record = VectorRecord {
        id: "test_id".to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4, 0.5],
        metadata,
        timestamp: chrono::Utc::now().timestamp() as u32,
        updated_at: None,
        expires_at: None,
        version: None,
        quantized_vector: None,
        source: None,
    };

    // Test protobuf serialization
    let start = std::time::Instant::now();
    let serialized = record.encode_to_vec();
    let serialization_time = start.elapsed();

    info!("✅ Serialization time: {:?}", serialization_time);
    info!("✅ Serialized size: {} bytes", serialized.len());

    // Test protobuf deserialization
    let start = std::time::Instant::now();
    let deserialized =
        VectorRecord::decode(&serialized[..]).expect("Deserialization should succeed");
    let deserialization_time = start.elapsed();

    info!("✅ Deserialization time: {:?}", deserialization_time);

    // Verify the data is identical
    assert_eq!(record.id, deserialized.id);
    assert_eq!(record.vector, deserialized.vector);
    assert_eq!(record.metadata, deserialized.metadata);
    assert_eq!(record.timestamp, deserialized.timestamp);

    info!("✅ Protobuf round-trip verified!");
}

#[test]
fn test_batch_serialization_performance() {
    // Initialize hardware capabilities for test
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut records = Vec::new();

    // Create 1000 test records
    for i in 0..1000 {
        let metadata = vec![MetadataItem {
            key: "index".to_string(),
            value: Some(metadata_item::Value::StringValue(i.to_string())),
        }];

        let record = VectorRecord {
            id: format!("test_id_{}", i),
            vector: vec![0.1; 768], // 768-dimensional vector (BERT-base size)
            metadata,
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: None,
            expires_at: None,
            version: None,
            quantized_vector: None,
            source: None,
        };
        records.push(record);
    }

    // Measure batch serialization
    let start = std::time::Instant::now();
    let mut total_size = 0;

    for record in &records {
        let serialized = record.encode_to_vec();
        total_size += serialized.len();
    }

    let batch_time = start.elapsed();
    let avg_time_per_record = batch_time / 1000;

    info!("📊 Batch serialization results:");
    debug!("   Total records: 1000");
    debug!("   Total time: {:?}", batch_time);
    debug!("   Average time per record: {:?}", avg_time_per_record);
    debug!(
        "   Total serialized size: {} MB",
        total_size as f64 / 1_048_576.0
    );
    debug!(
        "   Average size per record: {} KB",
        total_size as f64 / 1000.0 / 1024.0
    );

    // Performance assertions - more reasonable expectation for protobuf
    assert!(
        avg_time_per_record.as_micros() < 10000,
        "Serialization should be under 10ms per record"
    );
}

fn main() {
    debug!("🚀 Running protobuf serialization tests...\n");
    test_protobuf_serialization();
    debug!("\n");
    test_batch_serialization_performance();
}
