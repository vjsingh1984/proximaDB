//! Embedded mode wrapper for ProximaDB
//!
//! This module provides the `ProximaDB` wrapper for running ProximaDB
//! as an in-process database without network overhead.
//!
//! Requires the `embedded` feature to be enabled.

use crate::collection::{CollectionBuilder, CollectionHandle, IndexType, StorageEngine};
use crate::error::{CollectionError, EmbeddedError, ProximaError, Result};
use crate::search::{SearchMode, SearchResult};
use std::collections::HashMap;

/// Configuration for embedded ProximaDB
#[derive(Debug, Clone)]
pub struct EmbeddedConfig {
    /// Primary data directory
    pub data_dir: String,
    /// Additional storage locations with weights
    pub storage_locations: Vec<StorageLocation>,
    /// Cache size in MB
    pub cache_size_mb: usize,
    /// Default storage engine
    pub default_engine: StorageEngine,
    /// Enable WAL for durability
    pub enable_wal: bool,
    /// Enable RL-based query planner
    pub enable_rl_planner: bool,
}

impl Default for EmbeddedConfig {
    fn default() -> Self {
        Self {
            data_dir: "./data".to_string(),
            storage_locations: vec![],
            cache_size_mb: 512,
            default_engine: StorageEngine::Sst,
            enable_wal: true,
            enable_rl_planner: true,
        }
    }
}

impl EmbeddedConfig {
    /// Create a new embedded config with the given data directory
    pub fn new(data_dir: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ..Default::default()
        }
    }

    /// Create a minimal configuration for testing
    pub fn for_testing(data_dir: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            cache_size_mb: 64,
            enable_rl_planner: false,
            ..Default::default()
        }
    }

    /// Create an optimized configuration for benchmarks
    pub fn for_benchmarks(data_dir: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            cache_size_mb: 1024,
            enable_rl_planner: true,
            ..Default::default()
        }
    }
}

/// Storage location configuration for multi-disk support
#[derive(Debug, Clone)]
pub struct StorageLocation {
    /// Path to storage directory
    pub path: String,
    /// Weight for data distribution (higher = more data)
    pub weight: u32,
    /// Tags for storage tier identification
    pub tags: Vec<String>,
}

impl StorageLocation {
    /// Create a new storage location
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            weight: 1,
            tags: vec![],
        }
    }

    /// Set the weight
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Add a tag
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}

/// Builder for creating an embedded ProximaDB instance
#[derive(Debug, Clone)]
pub struct EmbeddedBuilder {
    config: EmbeddedConfig,
}

impl EmbeddedBuilder {
    /// Create a new embedded builder
    pub fn new() -> Self {
        Self {
            config: EmbeddedConfig::default(),
        }
    }

    /// Set the data directory
    pub fn data_dir(mut self, path: impl Into<String>) -> Self {
        self.config.data_dir = path.into();
        self
    }

    /// Add a storage location
    pub fn storage_location(mut self, location: StorageLocation) -> Self {
        self.config.storage_locations.push(location);
        self
    }

    /// Set the cache size in MB
    pub fn cache_size_mb(mut self, size: usize) -> Self {
        self.config.cache_size_mb = size;
        self
    }

    /// Set the default storage engine
    pub fn default_engine(mut self, engine: StorageEngine) -> Self {
        self.config.default_engine = engine;
        self
    }

    /// Enable or disable WAL
    pub fn enable_wal(mut self, enable: bool) -> Self {
        self.config.enable_wal = enable;
        self
    }

    /// Enable or disable RL planner
    pub fn enable_rl_planner(mut self, enable: bool) -> Self {
        self.config.enable_rl_planner = enable;
        self
    }

    /// Open the embedded database
    #[cfg(feature = "embedded")]
    pub fn open(self) -> Result<ProximaDB> {
        ProximaDB::with_config(self.config)
    }

    /// Open the embedded database (stub for non-embedded builds)
    #[cfg(not(feature = "embedded"))]
    pub fn open(self) -> Result<ProximaDB> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }
}

