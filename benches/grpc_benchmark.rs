//! gRPC Benchmark Client for ProximaDB
//!
//! This benchmark uses gRPC to measure true database performance
//! without HTTP/JSON overhead.

use std::time::{Duration, Instant};
use tonic::transport::Channel;

// Use the generated proto code
use proximadb::proto::proximadb_v1::{
    CollectionConfig, DeleteCollectionRequest, DistanceMetric, SearchQuery, StorageEngine,
    VectorBatchRequest, VectorRecord, VectorSearchRequest,
    collection_service_client::CollectionServiceClient, vector_service_client::VectorServiceClient,
};

/// Benchmark configuration
#[derive(Clone, Debug)]
struct BenchmarkConfig {
    server_url: String,
    collection_name: String,
    dimension: usize,
    num_vectors: usize,
    num_queries: usize,
    top_k: usize,
    batch_size: usize,
    readiness_timeout_secs: u64,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        let dimension = read_env_usize("PROXIMADB_BENCH_DIMENSION", 384);
        let num_vectors = read_env_usize("PROXIMADB_BENCH_NUM_VECTORS", 1_000);
        Self {
            server_url: read_env_string("PROXIMADB_BENCH_SERVER_URL", "http://localhost:5679"),
            collection_name: read_env_string(
                "PROXIMADB_BENCH_COLLECTION",
                &format!("benchmark_{}v_{}d", num_vectors, dimension),
            ),
            dimension,
            num_vectors,
            num_queries: read_env_usize("PROXIMADB_BENCH_NUM_QUERIES", 100),
            top_k: read_env_usize("PROXIMADB_BENCH_TOP_K", 10),
            batch_size: read_env_usize("PROXIMADB_BENCH_BATCH_SIZE", 10_000),
            readiness_timeout_secs: std::env::var("PROXIMADB_BENCH_READY_TIMEOUT_SECS")
                .ok()
                .and_then(|raw| raw.parse::<u64>().ok())
                .unwrap_or(120),
        }
    }
}

fn read_env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn read_env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(default)
}

/// Benchmark result
struct BenchmarkResult {
    operation: String,
    count: usize,
    duration_ms: u64,
    throughput: f64,
    avg_latency_us: f64,
}

impl BenchmarkResult {
    fn print(&self) {
        println!(
            "{}: {} ops in {}ms = {:.2} ops/sec ({} μs avg)",
            self.operation,
            self.count,
            self.duration_ms,
            self.throughput,
            self.avg_latency_us as u64
        );
    }
}

/// gRPC benchmark client
struct GrpcBenchmarkClient {
    collection_client: CollectionServiceClient<Channel>,
    vector_client: VectorServiceClient<Channel>,
    config: BenchmarkConfig,
}

impl GrpcBenchmarkClient {
    /// Create new gRPC benchmark client
    async fn connect(config: BenchmarkConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let collection_client = CollectionServiceClient::connect(config.server_url.clone()).await?;
        let vector_client = VectorServiceClient::connect(config.server_url.clone()).await?;

        Ok(Self {
            collection_client,
            vector_client,
            config,
        })
    }

    async fn recreate_collection(&self) -> Result<(), Box<dyn std::error::Error>> {
        let delete_request = DeleteCollectionRequest {
            collection_id: self.config.collection_name.clone(),
        };

        match self
            .collection_client
            .clone()
            .delete_collection(delete_request)
            .await
        {
            Ok(_) => {
                println!(
                    "Deleted existing collection '{}'",
                    self.config.collection_name
                );
            }
            Err(status) => {
                println!(
                    "No existing collection '{}' to delete: {}",
                    self.config.collection_name,
                    status.message()
                );
            }
        }

        let request = CollectionConfig {
            name: self.config.collection_name.clone(),
            dimension: self.config.dimension as u32,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            embedding_models: vec!["BAAI/bge-small-en-v1.5".to_string()],
            enable_dual_use_embeddings: Some(true),
            ..Default::default()
        };

        let response = self
            .collection_client
            .clone()
            .create_collection(request)
            .await?
            .into_inner();

        println!(
            "Created collection '{}' ({}D, engine=SST, model=BAAI/bge-small-en-v1.5)",
            response.id, self.config.dimension
        );

        Ok(())
    }

