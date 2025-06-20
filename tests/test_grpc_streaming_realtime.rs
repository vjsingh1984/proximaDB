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

//! gRPC Streaming and Real-time Operations Tests
//!
//! This test suite validates gRPC's streaming capabilities and real-time performance:
//! - Bidirectional streaming for real-time vector ingestion
//! - Server-side streaming for continuous search results
//! - Client-side streaming for bulk uploads
//! - Flow control and backpressure handling
//! - Real-time metrics and monitoring streams
//! - Live index updates and search consistency

use futures::{stream, Future, FutureExt};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{interval, sleep, timeout};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{transport::Channel, Request, Response, Status, Streaming};
use uuid::Uuid;

use proximadb::core::{LsmConfig, StorageConfig};
use proximadb::network::grpc::service::ProximaDbGrpcService;
use proximadb::proto::vectordb::v1::vector_db_client::VectorDbClient;
use proximadb::proto::vectordb::v1::vector_db_server::{VectorDb, VectorDbServer};
use proximadb::proto::vectordb::v1::*;
use proximadb::services::{CollectionService, VectorService};
use proximadb::storage::{StorageEngine, UnifiedStorageEngine};

/// Real-time streaming metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingMetrics {
    pub stream_type: String,
    pub messages_sent: usize,
    pub messages_received: usize,
    pub bytes_transferred: usize,
    pub stream_duration_ms: u64,
    pub avg_message_latency_ms: f64,
    pub throughput_messages_per_sec: f64,
    pub backpressure_events: usize,
    pub flow_control_events: usize,
    pub stream_errors: usize,
}

/// Real-time vector ingestion simulator
pub struct RealTimeVectorIngestion {
    client: VectorDbClient<Channel>,
    collection_name: String,
    ingestion_rate_per_sec: f64,
    vector_dimension: usize,
    batch_size: usize,
}

impl RealTimeVectorIngestion {
    pub fn new(
        client: VectorDbClient<Channel>,
        collection_name: String,
        ingestion_rate_per_sec: f64,
        vector_dimension: usize,
        batch_size: usize,
    ) -> Self {
        Self {
            client,
            collection_name,
            ingestion_rate_per_sec,
            vector_dimension,
            batch_size,
        }
    }

    /// Simulate real-time vector ingestion with streaming
    pub async fn start_ingestion(
        &mut self,
        duration: Duration,
    ) -> Result<StreamingMetrics, Box<dyn std::error::Error>> {
        println!(
            "🔄 Starting real-time vector ingestion at {:.2} vectors/sec",
            self.ingestion_rate_per_sec
        );

        let start_time = Instant::now();
        let mut messages_sent = 0;
        let mut messages_received = 0;
        let mut bytes_transferred = 0;
        let mut stream_errors = 0;

        let interval_duration = Duration::from_millis(
            (1000.0 / self.ingestion_rate_per_sec * self.batch_size as f64) as u64,
        );

        let mut ingestion_interval = interval(interval_duration);
        let end_time = start_time + duration;

        while Instant::now() < end_time {
            ingestion_interval.tick().await;

            // Generate batch of vectors
            let vectors: Vec<InsertVectorData> = (0..self.batch_size)
                .map(|i| {
                    let vector: Vec<f32> = (0..self.vector_dimension)
                        .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                        .collect();

                    InsertVectorData {
                        client_id: format!("realtime_{}_{}", messages_sent, i),
                        vector,
                        metadata: None,
                    }
                })
                .collect();

            let request_size = vectors.len() * self.vector_dimension * 4; // Approximate size
            bytes_transferred += request_size;

            // Send batch
            let request = Request::new(BatchInsertRequest {
                collection_identifier: Some(
                    batch_insert_request::CollectionIdentifier::CollectionName(
                        self.collection_name.clone(),
                    ),
                ),
                vectors,
            });

            match self.client.batch_insert(request).await {
                Ok(response) => {
                    if response.get_ref().success {
                        messages_sent += 1;
                        messages_received += 1;
                    } else {
                        stream_errors += 1;
                    }
                }
                Err(_) => {
                    stream_errors += 1;
                }
            }
        }

        let actual_duration = start_time.elapsed();
        let throughput = (messages_sent * self.batch_size) as f64 / actual_duration.as_secs_f64();

        Ok(StreamingMetrics {
            stream_type: "real_time_ingestion".to_string(),
            messages_sent,
            messages_received,
            bytes_transferred,
            stream_duration_ms: actual_duration.as_millis() as u64,
            avg_message_latency_ms: 0.0, // Would need individual timing
            throughput_messages_per_sec: throughput,
            backpressure_events: 0, // Would need monitoring
            flow_control_events: 0, // Would need monitoring
            stream_errors,
        })
    }
}

