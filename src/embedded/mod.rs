//! # Embedded Mode - In-Process ProximaDB Database
//!
//! This module provides an embedded mode for using ProximaDB as an in-process
//! database without the network layer. It enables direct access to all ProximaDB
//! functionality with full multi-disk support.
//!
//! ## Architecture
//!
//! ```text
//! Application (Rust / Python / Java / Go / Node.js)
//!       |
//!       v
//! ┌─────────────────────────────────────────────────────────────┐
//! │         EmbeddedProximaDB (this module)                 │
//! │   - Direct in-process API                               │
//! │   - No network overhead                                 │
//! │   - Multi-disk configuration                            │
//! ├─────────────────────────────────────────────────────────────┤
//! │   Language Bindings (feature-gated)                     │
//! │   - Python: PyO3 bindings (feature = "python")          │
//! │   - Java: JNI bindings (feature = "java")               │
//! │   - Go: C FFI bindings (feature = "c_ffi")              │
//! │   - Node.js: NAPI-RS (feature = "nodejs")               │
//! ├─────────────────────────────────────────────────────────────┤
//! │         ProximaDB Core (Rust)                           │
//! │   - Storage Engines (SST, VIPER, NOVA, SWIFT, etc.)     │
//! │   - Multi-disk support via StorageLocation              │
//! │   - WAL persistence                                     │
//! │   - Compute (SIMD-accelerated)                          │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Features
//!
//! - **No network overhead**: Direct in-process API calls
//! - **Multi-disk support**: Configure multiple storage locations with weights
//! - **Automatic hardware acceleration**: SIMD detection and usage
//! - **Full persistence**: WAL, snapshots, and recovery
//! - **Thread-safe**: Safe for concurrent access
//!
//! ## Build Features
//!
//! Enable language-specific bindings with Cargo features:
//! - `--features python` - Python PyO3 bindings
//! - `--features java` - Java JNI bindings
//! - `--features c_ffi` - C FFI for Go CGO
//! - `--features nodejs` - Node.js NAPI-RS bindings
//! - `--features embedded-all` - All language bindings

// Language-specific bindings - compiled when corresponding feature is enabled

// Python bindings via PyO3
#[cfg(feature = "python")]
pub mod python;

// Java bindings via JNI
#[cfg(feature = "java")]
pub mod java;

// C FFI for Go CGO and other C-compatible languages
#[cfg(feature = "c_ffi")]
pub mod c_ffi;

// Node.js bindings via NAPI-RS
#[cfg(feature = "nodejs")]
pub mod nodejs;

/// Embedded database configuration for multi-disk support
#[derive(Debug, Clone)]
pub struct EmbeddedConfig {
    /// Storage locations with weights for data distribution
    pub storage_locations: Vec<StorageLocationConfig>,
    /// Metadata storage path (should be on fast storage)
    pub metadata_path: String,
    /// Total cache size in MB
    pub cache_size_mb: usize,
    /// Default storage engine type
    pub default_engine: String,
    /// Enable WAL for durability
    pub enable_wal: bool,
    /// WAL sync mode: "immediate", "batch", or "async"
    pub wal_sync_mode: String,
}

impl Default for EmbeddedConfig {
    fn default() -> Self {
        Self {
            storage_locations: vec![StorageLocationConfig {
                path: "./data".to_string(),
                weight: 1,
                tags: vec![],
            }],
            metadata_path: "./data/metadata".to_string(),
            cache_size_mb: 512,
            default_engine: "sst".to_string(),
            enable_wal: true,
            wal_sync_mode: "batch".to_string(),
        }
    }
}

/// Storage location configuration for multi-disk support
#[derive(Debug, Clone)]
pub struct StorageLocationConfig {
    /// Path to storage directory (will be converted to file:// URL)
    pub path: String,
    /// Weight for data distribution (higher = more data)
    pub weight: u32,
    /// Tags for storage tier identification (e.g., "hot", "cold")
    pub tags: Vec<String>,
}

