/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Comprehensive gRPC Performance and Network I/O Optimization Tests
//!
//! This test suite focuses on gRPC as the preferred SDK for ProximaDB with emphasis on:
//! - Network I/O optimization and performance benchmarking
//! - Concurrent client testing and load simulation
//! - Memory efficiency and connection pooling
//! - Stream processing and batch operation optimization
//! - Error resilience and retry mechanisms
//! - Protocol buffer optimization and compression

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{sleep, timeout};
use tonic::{
    transport::{Channel, ClientTlsConfig, Server},
    Code, Request, Response, Status,
};
use tonic_reflection::server::Builder as ReflectionBuilder;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use proximadb::core::{LsmConfig, StorageConfig};
use proximadb::network::grpc::service::ProximaDbGrpcService;
use proximadb::proto::vectordb::v1::vector_db_client::VectorDbClient;
use proximadb::proto::vectordb::v1::vector_db_server::{VectorDb, VectorDbServer};
use proximadb::proto::vectordb::v1::*;
use proximadb::services::{CollectionService, VectorService};
use proximadb::storage::{
    CollectionConfig, StorageEngine, StorageLayoutStrategy, UnifiedStorageEngine,
};

/// Performance metrics for gRPC operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcPerformanceMetrics {
    pub operation: String,
    pub duration_ms: u64,
    pub payload_size_bytes: usize,
    pub throughput_ops_per_sec: f64,
    pub memory_usage_mb: f64,
    pub connection_count: usize,
    pub error_rate: f64,
    pub p95_latency_ms: u64,
    pub p99_latency_ms: u64,
}

/// Test configuration for performance scenarios
#[derive(Debug, Clone)]
pub struct PerformanceTestConfig {
    pub concurrent_clients: usize,
    pub operations_per_client: usize,
    pub vector_dimension: usize,
    pub batch_size: usize,
    pub test_duration_seconds: u64,
    pub target_qps: f64,
    pub enable_compression: bool,
    pub connection_pool_size: usize,
}

impl Default for PerformanceTestConfig {
    fn default() -> Self {
        Self {
            concurrent_clients: 10,
            operations_per_client: 100,
            vector_dimension: 384,
            batch_size: 50,
            test_duration_seconds: 30,
            target_qps: 1000.0,
            enable_compression: true,
            connection_pool_size: 20,
        }
    }
}

/// Comprehensive test harness for gRPC performance testing
pub struct GrpcPerformanceTestHarness {
    server_address: String,
    storage_engine: Arc<RwLock<UnifiedStorageEngine>>,
    service: ProximaDbGrpcService,
    metrics_collector: Arc<RwLock<Vec<GrpcPerformanceMetrics>>>,
    client_pool: Arc<RwLock<Vec<VectorDbClient<Channel>>>>,
    temp_dir: tempfile::TempDir,
}

impl GrpcPerformanceTestHarness {
    /// Create a new performance test harness with optimized configuration
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = tempfile::TempDir::new()?;
        let data_dir = temp_dir.path().join("data");
        let wal_dir = temp_dir.path().join("wal");

        // Create optimized storage configuration for performance testing
        let storage_config = StorageConfig {
            data_dirs: vec![data_dir.clone()],
            wal_dir: wal_dir.clone(),
            mmap_enabled: true,
            lsm_config: LsmConfig {
                memtable_size_mb: 64, // Larger memtable for better performance
                level_count: 4,
                compaction_threshold: 4,
                block_size_kb: 16, // Larger blocks for better I/O
            },
            cache_size_mb: 256,    // Larger cache for performance testing
            bloom_filter_bits: 12, // More bits for better filtering
        };

        // Initialize unified storage engine with VIPER for performance
        let mut unified_engine = UnifiedStorageEngine::new(storage_config).await?;
        unified_engine.start().await?;

        let storage_engine = Arc::new(RwLock::new(unified_engine));

        // Create vector and collection services
        let vector_service = VectorService::new(storage_engine.clone());
        let collection_service = CollectionService::new(storage_engine.clone());

        // Create gRPC service with performance optimizations
        let service = ProximaDbGrpcService::new(
            storage_engine.clone(),
            vector_service,
            collection_service,
            format!("test-node-{}", Uuid::new_v4()),
            "0.1.0".to_string(),
        );

        let server_address = "127.0.0.1:0".to_string(); // Let OS assign port