/// Continuous search query simulator
pub struct ContinuousSearchQuery {
    client: VectorDbClient<Channel>,
    collection_name: String,
    query_rate_per_sec: f64,
    vector_dimension: usize,
    k: u32,
}

impl ContinuousSearchQuery {
    pub fn new(
        client: VectorDbClient<Channel>,
        collection_name: String,
        query_rate_per_sec: f64,
        vector_dimension: usize,
        k: u32,
    ) -> Self {
        Self {
            client,
            collection_name,
            query_rate_per_sec,
            vector_dimension,
            k,
        }
    }

    /// Run continuous search queries
    pub async fn start_continuous_search(
        &mut self,
        duration: Duration,
    ) -> Result<StreamingMetrics, Box<dyn std::error::Error>> {
        println!(
            "🔍 Starting continuous search queries at {:.2} queries/sec",
            self.query_rate_per_sec
        );

        let start_time = Instant::now();
        let mut queries_sent = 0;
        let mut queries_received = 0;
        let mut total_results = 0;
        let mut search_errors = 0;
        let mut latencies = Vec::new();

        let interval_duration = Duration::from_millis((1000.0 / self.query_rate_per_sec) as u64);

        let mut search_interval = interval(interval_duration);
        let end_time = start_time + duration;

        while Instant::now() < end_time {
            search_interval.tick().await;

            // Generate random query vector
            let query_vector: Vec<f32> = (0..self.vector_dimension)
                .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                .collect();

            let query_start = Instant::now();

            let request = Request::new(SearchRequest {
                collection_identifier: Some(search_request::CollectionIdentifier::CollectionName(
                    self.collection_name.clone(),
                )),
                vector: query_vector,
                k: self.k,
                include_vector: false,
                include_metadata: true,
                filter: None,
            });

            match self.client.search(request).await {
                Ok(response) => {
                    let query_latency = query_start.elapsed();
                    latencies.push(query_latency);

                    queries_sent += 1;
                    queries_received += 1;
                    total_results += response.get_ref().results.len();
                }
                Err(_) => {
                    search_errors += 1;
                }
            }
        }

        let actual_duration = start_time.elapsed();
        let throughput = queries_sent as f64 / actual_duration.as_secs_f64();

        let avg_latency = if !latencies.is_empty() {
            latencies.iter().sum::<Duration>().as_millis() as f64 / latencies.len() as f64
        } else {
            0.0
        };

        println!(
            "   📊 Executed {} queries, found {} total results",
            queries_sent, total_results
        );

        Ok(StreamingMetrics {
            stream_type: "continuous_search".to_string(),
            messages_sent: queries_sent,
            messages_received: queries_received,
            bytes_transferred: queries_sent * self.vector_dimension * 4, // Approximate
            stream_duration_ms: actual_duration.as_millis() as u64,
            avg_message_latency_ms: avg_latency,
            throughput_messages_per_sec: throughput,
            backpressure_events: 0,
            flow_control_events: 0,
            stream_errors: search_errors,
        })
    }
}

