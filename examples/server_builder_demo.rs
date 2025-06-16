// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! ProximaDB Server Builder Architecture Demo
//! 
//! This example demonstrates the new layered builder pattern that properly
//! separates concerns across different subsystems while providing a unified
//! configuration interface.

use proximadb::server::{
    ServerBuilder, 
    HardwareAcceleration, 
    IndexingAlgorithm, 
    DistanceMetric
};
use proximadb::storage::builder::StorageLayoutStrategy;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::init();

    println!("🚀 ProximaDB Server Builder Architecture Demo");
    println!("==============================================");

    // Example 1: Basic server configuration
    println!("\n📋 Example 1: Basic Server Configuration");
    let basic_server = ServerBuilder::new()
        .with_server_endpoint("localhost", 8080)
        .with_simd_acceleration()
        .with_indexing_algorithm(IndexingAlgorithm::HNSW)
        .with_distance_metric(DistanceMetric::Cosine)
        .build()
        .await?;

    println!("✅ Basic server configured:");
    println!("   - Endpoint: {}:{}", basic_server.endpoint().0, basic_server.endpoint().1);
    println!("   - Hardware: {:?}", basic_server.compute_config().acceleration);
    println!("   - Indexing: {:?}", basic_server.indexing_config().default_algorithm);
    println!("   - Distance: {:?}", basic_server.indexing_config().default_distance_metric);

    // Example 2: High-performance configuration with storage optimization
    println!("\n🏎️ Example 2: High-Performance Configuration");
    let high_perf_server = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .with_cuda_acceleration(vec![0, 1])  // Use GPU 0 and 1
        .configure_storage(|storage| {
            storage
                .with_viper_layout()                    // Use VIPER for clustered vectors
                .with_high_performance_storage_mode()   // Optimize for performance
                .with_multi_disk_data_storage(vec![     // Use multiple NVMe drives
                    "/nvme1/proximadb/data".to_string(),
                    "/nvme2/proximadb/data".to_string(),
                    "/nvme3/proximadb/data".to_string(),
                ])
                .with_wal_format(proximadb::storage::wal::manager::WalFormat::Avro)
                .with_high_data_compression()           // Compress for storage efficiency
        })
        .with_hnsw_params(32, 400)                     // High-quality HNSW index
        .with_detailed_monitoring()                     // Enable comprehensive monitoring
        .build()
        .await?;

    println!("✅ High-performance server configured:");
    println!("   - Endpoint: {}:{}", high_perf_server.endpoint().0, high_perf_server.endpoint().1);
    println!("   - Hardware: {:?}", high_perf_server.compute_config().acceleration);
    println!("   - CUDA devices: {:?}", high_perf_server.compute_config().cuda_devices);
    println!("   - Storage layout: {:?}", high_perf_server.storage_system().storage_layout());
    println!("   - Data storage URLs: {}", high_perf_server.storage_system().config().data_storage.data_urls.len());

    // Example 3: Cloud-native configuration
    println!("\n☁️ Example 3: Cloud-Native Configuration");
    let cloud_server = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .with_hardware_acceleration(HardwareAcceleration::Auto)  // Auto-detect best acceleration
        .configure_storage(|storage| {
            storage
                .with_storage_layout(StorageLayoutStrategy::Hybrid)  // Hybrid layout for cloud
                .with_s3_data_storage(vec![                          // Use S3 for storage
                    "proximadb-prod-data-us-east-1".to_string(),
                    "proximadb-prod-data-us-west-2".to_string(),
                ])
                .with_wal_format(proximadb::storage::wal::manager::WalFormat::Avro)
                .with_fast_data_compression()                        // Fast compression for cloud
                .with_storage_memory_config(8192, 4096)             // 8GB cache, 4GB pool
        })
        .with_indexing_algorithm(IndexingAlgorithm::Auto)           // Auto-select algorithm
        .with_distance_metric(DistanceMetric::Auto)                 // Auto-select distance
        .build()
        .await?;

    println!("✅ Cloud-native server configured:");
    println!("   - Hardware: {:?}", cloud_server.compute_config().acceleration);
    println!("   - Storage layout: {:?}", cloud_server.storage_system().storage_layout());
    println!("   - Auto algorithm selection: {}", cloud_server.indexing_config().enable_auto_selection);
    println!("   - Auto distance selection: {}", cloud_server.indexing_config().enable_auto_distance_selection);

    // Example 4: Development configuration
    println!("\n🛠️ Example 4: Development Configuration");
    let dev_server = ServerBuilder::new()
        .with_server_endpoint("127.0.0.1", 9090)
        .configure_storage(|storage| {
            storage
                .with_multi_disk_data_storage(vec![
                    "/tmp/proximadb/data1".to_string(),
                    "/tmp/proximadb/data2".to_string(),
                ])
                .without_data_compression()                         // No compression for dev speed
                .with_zero_copy_storage()                          // Enable zero-copy optimizations
        })
        .with_indexing_algorithm(IndexingAlgorithm::BruteForce)    // Simple algorithm for testing
        .with_detailed_monitoring()                                // Enable debugging
        .build()
        .await?;

    println!("✅ Development server configured:");
    println!("   - Endpoint: {}:{}", dev_server.endpoint().0, dev_server.endpoint().1);
    println!("   - Indexing: {:?}", dev_server.indexing_config().default_algorithm);
    println!("   - Zero-copy enabled: {}", dev_server.storage_system().storage_performance().enable_zero_copy);
    println!("   - Compression disabled: {}", !dev_server.storage_system().data_storage_config().compression.compress_vectors);

    println!("\n🎉 All server configurations demonstrated successfully!");
    println!("\n📊 Architecture Benefits:");
    println!("   ✅ Separation of Concerns: Network, storage, compute, and indexing are independently configurable");
    println!("   ✅ Layered Design: Each subsystem has its own focused builder");
    println!("   ✅ Performance: Strategy classes loaded once at startup for zero runtime overhead");
    println!("   ✅ Flexibility: Easy to mix and match configurations for different deployment scenarios");
    println!("   ✅ Extensibility: New builders can be added without affecting existing ones");

    Ok(())
}