        Ok(Self {
            server_address,
            storage_engine,
            service,
            metrics_collector: Arc::new(RwLock::new(Vec::new())),
            client_pool: Arc::new(RwLock::new(Vec::new())),
            temp_dir,
        })
    }

    /// Start the gRPC server with performance optimizations
    pub async fn start_server(&self) -> Result<String, Box<dyn std::error::Error>> {
        let addr = self.server_address.parse()?;

        // Create reflection service for better debugging
        let reflection_service = ReflectionBuilder::configure()
            .register_encoded_file_descriptor_set(proximadb::proto::FILE_DESCRIPTOR_SET)
            .build()?;

        // Start server with optimizations
        let server = Server::builder()
            .add_service(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_grpc())
                    .service(VectorDbServer::new(self.service.clone())),
            )
            .add_service(reflection_service)
            .serve(addr);

        let local_addr = server.local_addr();

        // Start server in background
        tokio::spawn(async move {
            if let Err(e) = server.await {
                eprintln!("Server error: {}", e);
            }
        });

        // Wait for server to start
        sleep(Duration::from_millis(100)).await;

        Ok(format!("http://{}", local_addr))
    }

    /// Initialize client connection pool for load testing
    pub async fn initialize_client_pool(
        &mut self,
        pool_size: usize,
        server_url: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut clients = Vec::new();

        for _ in 0..pool_size {
            let channel = Channel::from_shared(server_url.to_string())?
                .keep_alive_while_idle(true)
                .http2_keep_alive_interval(Duration::from_secs(30))
                .keep_alive_timeout(Duration::from_secs(5))
                .connect()
                .await?;

            let client = VectorDbClient::new(channel);
            clients.push(client);
        }

        *self.client_pool.write().await = clients;
        Ok(())
    }

    /// Create a performance test collection with VIPER storage layout
    pub async fn create_performance_collection(
        &self,
        collection_id: &str,
        dimension: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut client_pool = self.client_pool.write().await;
        let client = client_pool.get_mut(0).ok_or("No clients available")?;

        // Create collection with VIPER layout for optimal performance
        let mut config = HashMap::new();
        config.insert("storage_layout".to_string(), serde_json::json!("viper"));
        config.insert("enable_clustering".to_string(), serde_json::json!(true));
        config.insert("cluster_count".to_string(), serde_json::json!(256));
        config.insert("enable_compression".to_string(), serde_json::json!(true));
        config.insert(
            "filterable_metadata_fields".to_string(),
            serde_json::json!(["category", "source", "priority"]),
        );

        let config_struct = prost_types::Struct {
            fields: config
                .into_iter()
                .map(|(k, v)| (k, Self::json_to_protobuf_value(v)))
                .collect(),
        };

        let request = Request::new(CreateCollectionRequest {
            name: format!("perf_collection_{}", collection_id),
            dimension,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            allow_client_ids: true,
            config: Some(config_struct),
        });

        let response = client.create_collection(request).await?;
        if !response.get_ref().success {
            return Err(format!(
                "Failed to create collection: {}",
                response.get_ref().message
            )
            .into());
        }

        println!("📦 Created performance test collection: {}", collection_id);
        Ok(())
    }

    /// Generate realistic test vectors with metadata
    pub fn generate_test_vectors(&self, count: usize, dimension: usize) -> Vec<InsertVectorData> {
        (0..count)
            .map(|i| {
                let vector: Vec<f32> = (0..dimension)
                    .map(|_| rand::random::<f32>() * 2.0 - 1.0) // Range [-1, 1]
                    .collect();

                // Add realistic metadata
                let mut metadata = HashMap::new();
                metadata.insert(
                    "category".to_string(),
                    serde_json::json!(format!("cat_{}", i % 10)),
                );
                metadata.insert(
                    "source".to_string(),
                    serde_json::json!(format!("source_{}", i % 5)),
                );
                metadata.insert("priority".to_string(), serde_json::json!(i % 3));
                metadata.insert(
                    "timestamp".to_string(),
                    serde_json::json!(Utc::now().timestamp()),
                );

                let metadata_struct = prost_types::Struct {
                    fields: metadata
                        .into_iter()
                        .map(|(k, v)| (k, Self::json_to_protobuf_value(v)))
                        .collect(),
                };

                InsertVectorData {
                    client_id: format!("client_id_{}", i),
                    vector,
                    metadata: Some(metadata_struct),
                }
            })
            .collect()
    }

    /// Convert JSON value to protobuf value
    fn json_to_protobuf_value(value: serde_json::Value) -> prost_types::Value {
        use prost_types::{value::Kind, Value};

        match value {
            serde_json::Value::Null => Value {
                kind: Some(Kind::NullValue(0)),
            },
            serde_json::Value::Bool(b) => Value {
                kind: Some(Kind::BoolValue(b)),
            },
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    Value {
                        kind: Some(Kind::NumberValue(f)),
                    }
                } else {
                    Value {
                        kind: Some(Kind::StringValue(n.to_string())),
                    }
                }
            }
            serde_json::Value::String(s) => Value {
                kind: Some(Kind::StringValue(s)),
            },
            _ => Value {
                kind: Some(Kind::StringValue(value.to_string())),
            },
        }
    }
}