impl Default for EmbeddedBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Embedded ProximaDB instance
///
/// Provides direct in-process access to ProximaDB without network overhead.
///
/// # Example
///
/// ```rust,ignore
/// use proximadb_sdk::{ProximaDB, StorageEngine};
///
/// let db = ProximaDB::embedded()
///     .data_dir("/tmp/agent-memory")
///     .cache_size_mb(512)
///     .open()?;
///
/// // Create a collection
/// db.create_collection("memories")
///     .dimension(768)
///     .engine(StorageEngine::Sst)
///     .execute_sync()?;
///
/// // Insert vectors
/// db.collection("memories")
///     .insert()
///     .id("mem_1")
///     .vector(&embedding)
///     .meta("type", "conversation")
///     .execute_sync()?;
///
/// // Search
/// let results = db.collection("memories")
///     .search_embedded()
///     .vector(&query)
///     .top_k(10)
///     .execute_sync()?;
///
/// // Flush and close
/// db.flush()?;
/// db.close()?;
/// ```
pub struct ProximaDB {
    #[cfg(feature = "embedded")]
    inner: proximadb::embedded::EmbeddedProximaDB,
    config: EmbeddedConfig,
}

impl ProximaDB {
    /// Get a builder for creating an embedded database
    pub fn embedded() -> EmbeddedBuilder {
        EmbeddedBuilder::new()
    }

    /// Create an embedded database with the given config
    #[cfg(feature = "embedded")]
    pub fn with_config(config: EmbeddedConfig) -> Result<Self> {
        // Convert SDK config to internal EmbeddedConfig
        let mut storage_locations = vec![proximadb::embedded::StorageLocationConfig::new(
            &config.data_dir,
        )];

        for loc in &config.storage_locations {
            let mut storage_loc = proximadb::embedded::StorageLocationConfig::new(&loc.path);
            storage_loc = storage_loc.with_weight(loc.weight);
            for tag in &loc.tags {
                storage_loc = storage_loc.with_tag(tag);
            }
            storage_locations.push(storage_loc);
        }

        let internal_config = proximadb::embedded::EmbeddedConfig {
            storage_locations,
            metadata_path: format!("{}/metadata", config.data_dir),
            cache_size_mb: config.cache_size_mb,
            default_engine: config.default_engine.as_str().to_string(),
            enable_wal: config.enable_wal,
            wal_sync_mode: "batch".to_string(),
            block_prune_mode: "sqrt".to_string(),
            block_prune_ratio: 0.2,
            block_prune_min_keep: 1,
            block_prune_max_keep: 0,
            enable_rl_planner: config.enable_rl_planner,
            rl_policy_path: Some(format!("{}/rl_policy.json", config.data_dir)),
            access_mode: proximadb::embedded::AccessMode::Exclusive,
            node_id: None,
        };

        let inner = proximadb::embedded::EmbeddedProximaDB::new(internal_config).map_err(|e| {
            ProximaError::Embedded(EmbeddedError::InitializationFailed {
                reason: e.to_string(),
            })
        })?;

        Ok(Self { inner, config })
    }

    /// Create an embedded database with default config at the given path
    #[cfg(feature = "embedded")]
    pub fn open(data_dir: impl Into<String>) -> Result<Self> {
        Self::with_config(EmbeddedConfig::new(data_dir))
    }