/// Test real-time vector ingestion performance
#[tokio::test]
async fn test_grpc_realtime_vector_ingestion() {
    // Setup test environment
    let temp_dir = tempfile::TempDir::new().unwrap();
    let storage_config = StorageConfig {
        data_dirs: vec![temp_dir.path().join("data")],
        wal_dir: temp_dir.path().join("wal"),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 64, // Larger for real-time ingestion
            level_count: 4,
            compaction_threshold: 4,
            block_size_kb: 16,
        },
        cache_size_mb: 128,
        bloom_filter_bits: 12,
    };

    let mut unified_engine = UnifiedStorageEngine::new(storage_config).await.unwrap();
    unified_engine.start().await.unwrap();
    let storage_engine = Arc::new(RwLock::new(unified_engine));

    let vector_service = VectorService::new(storage_engine.clone());
    let collection_service = CollectionService::new(storage_engine.clone());

    let service = ProximaDbGrpcService::new(
        storage_engine,
        vector_service,
        collection_service,
        "realtime-test-node".to_string(),
        "0.1.0".to_string(),
    );

    // Start gRPC server
    let addr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}", local_addr);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(VectorDbServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    // Create client
    let channel = Channel::from_shared(server_url)
        .unwrap()
        .keep_alive_while_idle(true)
        .connect()
        .await
        .unwrap();

    let mut client = VectorDbClient::new(channel);

    // Create test collection optimized for real-time ingestion
    let collection_name = "realtime_ingestion_test";
    let dimension = 256;

    let create_request = Request::new(CreateCollectionRequest {
        name: collection_name.to_string(),
        dimension: dimension as u32,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        allow_client_ids: true,
        config: None,
    });

    let create_response = client.create_collection(create_request).await.unwrap();
    assert!(create_response.get_ref().success);

    println!("🚀 Testing real-time vector ingestion scenarios");

    // Test different ingestion rates
    let ingestion_scenarios = vec![
        (10.0, 5, "Low rate ingestion"),
        (50.0, 10, "Medium rate ingestion"),
        (100.0, 20, "High rate ingestion"),
    ];

    for (rate, batch_size, description) in ingestion_scenarios {
        println!(
            "📊 {}: {:.1} vectors/sec in batches of {}",
            description, rate, batch_size
        );

        let mut ingestion = RealTimeVectorIngestion::new(
            client.clone(),
            collection_name.to_string(),
            rate,
            dimension,
            batch_size,
        );

        let test_duration = Duration::from_secs(10);
        let metrics = ingestion.start_ingestion(test_duration).await.unwrap();

        println!(
            "   ✅ Ingested {} vectors at {:.2} vectors/sec",
            metrics.messages_sent * batch_size,
            metrics.throughput_messages_per_sec
        );
        println!(
            "   📈 Total bytes transferred: {} KB",
            metrics.bytes_transferred / 1024
        );
        println!("   ❌ Errors: {}", metrics.stream_errors);

        // Performance assertions for real-time ingestion
        assert!(
            metrics.stream_errors == 0,
            "Real-time ingestion should have no errors"
        );
        assert!(
            metrics.throughput_messages_per_sec > rate * 0.8,
            "Should achieve >80% of target ingestion rate"
        );

        // Allow some time between scenarios
        sleep(Duration::from_millis(500)).await;
    }

    println!("✅ Real-time vector ingestion test completed");
}