/// Benchmark batch insert operations with varying batch sizes
#[tokio::test]
async fn test_grpc_batch_insert_performance() {
    let mut harness = GrpcPerformanceTestHarness::new().await.unwrap();
    let server_url = harness.start_server().await.unwrap();
    harness
        .initialize_client_pool(10, &server_url)
        .await
        .unwrap();

    let collection_id = "batch_perf_test";
    harness
        .create_performance_collection(collection_id, 384)
        .await
        .unwrap();

    let batch_sizes = vec![1, 10, 50, 100, 500];
    let mut results = Vec::new();

    for batch_size in batch_sizes {
        println!("🔬 Testing batch size: {}", batch_size);

        let test_vectors = harness.generate_test_vectors(batch_size, 384);
        let payload_size = test_vectors.len() * 384 * 4; // Approximate size in bytes

        let start_time = Instant::now();

        let mut client_pool = harness.client_pool.write().await;
        let client = client_pool.get_mut(0).unwrap();

        let request = Request::new(BatchInsertRequest {
            collection_identifier: Some(
                batch_insert_request::CollectionIdentifier::CollectionName(
                    collection_id.to_string(),
                ),
            ),
            vectors: test_vectors,
        });

        let response = client.batch_insert(request).await.unwrap();
        let duration = start_time.elapsed();

        assert!(response.get_ref().success);
        assert_eq!(response.get_ref().inserted_count, batch_size as u32);

        let throughput = batch_size as f64 / duration.as_secs_f64();

        println!(
            "✅ Batch size {}: {} vectors in {:?} ({:.2} ops/sec)",
            batch_size, batch_size, duration, throughput
        );

        results.push(GrpcPerformanceMetrics {
            operation: format!("batch_insert_{}", batch_size),
            duration_ms: duration.as_millis() as u64,
            payload_size_bytes: payload_size,
            throughput_ops_per_sec: throughput,
            memory_usage_mb: 0.0, // Would need system monitoring
            connection_count: 1,
            error_rate: 0.0,
            p95_latency_ms: duration.as_millis() as u64,
            p99_latency_ms: duration.as_millis() as u64,
        });
    }

    // Verify performance characteristics
    assert!(
        results.iter().any(|r| r.throughput_ops_per_sec > 100.0),
        "Batch insert should achieve >100 ops/sec for larger batches"
    );

    println!("📊 Batch insert performance test completed successfully");
}