    /// Generate random vector
    fn generate_random_vector(dimension: usize) -> Vec<f32> {
        (0..dimension).map(|_| rand::random::<f32>()).collect()
    }

    /// Benchmark vector insertion
    async fn benchmark_vector_insert(&self) -> Result<BenchmarkResult, Box<dyn std::error::Error>> {
        println!("\n=== Vector Insert Benchmark ===");
        println!(
            "Inserting {} vectors of dimension {} in batches of {}",
            self.config.num_vectors, self.config.dimension, self.config.batch_size
        );

        let start = Instant::now();
        let mut inserted = 0usize;
        let mut batch_start = 0usize;

        while batch_start < self.config.num_vectors {
            let batch_end = std::cmp::min(
                batch_start + self.config.batch_size,
                self.config.num_vectors,
            );
            let batch_len = batch_end - batch_start;
            let mut vectors = Vec::with_capacity(batch_len);

            for i in batch_start..batch_end {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert(
                    "index".to_string(),
                    proximadb::proto::proximadb_v1::SqlValue {
                        value: Some(
                            proximadb::proto::proximadb_v1::sql_value::Value::Int64Value(i as i64),
                        ),
                    },
                );

                vectors.push(VectorRecord {
                    id: format!("vec_{}", i),
                    vector: Self::generate_random_vector(self.config.dimension),
                    metadata,
                    timestamp: None,
                    updated_at: None,
                    expires_at: None,
                    version: None,
                    source: None,
                });
            }

            let request = VectorBatchRequest {
                collection_id: self.config.collection_name.clone(),
                vectors,
            };

            let _response = self.vector_client.clone().vector_batch(request).await?;
            inserted += batch_len;

            println!(
                "Inserted batch {}-{} ({} / {} vectors)",
                batch_start,
                batch_end - 1,
                inserted,
                self.config.num_vectors
            );

            batch_start = batch_end;
        }

        let duration = start.elapsed();
        let duration_ms = duration.as_millis() as u64;
        let throughput = (inserted as f64 * 1000.0) / duration_ms as f64;
        let avg_latency_us = (duration_ms as f64 * 1000.0) / inserted as f64;

        Ok(BenchmarkResult {
            operation: "Vector Insert (Batch)".to_string(),
            count: inserted,
            duration_ms,
            throughput,
            avg_latency_us,
        })
    }