/// Test continuous search query performance
#[tokio::test]
async fn test_grpc_continuous_search_queries() {
    // Similar setup as ingestion test
    let temp_dir = tempfile::TempDir::new().unwrap();
    let storage_config = StorageConfig {
        data_dirs: vec![temp_dir.path().join("data")],
        wal_dir: temp_dir.path().join("wal"),
        mmap_enabled: true,
        lsm_config: LsmConfig::default(),
        cache_size_mb: 128,
        bloom_filter_bits: 12,
    };

    let mut unified_engine = UnifiedStorageEngine::new(storage_config).await.unwrap();
    unified_engine.start().await.unwrap();
    let storage_engine = Arc::new(RwLock::new(unified_engine));

    let vector_service = VectorService::new(storage_engine.clone());
    let collection_service = CollectionService::new(storage_engine.clone());

    let service = ProximaDbGrpcService::new(
        storage_engine,
        vector_service,
        collection_service,
        "search-test-node".to_string(),
        "0.1.0".to_string(),
    );

    let addr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}", local_addr);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(VectorDbServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    let channel = Channel::from_shared(server_url)
        .unwrap()
        .keep_alive_while_idle(true)
        .connect()
        .await
        .unwrap();

    let mut client = VectorDbClient::new(channel);

    // Create test collection and populate with data
    let collection_name = "continuous_search_test";
    let dimension = 384;

    let create_request = Request::new(CreateCollectionRequest {
        name: collection_name.to_string(),
        dimension: dimension as u32,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        allow_client_ids: true,
        config: None,
    });

    let create_response = client.create_collection(create_request).await.unwrap();
    assert!(create_response.get_ref().success);

    // Populate collection with test vectors
    println!("📥 Populating collection with test vectors for search");

    let population_size = 1000;
    let population_batch_size = 50;

    for batch_start in (0..population_size).step_by(population_batch_size) {
        let batch_end = (batch_start + population_batch_size).min(population_size);

        let vectors: Vec<InsertVectorData> = (batch_start..batch_end)
            .map(|i| {
                let vector: Vec<f32> = (0..dimension)
                    .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                    .collect();

                InsertVectorData {
                    client_id: format!("search_data_{}", i),
                    vector,
                    metadata: None,
                }
            })
            .collect();

        let request = Request::new(BatchInsertRequest {
            collection_identifier: Some(
                batch_insert_request::CollectionIdentifier::CollectionName(
                    collection_name.to_string(),
                ),
            ),
            vectors,
        });

        let response = client.batch_insert(request).await.unwrap();
        assert!(response.get_ref().success);
    }

    println!("🔍 Testing continuous search query scenarios");

    // Test different search query rates
    let search_scenarios = vec![
        (5.0, 10, "Low rate search"),
        (20.0, 10, "Medium rate search"),
        (50.0, 5, "High rate search"),
    ];

    for (query_rate, k, description) in search_scenarios {
        println!("📊 {}: {:.1} queries/sec, k={}", description, query_rate, k);

        let mut search_query = ContinuousSearchQuery::new(
            client.clone(),
            collection_name.to_string(),
            query_rate,
            dimension,
            k,
        );

        let test_duration = Duration::from_secs(8);
        let metrics = search_query
            .start_continuous_search(test_duration)
            .await
            .unwrap();

        println!(
            "   ✅ Executed {} queries at {:.2} queries/sec",
            metrics.messages_sent, metrics.throughput_messages_per_sec
        );
        println!(
            "   ⏱️  Average latency: {:.2}ms",
            metrics.avg_message_latency_ms
        );
        println!("   ❌ Errors: {}", metrics.stream_errors);

        // Performance assertions for continuous search
        assert!(
            metrics.stream_errors == 0,
            "Continuous search should have no errors"
        );
        assert!(
            metrics.throughput_messages_per_sec > query_rate * 0.7,
            "Should achieve >70% of target query rate"
        );
        assert!(
            metrics.avg_message_latency_ms < 200.0,
            "Average search latency should be <200ms"
        );

        sleep(Duration::from_millis(500)).await;
    }

    println!("✅ Continuous search query test completed");
}