impl StorageLocationConfig {
    /// Create a new storage location
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            weight: 1,
            tags: vec![],
        }
    }

    /// Set the weight for this storage location
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Add a tag to this storage location
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Convert to file:// URL format for internal use
    pub fn to_url(&self) -> String {
        if self.path.starts_with("file://") {
            self.path.clone()
        } else {
            let abs_path = if self.path.starts_with('/') {
                self.path.clone()
            } else {
                // Make relative path absolute
                std::env::current_dir()
                    .map(|p| p.join(&self.path).to_string_lossy().to_string())
                    .unwrap_or_else(|_| self.path.clone())
            };
            format!("file://{}", abs_path)
        }
    }
}

/// Search result from embedded database
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Vector ID
    pub id: String,
    /// Similarity score (lower is more similar for distance metrics)
    pub score: f32,
    /// Associated metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// Collection information
#[derive(Debug, Clone)]
pub struct CollectionInfo {
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: u32,
    /// Number of vectors
    pub vector_count: u64,
    /// Storage engine type
    pub engine: String,
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    /// Total number of vectors across all collections
    pub total_vectors: u64,
    /// Total number of collections
    pub total_collections: u64,
    /// Total disk usage in bytes
    pub disk_usage_bytes: u64,
    /// Cache hit rate (0.0 to 1.0)
    pub cache_hit_rate: f64,
}

/// Embedded ProximaDB instance without network layer
///
/// This provides direct in-process access to ProximaDB functionality
/// without any network overhead. All operations are performed directly
/// against the storage engines.
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::embedded::{EmbeddedProximaDB, EmbeddedConfig, StorageLocationConfig};
///
/// let config = EmbeddedConfig {
///     storage_locations: vec![
///         StorageLocationConfig::new("/nvme/data").with_weight(2),
///         StorageLocationConfig::new("/hdd/data").with_weight(1),
///     ],
///     ..Default::default()
/// };
///
/// let db = EmbeddedProximaDB::new(config)?;
/// db.create_collection("embeddings", 768, None)?;
/// db.insert("embeddings", vec!["id1".into()], vec![vec![0.1; 768]], None)?;
/// let results = db.search("embeddings", vec![0.1; 768], 10, None)?;
/// ```
pub struct EmbeddedProximaDB {
    /// Configuration
    config: EmbeddedConfig,
    /// Tokio runtime for async operations
    runtime: tokio::runtime::Runtime,
    /// Shared services containing all ProximaDB functionality
    shared_services: crate::network::multi_server::SharedServices,
    /// Collection service for collection management
    collection_service: std::sync::Arc<crate::services::collection::manager::CollectionService>,
}

impl EmbeddedProximaDB {
    /// Create a new embedded ProximaDB instance
    ///
    /// This initializes the database with the given configuration,
    /// including multi-disk support and WAL settings.
    pub fn new(config: EmbeddedConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Create tokio runtime for async operations
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(num_cpus::get().min(4))
            .enable_all()
            .build()?;

        // Convert EmbeddedConfig to StorageConfig
        let storage_config = Self::to_storage_config(&config);

        // Initialize SharedServices using the runtime
        let (shared_services, collection_service) = runtime.block_on(async {
            Self::init_services(storage_config).await
        })?;

        Ok(Self {
            config,
            runtime,
            shared_services,
            collection_service,
        })
    }