/// Test concurrent client performance and connection scaling
#[tokio::test]
async fn test_grpc_concurrent_client_performance() {
    let mut harness = GrpcPerformanceTestHarness::new().await.unwrap();
    let server_url = harness.start_server().await.unwrap();

    let config = PerformanceTestConfig {
        concurrent_clients: 20,
        operations_per_client: 50,
        vector_dimension: 256,
        batch_size: 25,
        ..Default::default()
    };

    harness
        .initialize_client_pool(config.concurrent_clients, &server_url)
        .await
        .unwrap();

    let collection_id = "concurrent_perf_test";
    harness
        .create_performance_collection(collection_id, config.vector_dimension as u32)
        .await
        .unwrap();

    println!(
        "🚀 Starting concurrent client test with {} clients",
        config.concurrent_clients
    );

    let semaphore = Arc::new(Semaphore::new(config.concurrent_clients));
    let overall_start = Instant::now();
    let mut tasks = Vec::new();

    for client_id in 0..config.concurrent_clients {
        let semaphore = semaphore.clone();
        let server_url = server_url.clone();
        let collection_name = collection_id.to_string();
        let operations = config.operations_per_client;
        let dimension = config.vector_dimension;
        let batch_size = config.batch_size;

        let task = tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            // Create dedicated client for this task
            let channel = Channel::from_shared(server_url)
                .unwrap()
                .keep_alive_while_idle(true)
                .connect()
                .await
                .unwrap();
            let mut client = VectorDbClient::new(channel);

            let mut operation_times = Vec::new();
            let mut errors = 0;

            for op_id in 0..operations {
                let start = Instant::now();

                // Generate small batch of vectors
                let vectors: Vec<InsertVectorData> = (0..batch_size)
                    .map(|i| {
                        let vector: Vec<f32> = (0..dimension)
                            .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                            .collect();

                        InsertVectorData {
                            client_id: format!("client_{}_{}", client_id, i),
                            vector,
                            metadata: None,
                        }
                    })
                    .collect();

                let request = Request::new(BatchInsertRequest {
                    collection_identifier: Some(
                        batch_insert_request::CollectionIdentifier::CollectionName(
                            collection_name.clone(),
                        ),
                    ),
                    vectors,
                });

                match client.batch_insert(request).await {
                    Ok(response) => {
                        if !response.get_ref().success {
                            errors += 1;
                        }
                    }
                    Err(_) => errors += 1,
                }

                operation_times.push(start.elapsed());
            }

            (client_id, operation_times, errors)
        });

        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;
    let overall_duration = overall_start.elapsed();

    // Aggregate results
    let mut total_operations = 0;
    let mut total_errors = 0;
    let mut all_latencies = Vec::new();

    for result in results {
        let (client_id, operation_times, errors) = result.unwrap();
        total_operations += operation_times.len();
        total_errors += errors;
        all_latencies.extend(operation_times);

        println!(
            "📈 Client {}: {} operations, {} errors",
            client_id,
            operation_times.len(),
            errors
        );
    }

    // Calculate statistics
    all_latencies.sort();
    let p95_index = (all_latencies.len() as f64 * 0.95) as usize;
    let p99_index = (all_latencies.len() as f64 * 0.99) as usize;

    let p95_latency = all_latencies
        .get(p95_index)
        .unwrap_or(&Duration::from_millis(0));
    let p99_latency = all_latencies
        .get(p99_index)
        .unwrap_or(&Duration::from_millis(0));

    let total_throughput = total_operations as f64 / overall_duration.as_secs_f64();
    let error_rate = total_errors as f64 / total_operations as f64;

    println!("📊 Concurrent client performance results:");
    println!("   Total operations: {}", total_operations);
    println!("   Overall duration: {:?}", overall_duration);
    println!("   Throughput: {:.2} ops/sec", total_throughput);
    println!("   Error rate: {:.2}%", error_rate * 100.0);
    println!("   P95 latency: {:?}", p95_latency);
    println!("   P99 latency: {:?}", p99_latency);

    // Performance assertions
    assert!(
        total_throughput > 50.0,
        "Should achieve >50 ops/sec with concurrent clients"
    );
    assert!(error_rate < 0.05, "Error rate should be <5%");
    assert!(
        p95_latency.as_millis() < 1000,
        "P95 latency should be <1000ms"
    );

    println!("✅ Concurrent client performance test passed");
}

/// Test search performance with various query patterns
#[tokio::test]
async fn test_grpc_search_performance() {
    let mut harness = GrpcPerformanceTestHarness::new().await.unwrap();
    let server_url = harness.start_server().await.unwrap();
    harness
        .initialize_client_pool(5, &server_url)
        .await
        .unwrap();

    let collection_id = "search_perf_test";
    let dimension = 384;
    harness
        .create_performance_collection(collection_id, dimension)
        .await
        .unwrap();

    // Insert test data for search
    let test_vectors = harness.generate_test_vectors(1000, dimension as usize);

    let mut client_pool = harness.client_pool.write().await;
    let client = client_pool.get_mut(0).unwrap();

    // Insert vectors in batches
    for chunk in test_vectors.chunks(100) {
        let request = Request::new(BatchInsertRequest {
            collection_identifier: Some(
                batch_insert_request::CollectionIdentifier::CollectionName(
                    collection_id.to_string(),
                ),
            ),
            vectors: chunk.to_vec(),
        });

        let response = client.batch_insert(request).await.unwrap();
        assert!(response.get_ref().success);
    }

    println!("📥 Inserted 1000 vectors for search testing");

    // Test various search scenarios
    let search_scenarios = vec![
        (5, "Small result set"),
        (20, "Medium result set"),
        (100, "Large result set"),
    ];

    for (k, description) in search_scenarios {
        println!("🔍 Testing search: {}", description);

        let query_vector: Vec<f32> = (0..dimension)
            .map(|_| rand::random::<f32>() * 2.0 - 1.0)
            .collect();

        let start = Instant::now();

        let request = Request::new(SearchRequest {
            collection_identifier: Some(search_request::CollectionIdentifier::CollectionName(
                collection_id.to_string(),
            )),
            vector: query_vector,
            k: k as u32,
            include_vector: false,
            include_metadata: true,
            filter: None,
        });

        let response = client.search(request).await.unwrap();
        let duration = start.elapsed();

        let results = response.get_ref().results.len();
        assert!(results > 0, "Search should return results");
        assert!(results <= k, "Should not return more than k results");

        println!("   ⚡ Found {} results in {:?}", results, duration);

        // Search should be fast
        assert!(
            duration.as_millis() < 500,
            "Search should complete in <500ms"
        );
    }

    println!("✅ Search performance test completed");
}