    /// Stub for non-embedded builds
    #[cfg(not(feature = "embedded"))]
    pub fn with_config(_config: EmbeddedConfig) -> Result<Self> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    /// Stub for non-embedded builds
    #[cfg(not(feature = "embedded"))]
    pub fn open(_data_dir: impl Into<String>) -> Result<Self> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    /// Get a handle to a collection
    pub fn collection(&self, name: &str) -> CollectionHandle<'_> {
        CollectionHandle::new_embedded(self, name)
    }

    /// Create a collection builder
    pub fn create_collection(&self, name: &str) -> CollectionBuilder<'_> {
        CollectionBuilder::new_embedded(self, name)
    }

    /// Delete a collection
    #[cfg(feature = "embedded")]
    pub fn delete_collection(&self, name: &str) -> Result<()> {
        self.inner.delete_collection(name).map_err(|_| {
            ProximaError::Collection(CollectionError::NotFound {
                name: name.to_string(),
            })
        })
    }

    /// List all collections
    #[cfg(feature = "embedded")]
    pub fn list_collections(&self) -> Result<Vec<String>> {
        self.inner
            .list_collections()
            .map(|collections| collections.into_iter().map(|c| c.name).collect())
            .map_err(|e| ProximaError::Internal(e.to_string()))
    }

    /// Flush all pending writes to disk
    #[cfg(feature = "embedded")]
    pub fn flush(&self) -> Result<()> {
        self.inner.flush().map_err(|e| {
            ProximaError::Embedded(EmbeddedError::FlushError {
                reason: e.to_string(),
            })
        })
    }

    /// Close the database gracefully
    #[cfg(feature = "embedded")]
    pub fn close(self) -> Result<()> {
        self.inner.close();
        Ok(())
    }

    /// Get storage statistics
    #[cfg(feature = "embedded")]
    pub fn storage_stats(&self) -> Result<StorageStats> {
        let stats = self
            .inner
            .stats()
            .map_err(|e| ProximaError::Internal(e.to_string()))?;

        Ok(StorageStats {
            total_vectors: stats.total_vectors,
            total_collections: stats.total_collections,
            disk_usage_bytes: stats.disk_usage_bytes,
            cache_hit_rate: stats.cache_hit_rate,
        })
    }

    /// Get the data directory
    pub fn data_dir(&self) -> &str {
        &self.config.data_dir
    }

    // Internal methods for builder pattern support

    #[cfg(feature = "embedded")]
    pub(crate) fn create_collection_internal(
        &self,
        name: &str,
        dimension: u32,
        engine: &StorageEngine,
        index: &IndexType,
    ) -> Result<()> {
        self.inner
            .create_collection_with_index(name, dimension, Some(engine.as_str()), index.as_str())
            .map_err(|e| {
                ProximaError::Collection(CollectionError::InvalidConfig {
                    reason: e.to_string(),
                })
            })
    }

    #[cfg(feature = "embedded")]
    pub(crate) fn insert_internal(
        &self,
        collection: &str,
        id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        self.inner
            .insert(collection, vec![id], vec![vector], Some(vec![metadata]))
            .map_err(|e| ProximaError::Internal(e.to_string()))?;
        Ok(())
    }

    #[cfg(feature = "embedded")]
    pub(crate) fn insert_batch_internal(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: Vec<Vec<f32>>,
        metadata: Vec<HashMap<String, serde_json::Value>>,
    ) -> Result<usize> {
        let count = ids.len();
        self.inner
            .insert(collection, ids, vectors, Some(metadata))
            .map_err(|e| ProximaError::Internal(e.to_string()))?;
        Ok(count)
    }

    #[cfg(feature = "embedded")]
    pub(crate) fn search_internal(
        &self,
        collection: &str,
        vector: Vec<f32>,
        top_k: usize,
        filter: Option<String>,
        mode: SearchMode,
    ) -> Result<Vec<SearchResult>> {
        let mode_str = mode.as_str();
        let results = self
            .inner
            .search_with_mode(
                collection,
                vector,
                top_k,
                filter.as_deref(),
                Some(&mode_str),
            )
            .map_err(|e| ProximaError::Internal(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|r| SearchResult {
                id: r.id,
                score: r.score,
                metadata: r.metadata,
                vector: None,
            })
            .collect())
    }
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

// Stub implementations for non-embedded builds
#[cfg(not(feature = "embedded"))]
impl ProximaDB {
    pub fn delete_collection(&self, _name: &str) -> Result<()> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub fn flush(&self) -> Result<()> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub fn close(self) -> Result<()> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub fn storage_stats(&self) -> Result<StorageStats> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub(crate) fn create_collection_internal(
        &self,
        _name: &str,
        _dimension: u32,
        _engine: &StorageEngine,
        _index: &IndexType,
    ) -> Result<()> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub(crate) fn insert_internal(
        &self,
        _collection: &str,
        _id: String,
        _vector: Vec<f32>,
        _metadata: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub(crate) fn insert_batch_internal(
        &self,
        _collection: &str,
        _ids: Vec<String>,
        _vectors: Vec<Vec<f32>>,
        _metadata: Vec<HashMap<String, serde_json::Value>>,
    ) -> Result<usize> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }

    pub(crate) fn search_internal(
        &self,
        _collection: &str,
        _vector: Vec<f32>,
        _top_k: usize,
        _filter: Option<String>,
        _mode: SearchMode,
    ) -> Result<Vec<SearchResult>> {
        Err(ProximaError::Embedded(EmbeddedError::NotAvailable))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_config_default() {
        let config = EmbeddedConfig::default();
        assert_eq!(config.data_dir, "./data");
        assert_eq!(config.cache_size_mb, 512);
        assert!(config.enable_wal);
    }

    #[test]
    fn test_embedded_builder() {
        let builder = ProximaDB::embedded()
            .data_dir("/tmp/test")
            .cache_size_mb(256)
            .enable_wal(false);

        assert_eq!(builder.config.data_dir, "/tmp/test");
        assert_eq!(builder.config.cache_size_mb, 256);
        assert!(!builder.config.enable_wal);
    }

    #[test]
    fn test_storage_location() {
        let loc = StorageLocation::new("/data/ssd")
            .with_weight(2)
            .with_tag("hot");

        assert_eq!(loc.path, "/data/ssd");
        assert_eq!(loc.weight, 2);
        assert_eq!(loc.tags, vec!["hot"]);
    }
}
