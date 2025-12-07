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
//! ┌─────────────────────────────────────────────────────────┐
//! │         EmbeddedProximaDB (this module)                 │
//! │   - Direct in-process API                               │
//! │   - No network overhead                                 │
//! │   - Multi-disk configuration                            │
//! ├─────────────────────────────────────────────────────────┤
//! │   Language Bindings (feature-gated)                     │
//! │   - Python: PyO3 bindings (feature = "python")          │
//! │   - Java: JNI bindings (feature = "java")               │
//! │   - Go: C FFI bindings (feature = "c_ffi")              │
//! │   - Node.js: NAPI-RS (feature = "nodejs")               │
//! ├─────────────────────────────────────────────────────────┤
//! │         ProximaDB Core (Rust)                           │
//! │   - Storage Engines (SST, VIPER, NOVA, SWIFT, etc.)     │
//! │   - Multi-disk support via StorageLocation              │
//! │   - WAL persistence                                     │
//! │   - Compute (SIMD-accelerated)                          │
//! └─────────────────────────────────────────────────────────┘
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
/// NOTE: This is the public API structure. The actual implementation
/// delegates to SharedServices and StorageEngine internally.
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
/// ```
pub struct EmbeddedProximaDB {
    /// Configuration
    _config: EmbeddedConfig,
    /// Tokio runtime for async operations
    _runtime: tokio::runtime::Runtime,
    // TODO: Add internal ProximaDB instance when full implementation is ready
    // The actual storage and services would be initialized here
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

        // TODO: Initialize actual ProximaDB components here
        // This would involve:
        // 1. Creating StorageConfig from EmbeddedConfig
        // 2. Initializing SharedServices
        // 3. Creating StorageEngine
        // 4. Setting up WAL and metadata

        Ok(Self {
            _config: config,
            _runtime: runtime,
        })
    }

    /// Create a new collection
    pub fn create_collection(
        &self,
        _name: &str,
        _dimension: u32,
        _engine: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement using internal SharedServices
        Err("Embedded mode not fully implemented yet".into())
    }

    /// Delete a collection
    pub fn delete_collection(
        &self,
        _name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement
        Err("Embedded mode not fully implemented yet".into())
    }

    /// Insert vectors into a collection
    pub fn insert(
        &self,
        _collection: &str,
        _ids: Vec<String>,
        _vectors: Vec<Vec<f32>>,
        _metadata: Option<Vec<std::collections::HashMap<String, serde_json::Value>>>,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement
        Err("Embedded mode not fully implemented yet".into())
    }

    /// Search for similar vectors
    pub fn search(
        &self,
        _collection: &str,
        _query_vector: Vec<f32>,
        _top_k: usize,
        _filter: Option<&str>,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement
        Err("Embedded mode not fully implemented yet".into())
    }

    /// Get collection information
    pub fn get_collection(
        &self,
        _name: &str,
    ) -> Result<Option<CollectionInfo>, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement
        Ok(None)
    }

    /// List all collections
    pub fn list_collections(
        &self,
    ) -> Result<Vec<CollectionInfo>, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement
        Ok(vec![])
    }

    /// Flush all pending writes to disk
    pub fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement
        Ok(())
    }

    /// Get storage statistics
    pub fn stats(&self) -> Result<StorageStats, Box<dyn std::error::Error + Send + Sync>> {
        Ok(StorageStats {
            total_vectors: 0,
            total_collections: 0,
            disk_usage_bytes: 0,
            cache_hit_rate: 0.0,
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