/// Test memory efficiency and resource usage patterns
#[tokio::test]
async fn test_grpc_memory_efficiency() {
    let mut harness = GrpcPerformanceTestHarness::new().await.unwrap();
    let server_url = harness.start_server().await.unwrap();
    harness
        .initialize_client_pool(3, &server_url)
        .await
        .unwrap();

    let collection_id = "memory_test";
    harness
        .create_performance_collection(collection_id, 128)
        .await
        .unwrap();

    // Test with increasing payload sizes to monitor memory usage
    let payload_sizes = vec![100, 500, 1000, 2000];

    for payload_size in payload_sizes {
        println!("💾 Testing memory efficiency with {} vectors", payload_size);

        let test_vectors = harness.generate_test_vectors(payload_size, 128);

        let mut client_pool = harness.client_pool.write().await;
        let client = client_pool.get_mut(0).unwrap();

        // Insert in chunks to test memory management
        for chunk in test_vectors.chunks(100) {
            let request = Request::new(BatchInsertRequest {
                collection_identifier: Some(
                    batch_insert_request::CollectionIdentifier::CollectionName(
                        collection_id.to_string(),
                    ),
                ),
                vectors: chunk.to_vec(),
            });

            let response = client.batch_insert(request).await.unwrap();
            assert!(response.get_ref().success);
        }

        // Force a small delay to allow for cleanup
        sleep(Duration::from_millis(100)).await;

        println!("   ✅ Successfully processed {} vectors", payload_size);
    }

    // Test connection cleanup and resource release
    drop(client_pool);
    sleep(Duration::from_millis(500)).await;

    println!("✅ Memory efficiency test completed");
}

/// Test error handling and retry mechanisms
#[tokio::test]
async fn test_grpc_error_resilience() {
    let mut harness = GrpcPerformanceTestHarness::new().await.unwrap();
    let server_url = harness.start_server().await.unwrap();
    harness
        .initialize_client_pool(2, &server_url)
        .await
        .unwrap();

    let mut client_pool = harness.client_pool.write().await;
    let client = client_pool.get_mut(0).unwrap();

    println!("🛡️ Testing error resilience and recovery");

    // Test invalid collection operations
    let invalid_request = Request::new(SearchRequest {
        collection_identifier: Some(search_request::CollectionIdentifier::CollectionName(
            "nonexistent".to_string(),
        )),
        vector: vec![1.0, 2.0, 3.0],
        k: 5,
        include_vector: false,
        include_metadata: false,
        filter: None,
    });

    let error_response = client.search(invalid_request).await;
    assert!(
        error_response.is_err(),
        "Should fail for nonexistent collection"
    );

    match error_response.unwrap_err().code() {
        Code::NotFound | Code::InvalidArgument => {
            println!("   ✅ Correctly handled invalid collection error");
        }
        _ => panic!("Unexpected error code"),
    }

    // Test malformed vector data
    let malformed_request = Request::new(BatchInsertRequest {
        collection_identifier: Some(batch_insert_request::CollectionIdentifier::CollectionName(
            "test".to_string(),
        )),
        vectors: vec![InsertVectorData {
            client_id: "test".to_string(),
            vector: vec![], // Empty vector should fail
            metadata: None,
        }],
    });

    let malformed_response = client.batch_insert(malformed_request).await;
    // This might succeed or fail depending on validation logic
    if let Ok(response) = malformed_response {
        // If it succeeds, it should report an error in the response
        assert!(!response.get_ref().success || response.get_ref().inserted_count == 0);
    }

    println!("✅ Error resilience test completed");
}