    /// Convert EmbeddedConfig to the internal StorageConfig
    fn to_storage_config(config: &EmbeddedConfig) -> crate::core::config::StorageConfig {
        use crate::core::config::{StorageConfig, StorageLocation};

        // Convert storage locations
        let storage_locations: Vec<StorageLocation> = config
            .storage_locations
            .iter()
            .map(|loc| StorageLocation {
                url: loc.to_url(),
                weight: loc.weight,
                tags: loc.tags.clone(),
            })
            .collect();

        // Convert metadata path to URL
        let metadata_url = if config.metadata_path.starts_with("file://") {
            config.metadata_path.clone()
        } else if config.metadata_path.starts_with('/') {
            format!("file://{}", config.metadata_path)
        } else {
            let abs_path = std::env::current_dir()
                .map(|p| p.join(&config.metadata_path).to_string_lossy().to_string())
                .unwrap_or_else(|_| config.metadata_path.clone());
            format!("file://{}", abs_path)
        };

        StorageConfig {
            storage_locations,
            metadata_url,
            cache_size_mb: config.cache_size_mb as u64,
            wal_config: crate::core::config::WriteBufferUserConfig {
                enable_wal: config.enable_wal,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Initialize the internal services asynchronously
    async fn init_services(
        storage_config: crate::core::config::StorageConfig,
    ) -> Result<
        (
            crate::network::multi_server::SharedServices,
            std::sync::Arc<crate::services::collection::manager::CollectionService>,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        use std::sync::Arc;

        // Create cache orchestrator
        let cache_budget_bytes = (storage_config.cache_size_mb * 1024 * 1024) as usize;
        let orchestrator = Arc::new(CrossCacheOrchestrator::new(cache_budget_bytes));

        // Create SharedServices - this sets up all the internal components
        let (shared_services, collection_service) =
            crate::network::multi_server::SharedServices::new(
                None, // No metrics collector needed for embedded
                &storage_config,
                Some(orchestrator),
                None, // No full config needed
            )
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })?;

        Ok((shared_services, collection_service))
    }

    /// Create a new collection
    pub fn create_collection(
        &self,
        name: &str,
        dimension: u32,
        engine: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{CollectionConfig, StorageEngine as ProtoStorageEngine};

        // Parse storage engine
        let storage_engine = match engine.unwrap_or(&self.config.default_engine).to_lowercase().as_str() {
            "sst" => ProtoStorageEngine::Sst,
            "viper" => ProtoStorageEngine::Viper,
            "nova" => ProtoStorageEngine::Nova,
            "swift" => ProtoStorageEngine::Swift,
            "helix" => ProtoStorageEngine::Helix,
            "raptor" => ProtoStorageEngine::Raptor,
            _ => ProtoStorageEngine::Sst, // Default to SST
        };

        let config = CollectionConfig {
            name: name.to_string(),
            dimension,
            storage_engine: Some(storage_engine as i32),
            distance_metric: Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
            ..Default::default()
        };

        self.runtime.block_on(async {
            self.collection_service
                .create_collection(&config)
                .await
                .map(|_| ())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })
        })
    }

    /// Delete a collection
    pub fn delete_collection(
        &self,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            self.collection_service
                .delete_collection(name)
                .await
                .map(|_| ())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })
        })
    }

    /// Insert vectors into a collection
    ///
    /// Supports batched inserts for better performance. Internally, vectors are
    /// processed in batches of up to 1024 for optimal throughput.
    pub fn insert(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: Vec<Vec<f32>>,
        metadata: Option<Vec<std::collections::HashMap<String, serde_json::Value>>>,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{VectorRecord, SqlValue};
        use std::sync::Arc;

        // Convert to VectorRecord format
        let records: Vec<VectorRecord> = ids
            .into_iter()
            .zip(vectors.into_iter())
            .enumerate()
            .map(|(i, (id, vector))| {
                let meta: std::collections::HashMap<String, SqlValue> = metadata
                    .as_ref()
                    .and_then(|m| m.get(i))
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| {
                                let sql_val = SqlValue {
                                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                        v.to_string()
                                    )),
                                };
                                (k.clone(), sql_val)
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                VectorRecord {
                    id,
                    vector,
                    metadata: meta,
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    updated_at: None,
                    expires_at: None,
                    version: Some(0),
                    source: None,
                }
            })
            .collect();

        let count = records.len();
        let records = Arc::new(records);

        self.runtime.block_on(async {
            self.shared_services.vector_operations_service
                .insert_vectors_direct(collection, records)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;
            Ok(count)
        })
    }

    /// Search for similar vectors
    pub fn search(
        &self,
        collection: &str,
        query_vector: Vec<f32>,
        top_k: usize,
        _filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            // For now, don't support filter expressions in embedded mode
            // TODO: Parse filter string into FilterExpression
            let results = self
                .shared_services.vector_operations_service
                .unified_search_native(collection, query_vector, top_k, None, None)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Convert to embedded SearchResult format
            Ok(results
                .into_iter()
                .map(|r| SearchResult {
                    id: r.id,
                    score: r.score,
                    metadata: r
                        .metadata
                        .into_iter()
                        .map(|(k, v)| {
                            let val_str = match v.value {
                                Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => s,
                                Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)) => f.to_string(),
                                Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => i.to_string(),
                                Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => b.to_string(),
                                _ => String::new(),
                            };
                            (k, val_str)
                        })
                        .collect(),
                })
                .collect())
        })
    }

    /// Get collection information
    pub fn get_collection(
        &self,
        name: &str,
    ) -> Result<Option<CollectionInfo>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let collection = self
                .collection_service
                .get_collection_with_tenant_context(name, None)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(collection.map(|c| {
                let config = c.config.unwrap_or_default();
                CollectionInfo {
                    name: config.name,
                    dimension: config.dimension,
                    vector_count: c.stats.map(|s| s.vector_count as u64).unwrap_or(0),
                    engine: format!("{:?}", config.storage_engine.unwrap_or(0)),
                }
            }))
        })
    }

    /// List all collections
    pub fn list_collections(
        &self,
    ) -> Result<Vec<CollectionInfo>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let collections = self
                .collection_service
                .list_collections()
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(collections
                .into_iter()
                .map(|c| {
                    let config = c.config.unwrap_or_default();
                    CollectionInfo {
                        name: config.name,
                        dimension: config.dimension,
                        vector_count: c.stats.map(|s| s.vector_count as u64).unwrap_or(0),
                        engine: format!("{:?}", config.storage_engine.unwrap_or(0)),
                    }
                })
                .collect())
        })
    }

    /// Flush all pending writes to disk
    pub fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Flush is automatic in ProximaDB - WAL ensures durability
        Ok(())
    }

    /// Get storage statistics
    pub fn stats(&self) -> Result<StorageStats, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let collections = self.collection_service.list_collections().await.ok();
            let total_collections = collections.as_ref().map(|c| c.len() as u64).unwrap_or(0);
            let total_vectors: u64 = collections
                .map(|c| {
                    c.iter()
                        .filter_map(|col| col.stats.as_ref())
                        .map(|s| s.vector_count as u64)
                        .sum()
                })
                .unwrap_or(0);

            Ok(StorageStats {
                total_vectors,
                total_collections,
                disk_usage_bytes: 0, // TODO: Calculate actual disk usage
                cache_hit_rate: 0.0, // TODO: Get from cache orchestrator
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_location_url_conversion() {
        let loc = StorageLocationConfig::new("/nvme1/proximadb");
        assert_eq!(loc.to_url(), "file:///nvme1/proximadb");

        let loc = StorageLocationConfig::new("file:///already/url");
        assert_eq!(loc.to_url(), "file:///already/url");

        let loc = StorageLocationConfig::new("relative/path");
        assert!(loc.to_url().starts_with("file://"));
        assert!(loc.to_url().contains("relative/path"));
    }

    #[test]
    fn test_embedded_config_default() {
        let config = EmbeddedConfig::default();
        assert_eq!(config.cache_size_mb, 512);
        assert_eq!(config.default_engine, "sst");
        assert!(config.enable_wal);
    }

    #[test]
    fn test_storage_location_builder() {
        let loc = StorageLocationConfig::new("/data")
            .with_weight(2)
            .with_tag("hot");

        assert_eq!(loc.weight, 2);
        assert!(loc.tags.contains(&"hot".to_string()));
    }
}