    async fn wait_for_search_ready(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!(
            "\nWaiting up to {} seconds for search readiness...",
            self.config.readiness_timeout_secs
        );

        let deadline = Instant::now() + Duration::from_secs(self.config.readiness_timeout_secs);
        let mut attempts = 0usize;

        while Instant::now() < deadline {
            attempts += 1;
            let query_vector = Self::generate_random_vector(self.config.dimension);
            let request = VectorSearchRequest {
                collection_id: self.config.collection_name.clone(),
                queries: vec![SearchQuery {
                    vector: query_vector,
                    filters: std::collections::HashMap::new(),
                    advanced_filter: None,
                }],
                top_k: self.config.top_k as u32,
                include_fields: None,
                search_params: None,
                distance_metric_override: None,
                search_optimization: None,
            };

            match self.vector_client.clone().vector_search(request).await {
                Ok(_) => {
                    println!("Search became ready after {} attempts", attempts);
                    return Ok(());
                }
                Err(status) => {
                    println!(
                        "Search not ready yet (attempt {}): {}",
                        attempts,
                        status.message()
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }

        Err(format!(
            "Search was not ready within {} seconds for collection '{}'",
            self.config.readiness_timeout_secs, self.config.collection_name
        )
        .into())
    }

    /// Benchmark vector search
    async fn benchmark_vector_search(&self) -> Result<BenchmarkResult, Box<dyn std::error::Error>> {
        println!(
            "\n=== Vector Search Benchmark (Top-{}) ===",
            self.config.top_k
        );
        println!("Running {} searches", self.config.num_queries);

        let start = Instant::now();

        for _ in 0..self.config.num_queries {
            let query_vector = Self::generate_random_vector(self.config.dimension);

            let search_query = SearchQuery {
                vector: query_vector,
                filters: std::collections::HashMap::new(),
                advanced_filter: None,
            };

            let request = VectorSearchRequest {
                collection_id: self.config.collection_name.clone(),
                queries: vec![search_query],
                top_k: self.config.top_k as u32,
                include_fields: None,
                search_params: None,
                distance_metric_override: None,
                search_optimization: None,
            };

            let _response = self.vector_client.clone().vector_search(request).await?;
        }

        let duration = start.elapsed();
        let duration_ms = duration.as_millis() as u64;
        let throughput = (self.config.num_queries as f64 * 1000.0) / duration_ms as f64;
        let avg_latency_us = (duration_ms as f64 * 1000.0) / self.config.num_queries as f64;

        Ok(BenchmarkResult {
            operation: format!("Vector Search (Top-{})", self.config.top_k),
            count: self.config.num_queries,
            duration_ms,
            throughput,
            avg_latency_us,
        })
    }

    /// Benchmark vector search with different K values
    #[allow(dead_code)]
    async fn benchmark_vector_search_sweep(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("\n=== Vector Search Sweep ===");

        for k in [10, 50, 100] {
            let start = Instant::now();

            for _ in 0..self.config.num_queries {
                let query_vector = Self::generate_random_vector(self.config.dimension);

                let search_query = SearchQuery {
                    vector: query_vector,
                    filters: std::collections::HashMap::new(),
                    advanced_filter: None,
                };

                let request = VectorSearchRequest {
                    collection_id: self.config.collection_name.clone(),
                    queries: vec![search_query],
                    top_k: k as u32,
                    include_fields: None,
                    search_params: None,
                    distance_metric_override: None,
                    search_optimization: None,
                };

                let _response = self.vector_client.clone().vector_search(request).await?;
            }

            let duration = start.elapsed();
            let duration_ms = duration.as_millis() as u64;
            let throughput = (self.config.num_queries as f64 * 1000.0) / duration_ms as f64;
            let avg_latency_us = (duration_ms as f64 * 1000.0) / self.config.num_queries as f64;

            BenchmarkResult {
                operation: format!("Vector Search (Top-{})", k),
                count: self.config.num_queries,
                duration_ms,
                throughput,
                avg_latency_us,
            }
            .print();
        }

        Ok(())
    }

    /// Run all benchmarks
    async fn run_all(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("╔════════════════════════════════════════════════════════════╗");
        println!("║     ProximaDB gRPC Benchmark (Release Build)               ║");
        println!("╚════════════════════════════════════════════════════════════╝");
        println!("Server: {}", self.config.server_url);
        println!("Collection: {}", self.config.collection_name);
        println!("Vectors: {}", self.config.num_vectors);
        println!("Queries: {}", self.config.num_queries);
        println!("Dimension: {}", self.config.dimension);
        println!("Batch size: {}", self.config.batch_size);
        println!("Embedding baseline: BAAI/bge-small-en-v1.5 (384D)");

        self.recreate_collection().await?;

        // Insert benchmark
        let insert_result = self.benchmark_vector_insert().await?;
        insert_result.print();

        self.wait_for_search_ready().await?;

        let search_result = self.benchmark_vector_search().await?;
        search_result.print();

        println!("\n=== Benchmark Complete ===");
        println!(
            "\n🎯 Key Result: Vector Insert achieved {} ops/sec",
            insert_result.throughput
        );
        println!("🎯 This is the TRUE database performance without HTTP overhead!");

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BenchmarkConfig::default();

    println!(
        "Connecting to ProximaDB gRPC server at {}...",
        config.server_url
    );

    let client = GrpcBenchmarkClient::connect(config).await?;

    client.run_all().await?;

    Ok(())
}
