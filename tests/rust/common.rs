//! Common utilities for integration tests

use anyhow::Result;
use std::sync::Once;
use tokio::sync::Mutex;
use proximadb::core::{CollectionId};
use proximadb::schema_types::VectorRecord;
use proximadb::schema_types::{CollectionConfig, DistanceMetric, StorageEngine, IndexingAlgorithm};
use std::collections::HashMap;
use uuid::Uuid;

static INIT: Once = Once::new();
static SERVER_LOCK: Mutex<()> = Mutex::const_new(());

/// Initialize test environment
pub fn init_test_env() {
    INIT.call_once(|| {
        env_logger::builder()
            .filter_level(env_logger::LevelFilter::Info)
            .init();
    });
}

/// Generate unique collection name for testing
pub fn generate_test_collection_name() -> String {
    format!("test_collection_{}", Uuid::new_v4().simple())
}

/// Create test collection configuration
pub fn create_test_collection_config(name: String, dimension: i32) -> CollectionConfig {
    CollectionConfig {
        name,
        dimension,
        distance_metric: DistanceMetric::Cosine,
        storage_engine: StorageEngine::Viper,
        indexing_algorithm: IndexingAlgorithm::Hnsw,
        filterable_metadata_fields: vec![
            "category".to_string(),
            "author".to_string(),
            "doc_type".to_string(),
        ],
        indexing_config: HashMap::new(),
    }
}

/// Create test vector record
pub fn create_test_vector_record(
    id: String,
    _collection_id: String,
    dimension: usize,
) -> VectorRecord {
    let vector: Vec<f32> = (0..dimension)
        .map(|i| (i as f32) * 0.1)
        .collect();
    
    let mut metadata = HashMap::new();
    metadata.insert("category".to_string(), serde_json::Value::String("test".to_string()));
    metadata.insert("author".to_string(), serde_json::Value::String("test_author".to_string()));
    metadata.insert("doc_type".to_string(), serde_json::Value::String("unit_test".to_string()));
    metadata.insert("test_id".to_string(), serde_json::Value::String(id.clone()));
    
    VectorRecord {
        id: Some(id),
        vector,
        metadata: Some(metadata),
        timestamp: None, // Will be auto-generated
        version: 0,
        expires_at: None,
    }
}

/// Create batch of test vector records
pub fn create_test_vector_batch(
    collection_id: String,
    count: usize,
    dimension: usize,
) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let id = format!("test_vector_{:04}", i);
            create_test_vector_record(id, collection_id.clone(), dimension)
        })
        .collect()
}

/// Generate random vector
pub fn generate_random_vector(dimension: usize) -> Vec<f32> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..dimension).map(|_| rng.gen::<f32>()).collect()
}

/// Performance measurement utilities
pub struct PerformanceMeasurement {
    pub operation: String,
    pub duration_ms: f64,
    pub items_processed: usize,
    pub throughput: f64,
}

impl PerformanceMeasurement {
    pub fn new(operation: String, duration: std::time::Duration, items_processed: usize) -> Self {
        let duration_ms = duration.as_secs_f64() * 1000.0;
        let throughput = if duration_ms > 0.0 {
            (items_processed as f64) / (duration_ms / 1000.0)
        } else {
            0.0
        };
        
        Self {
            operation,
            duration_ms,
            items_processed,
            throughput,
        }
    }
    
    pub fn print_results(&self) {
        println!(
            "📊 {}: {:.2}ms, {} items, {:.1} ops/sec",
            self.operation,
            self.duration_ms,
            self.items_processed,
            self.throughput
        );
    }
}

/// Macro for performance measurement
#[macro_export]
macro_rules! measure_performance {
    ($operation:expr, $items:expr, $code:block) => {{
        let start = std::time::Instant::now();
        let result = $code;
        let duration = start.elapsed();
        let measurement = crate::common::PerformanceMeasurement::new(
            $operation.to_string(),
            duration,
            $items,
        );
        measurement.print_results();
        (result, measurement)
    }};
}

/// Test configuration
pub struct TestConfig {
    pub server_url: String,
    pub timeout_ms: u64,
    pub batch_size: usize,
    pub test_dimension: usize,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:5678".to_string(),
            timeout_ms: 30000,
            batch_size: 100,
            test_dimension: 384,
        }
    }
}

/// Cleanup helper for tests
pub struct TestCleanup {
    pub collections_to_delete: Vec<String>,
}

impl TestCleanup {
    pub fn new() -> Self {
        Self {
            collections_to_delete: Vec::new(),
        }
    }
    
    pub fn add_collection(&mut self, collection_id: String) {
        self.collections_to_delete.push(collection_id);
    }
}

impl Drop for TestCleanup {
    fn drop(&mut self) {
        // Note: In a real implementation, we would cleanup collections here
        for collection_id in &self.collections_to_delete {
            println!("🧹 Cleanup: {}", collection_id);
        }
    }
}