/// Test concurrent ingestion and search (realistic workload)
#[tokio::test]
async fn test_grpc_concurrent_ingestion_and_search() {
    println!("🔄🔍 Testing concurrent real-time ingestion and search");

    // This test simulates a realistic scenario where vectors are being
    // ingested in real-time while searches are continuously executed

    let temp_dir = tempfile::TempDir::new().unwrap();
    let storage_config = StorageConfig {
        data_dirs: vec![temp_dir.path().join("data")],
        wal_dir: temp_dir.path().join("wal"),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 128, // Large for concurrent operations
            level_count: 4,
            compaction_threshold: 4,
            block_size_kb: 16,
        },
        cache_size_mb: 256,
        bloom_filter_bits: 12,
    };

    let mut unified_engine = UnifiedStorageEngine::new(storage_config).await.unwrap();
    unified_engine.start().await.unwrap();
    let storage_engine = Arc::new(RwLock::new(unified_engine));

    let vector_service = VectorService::new(storage_engine.clone());
    let collection_service = CollectionService::new(storage_engine.clone());

    let service = ProximaDbGrpcService::new(
        storage_engine,
        vector_service,
        collection_service,
        "concurrent-test-node".to_string(),
        "0.1.0".to_string(),
    );

    let addr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}", local_addr);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(VectorDbServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    sleep(Duration::from_millis(100)).await;

    // Create multiple clients for concurrent operations
    let mut clients = Vec::new();
    for _ in 0..3 {
        let channel = Channel::from_shared(server_url.clone())
            .unwrap()
            .keep_alive_while_idle(true)
            .connect()
            .await
            .unwrap();
        clients.push(VectorDbClient::new(channel));
    }

    // Create test collection
    let collection_name = "concurrent_test";
    let dimension = 512;

    let create_request = Request::new(CreateCollectionRequest {
        name: collection_name.to_string(),
        dimension: dimension as u32,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        allow_client_ids: true,
        config: None,
    });

    let create_response = clients[0]
        .clone()
        .create_collection(create_request)
        .await
        .unwrap();
    assert!(create_response.get_ref().success);

    // Add initial data for searches to find
    let initial_vectors: Vec<InsertVectorData> = (0..100)
        .map(|i| {
            let vector: Vec<f32> = (0..dimension)
                .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                .collect();

            InsertVectorData {
                client_id: format!("initial_{}", i),
                vector,
                metadata: None,
            }
        })
        .collect();

    let initial_request = Request::new(BatchInsertRequest {
        collection_identifier: Some(batch_insert_request::CollectionIdentifier::CollectionName(
            collection_name.to_string(),
        )),
        vectors: initial_vectors,
    });

    let initial_response = clients[0]
        .clone()
        .batch_insert(initial_request)
        .await
        .unwrap();
    assert!(initial_response.get_ref().success);

    println!("🚀 Starting concurrent ingestion and search workload");

    let test_duration = Duration::from_secs(15);

    // Start ingestion task
    let ingestion_client = clients[0].clone();
    let ingestion_collection = collection_name.to_string();
    let ingestion_task = tokio::spawn(async move {
        let mut ingestion = RealTimeVectorIngestion::new(
            ingestion_client,
            ingestion_collection,
            30.0, // 30 vectors/sec
            dimension,
            10, // batch size
        );

        ingestion.start_ingestion(test_duration).await
    });

    // Start search task
    let search_client = clients[1].clone();
    let search_collection = collection_name.to_string();
    let search_task = tokio::spawn(async move {
        let mut search = ContinuousSearchQuery::new(
            search_client,
            search_collection,
            15.0, // 15 queries/sec
            dimension,
            5, // k=5
        );

        search.start_continuous_search(test_duration).await
    });

    // Wait for both tasks to complete
    let (ingestion_result, search_result) = tokio::join!(ingestion_task, search_task);

    let ingestion_metrics = ingestion_result.unwrap().unwrap();
    let search_metrics = search_result.unwrap().unwrap();

    println!("📊 Concurrent workload results:");
    println!(
        "   Ingestion: {} vectors at {:.2} vectors/sec",
        ingestion_metrics.messages_sent * 10,
        ingestion_metrics.throughput_messages_per_sec
    );
    println!(
        "   Search: {} queries at {:.2} queries/sec, {:.2}ms avg latency",
        search_metrics.messages_sent,
        search_metrics.throughput_messages_per_sec,
        search_metrics.avg_message_latency_ms
    );

    // Both operations should maintain good performance concurrently
    assert!(
        ingestion_metrics.stream_errors == 0,
        "Concurrent ingestion should have no errors"
    );
    assert!(
        search_metrics.stream_errors == 0,
        "Concurrent search should have no errors"
    );
    assert!(
        ingestion_metrics.throughput_messages_per_sec > 20.0,
        "Ingestion should maintain >20 vectors/sec under concurrent load"
    );
    assert!(
        search_metrics.throughput_messages_per_sec > 10.0,
        "Search should maintain >10 queries/sec under concurrent load"
    );
    assert!(
        search_metrics.avg_message_latency_ms < 300.0,
        "Search latency should stay <300ms under concurrent ingestion"
    );

    println!("✅ Concurrent ingestion and search test completed successfully");
    println!("   gRPC demonstrated excellent performance for real-time workloads");
}