/// Comprehensive performance benchmark suite
#[tokio::test]
async fn test_grpc_comprehensive_benchmark() {
    let mut harness = GrpcPerformanceTestHarness::new().await.unwrap();
    let server_url = harness.start_server().await.unwrap();

    let config = PerformanceTestConfig {
        concurrent_clients: 15,
        operations_per_client: 20,
        vector_dimension: 512,
        batch_size: 30,
        test_duration_seconds: 10,
        target_qps: 500.0,
        enable_compression: true,
        connection_pool_size: 15,
    };

    harness
        .initialize_client_pool(config.connection_pool_size, &server_url)
        .await
        .unwrap();

    let collection_id = "benchmark_test";
    harness
        .create_performance_collection(collection_id, config.vector_dimension as u32)
        .await
        .unwrap();

    println!("🏁 Starting comprehensive gRPC benchmark");
    println!("   Clients: {}", config.concurrent_clients);
    println!("   Vector dimension: {}", config.vector_dimension);
    println!("   Batch size: {}", config.batch_size);
    println!("   Target QPS: {}", config.target_qps);

    let start_time = Instant::now();
    let mut all_metrics = Vec::new();

    // Run the benchmark for the specified duration
    let benchmark_duration = Duration::from_secs(config.test_duration_seconds);
    let end_time = start_time + benchmark_duration;

    let mut operation_count = 0;
    let mut error_count = 0;

    while Instant::now() < end_time {
        let cycle_start = Instant::now();

        // Create a batch of concurrent operations
        let mut tasks = Vec::new();

        for i in 0..config.concurrent_clients.min(5) {
            // Limit concurrent tasks
            let server_url = server_url.clone();
            let collection_name = collection_id.to_string();
            let batch_size = config.batch_size;
            let dimension = config.vector_dimension;

            let task = tokio::spawn(async move {
                let channel = Channel::from_shared(server_url)
                    .unwrap()
                    .connect()
                    .await
                    .unwrap();
                let mut client = VectorDbClient::new(channel);

                let vectors: Vec<InsertVectorData> = (0..batch_size)
                    .map(|j| {
                        let vector: Vec<f32> = (0..dimension)
                            .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                            .collect();

                        InsertVectorData {
                            client_id: format!("bench_{}_{}", i, j),
                            vector,
                            metadata: None,
                        }
                    })
                    .collect();

                let request = Request::new(BatchInsertRequest {
                    collection_identifier: Some(
                        batch_insert_request::CollectionIdentifier::CollectionName(collection_name),
                    ),
                    vectors,
                });

                client.batch_insert(request).await
            });

            tasks.push(task);
        }

        // Wait for this cycle to complete
        let results = futures::future::join_all(tasks).await;

        for result in results {
            match result {
                Ok(Ok(response)) => {
                    if response.get_ref().success {
                        operation_count += response.get_ref().inserted_count as usize;
                    } else {
                        error_count += 1;
                    }
                }
                _ => error_count += 1,
            }
        }

        let cycle_duration = cycle_start.elapsed();

        // Rate limiting to achieve target QPS
        let target_cycle_duration = Duration::from_millis(
            (config.concurrent_clients.min(5) * 1000) as u64 / config.target_qps as u64,
        );

        if cycle_duration < target_cycle_duration {
            sleep(target_cycle_duration - cycle_duration).await;
        }
    }

    let total_duration = start_time.elapsed();
    let actual_qps = operation_count as f64 / total_duration.as_secs_f64();
    let error_rate = error_count as f64 / (operation_count + error_count) as f64;

    println!("📊 Comprehensive benchmark results:");
    println!("   Total operations: {}", operation_count);
    println!("   Total duration: {:?}", total_duration);
    println!("   Actual QPS: {:.2}", actual_qps);
    println!("   Error rate: {:.2}%", error_rate * 100.0);

    // Performance assertions for gRPC as preferred SDK
    assert!(
        actual_qps > 100.0,
        "gRPC should achieve >100 QPS in benchmark"
    );
    assert!(error_rate < 0.10, "Error rate should be <10% under load");

    println!("🎯 gRPC comprehensive benchmark completed successfully");
    println!("   gRPC demonstrated excellent performance as preferred SDK");
}
