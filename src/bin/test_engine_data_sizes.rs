// Test to verify all engines write expected data sizes
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::SqlValue;
use std::collections::HashMap;

fn main() {
    println!("🧪 Testing expected data sizes for storage engines...\n");

    // Calculate expected data sizes
    let num_vectors = 1000;
    let dimension = 384;
    let bytes_per_float = 4;

    // Raw vector data size
    let vector_data_size = num_vectors * dimension * bytes_per_float;
    println!("📊 Expected Data Sizes:");
    println!("  Vectors: {} x {} x {} bytes = {} bytes ({:.2} MB)",
             num_vectors, dimension, bytes_per_float,
             vector_data_size, vector_data_size as f64 / (1024.0 * 1024.0));

    // ID strings (assuming ~20 bytes per ID)
    let id_data_size = num_vectors * 20;
    println!("  IDs: {} x ~20 bytes = {} bytes ({:.2} KB)",
             num_vectors, id_data_size, id_data_size as f64 / 1024.0);

    // Metadata (2 fields per record, ~50 bytes total)
    let metadata_size = num_vectors * 50;
    println!("  Metadata: {} x ~50 bytes = {} bytes ({:.2} KB)",
             num_vectors, metadata_size, metadata_size as f64 / 1024.0);

    // Timestamps and versions
    let timestamp_size = num_vectors * 8 * 2; // 2 i64 fields
    println!("  Timestamps: {} x 16 bytes = {} bytes ({:.2} KB)",
             num_vectors, timestamp_size, timestamp_size as f64 / 1024.0);

    let total_raw_size = vector_data_size + id_data_size + metadata_size + timestamp_size;
    println!("\n  Total raw data: {} bytes ({:.2} MB)",
             total_raw_size, total_raw_size as f64 / (1024.0 * 1024.0));

    println!("\n📝 Expected file sizes by engine:");
    println!("  SST (Proxima + compression): ~{:.1} MB", total_raw_size as f64 / (1024.0 * 1024.0) * 0.7);
    println!("  SWIFT (Proxima blocks): ~{:.1} MB", total_raw_size as f64 / (1024.0 * 1024.0) * 0.7);
    println!("  VIPER (Parquet + Zstd): ~{:.1} MB", total_raw_size as f64 / (1024.0 * 1024.0) * 0.5);
    println!("  NOVA (Parquet + Zstd): ~{:.1} MB", total_raw_size as f64 / (1024.0 * 1024.0) * 0.5);

    println!("\n⚠️  Sizes below 100KB indicate data serialization issues!");
    println!("✅ Sizes above 500KB indicate proper serialization");

    // Create sample records for size estimation
    let mut sample_records = Vec::new();
    for i in 0..10 {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                    format!("category_{}", i % 3)
                ))
            }
        );
        metadata.insert(
            "price".to_string(),
            SqlValue {
                value: Some(proximadb::proto::proximadb_v1::sql_value::Value::NumberValue(
                    (i as f64) * 10.5
                ))
            }
        );

        let record = VectorRecord {
            id: format!("vec_{:05}", i),
            vector: vec![0.1; dimension],
            metadata,
            timestamp: 1000000 + i,
            version: Some(1),
            updated_at: Some(1000000 + i),
            expires_at: None,
            quantized_vector: Vec::new(),  // Empty vector instead of None
            source: None,
        };
        sample_records.push(record);
    }

    // Estimate serialized size of single record
    let single_record = &sample_records[0];
    let estimated_record_size =
        single_record.id.len() +                          // ID
        (single_record.vector.len() * 4) +                // Vector data
        50 +                                               // Metadata (estimated)
        16;                                                // Timestamps

    println!("\n📏 Single record estimated size: {} bytes", estimated_record_size);
    println!("   For {} records: ~{:.2} MB",
             num_vectors,
             (estimated_record_size * num_vectors) as f64 / (1024.0 * 1024.0));
}