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

// Import VectorRecord for get_vector and vector_exists operations
use crate::proto::proximadb_v1::VectorRecord;
use crate::core::config::{AdvancedPruneConfig, PruneModeConfig};

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
    /// Block prune mode for approximate search: "sqrt" (default), "ratio", "fixed", or "exact" (disabled)
    pub block_prune_mode: String,
    /// Block prune ratio (0.0-1.0) when mode is "ratio"
    pub block_prune_ratio: f32,
    /// Minimum blocks to keep (used with sqrt/ratio modes)
    pub block_prune_min_keep: usize,
    /// Maximum blocks to keep (0 = no cap)
    pub block_prune_max_keep: usize,
    /// Enable RL-based adaptive query planner
    pub enable_rl_planner: bool,
    /// Path for RL policy persistence (default: data_dir/rl_policy.json)
    pub rl_policy_path: Option<String>,
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
            // Block pruning defaults (sqrt mode with sensible bounds)
            block_prune_mode: "sqrt".to_string(),
            block_prune_ratio: 0.2,
            block_prune_min_keep: 1,
            block_prune_max_keep: 0, // No cap
            // RL planner defaults (enabled by default for adaptive query optimization)
            enable_rl_planner: true,
            rl_policy_path: None, // Default to data_dir/rl_policy.json
        }
    }
}

impl EmbeddedConfig {
    /// Create an optimized configuration for embedded benchmarks
    ///
    /// This configuration is tuned for:
    /// - Maximum write throughput
    /// - Low-latency reads
    /// - Efficient memory usage with adaptive bloom filters
    /// - Aggressive compaction for smaller footprint
    pub fn for_benchmarks(data_path: impl Into<String>) -> Self {
        let path = data_path.into();
        Self {
            storage_locations: vec![StorageLocationConfig {
                path: path.clone(),
                weight: 1,
                tags: vec!["benchmark".to_string()],
            }],
            metadata_path: format!("{}/metadata", path),
            cache_size_mb: 1024, // 1GB cache for benchmarks
            default_engine: "sst".to_string(),
            enable_wal: true,
            wal_sync_mode: "batch".to_string(), // Batch for better throughput
            block_prune_mode: "sqrt".to_string(),
            block_prune_ratio: 0.2,
            block_prune_min_keep: 1,
            block_prune_max_keep: 0,
            enable_rl_planner: true, // Enable RL for benchmark optimization
            rl_policy_path: Some(format!("{}/rl_policy.json", path)),
        }
    }

    /// Create an optimized configuration for memory-constrained environments
    ///
    /// Uses minimal cache and aggressive pruning
    pub fn for_low_memory(data_path: impl Into<String>) -> Self {
        let path = data_path.into();
        Self {
            storage_locations: vec![StorageLocationConfig {
                path: path.clone(),
                weight: 1,
                tags: vec![],
            }],
            metadata_path: format!("{}/metadata", path),
            cache_size_mb: 128, // Minimal cache
            default_engine: "sst".to_string(),
            enable_wal: true,
            wal_sync_mode: "batch".to_string(),
            block_prune_mode: "sqrt".to_string(),
            block_prune_ratio: 0.2,
            block_prune_min_keep: 1,
            block_prune_max_keep: 0,
            enable_rl_planner: false, // Disable RL for memory-constrained environments
            rl_policy_path: None,
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
    /// Disk usage in bytes for this collection
    pub disk_usage_bytes: u64,
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

// ============================================================================
// Generic Graph Database Types - Tool Agnostic API
// ============================================================================
//
// These types provide a generic, flexible graph API that can be used for
// any domain: social graphs, knowledge graphs, code graphs, etc.
// Domain-specific fields (like code's "signature", "docstring", "line")
// should be stored in the `properties` map.
//
// For code intelligence use cases, the consuming application (e.g., Victor)
// should build an adapter layer that maps its domain types to these generic types.

/// Generic graph node with flexible property storage
///
/// This is a domain-agnostic node type. All domain-specific attributes
/// should be stored in the `properties` map.
///
/// # Example
/// ```rust,ignore
/// // For a code symbol:
/// let node = GraphNode::new("fn_main")
///     .with_label("function")
///     .with_property("name", "main")
///     .with_property("file", "main.py")
///     .with_property("line", "42");
///
/// // For a social network:
/// let node = GraphNode::new("user_123")
///     .with_label("Person")
///     .with_property("name", "Alice")
///     .with_property("email", "alice@example.com");
/// ```
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Unique node identifier
    pub id: String,
    /// Node labels/types (e.g., "Person", "function", "Document")
    pub labels: Vec<String>,
    /// Flexible property storage for domain-specific attributes
    pub properties: std::collections::HashMap<String, String>,
}

impl GraphNode {
    /// Create a new node with the given ID
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            labels: Vec::new(),
            properties: std::collections::HashMap::new(),
        }
    }

    /// Add a label to this node
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }

    /// Add a property to this node
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Convert to proto Node for storage
    pub fn to_proto(&self) -> crate::proto::proximadb_v1::Node {
        use crate::proto::proximadb_v1::{Node, PropertyValue, property_value::Value};

        let properties: std::collections::HashMap<String, PropertyValue> = self.properties
            .iter()
            .map(|(k, v)| {
                (k.clone(), PropertyValue {
                    value: Some(Value::StringValue(v.clone()))
                })
            })
            .collect();

        Node {
            id: self.id.clone(),
            labels: self.labels.clone(),
            properties,
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create from proto Node
    pub fn from_proto(node: &crate::proto::proximadb_v1::Node) -> Self {
        use crate::proto::proximadb_v1::property_value::Value;

        let properties: std::collections::HashMap<String, String> = node.properties.iter()
            .filter_map(|(k, v)| {
                match &v.value {
                    Some(Value::StringValue(s)) => Some((k.clone(), s.clone())),
                    Some(Value::IntValue(i)) => Some((k.clone(), i.to_string())),
                    Some(Value::DoubleValue(d)) => Some((k.clone(), d.to_string())),
                    Some(Value::BoolValue(b)) => Some((k.clone(), b.to_string())),
                    _ => None
                }
            })
            .collect();

        Self {
            id: node.id.clone(),
            labels: node.labels.clone(),
            properties,
        }
    }
}

/// Generic graph edge with flexible property storage
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// Optional edge ID (auto-generated if not provided)
    pub id: Option<String>,
    /// Source node ID
    pub from_node_id: String,
    /// Destination node ID
    pub to_node_id: String,
    /// Edge type/relationship name
    pub edge_type: String,
    /// Optional weight for weighted traversal
    pub weight: Option<f64>,
    /// Flexible property storage
    pub properties: std::collections::HashMap<String, String>,
}

impl GraphEdge {
    /// Create a new edge
    pub fn new(from_node_id: impl Into<String>, to_node_id: impl Into<String>, edge_type: impl Into<String>) -> Self {
        Self {
            id: None,
            from_node_id: from_node_id.into(),
            to_node_id: to_node_id.into(),
            edge_type: edge_type.into(),
            weight: None,
            properties: std::collections::HashMap::new(),
        }
    }

    /// Set edge ID explicitly
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set edge weight
    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Add a property
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Generate edge ID from components
    fn generated_id(&self) -> String {
        format!("{}->{}:{}", self.from_node_id, self.to_node_id, self.edge_type)
    }

    /// Convert to proto Edge
    pub fn to_proto(&self) -> crate::proto::proximadb_v1::Edge {
        use crate::proto::proximadb_v1::{Edge, PropertyValue, property_value::Value};

        let properties: std::collections::HashMap<String, PropertyValue> = self.properties
            .iter()
            .map(|(k, v)| {
                (k.clone(), PropertyValue {
                    value: Some(Value::StringValue(v.clone()))
                })
            })
            .collect();

        Edge {
            id: self.id.clone().unwrap_or_else(|| self.generated_id()),
            from_node_id: self.from_node_id.clone(),
            to_node_id: self.to_node_id.clone(),
            edge_type: self.edge_type.clone(),
            properties,
            weight: self.weight,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create from proto Edge
    pub fn from_proto(edge: &crate::proto::proximadb_v1::Edge) -> Self {
        use crate::proto::proximadb_v1::property_value::Value;

        let properties: std::collections::HashMap<String, String> = edge.properties.iter()
            .filter_map(|(k, v)| {
                match &v.value {
                    Some(Value::StringValue(s)) => Some((k.clone(), s.clone())),
                    Some(Value::IntValue(i)) => Some((k.clone(), i.to_string())),
                    Some(Value::DoubleValue(d)) => Some((k.clone(), d.to_string())),
                    Some(Value::BoolValue(b)) => Some((k.clone(), b.to_string())),
                    _ => None
                }
            })
            .collect();

        Self {
            id: Some(edge.id.clone()),
            from_node_id: edge.from_node_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            edge_type: edge.edge_type.clone(),
            weight: edge.weight,
            properties,
        }
    }
}

/// Graph statistics
#[derive(Debug, Clone)]
pub struct GraphStats {
    /// Total number of nodes
    pub total_nodes: u64,
    /// Total number of edges
    pub total_edges: u64,
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
/// // Call close() to persist RL policy before dropping
/// db.close();
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
    /// Path where RL planner policy is persisted (None if RL disabled)
    rl_policy_path: Option<String>,
}

impl EmbeddedProximaDB {
    /// Create a new embedded ProximaDB instance
    ///
    /// This initializes the database with the given configuration,
    /// including multi-disk support and WAL settings.
    pub fn new(config: EmbeddedConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // [AGENT_FIX]: Forcefully reset global static state to allow multiple
        // embedded instances within the same process, which is critical for tests
        // and benchmarks. This is an unsafe workaround for a design limitation
        // where the engine relies on `OnceLock` globals.
        unsafe {
            crate::storage::persistence::write_ahead_log::reset_global_wal_state_for_tests();
            tracing::info!("🧹 EMBEDDED: Unsafe reset of global state (manifest, write buffer, registry) complete.");
        }

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

        // Initialize RL planner if enabled
        let rl_policy_path = if config.enable_rl_planner {
            let policy_path = config.rl_policy_path.clone().unwrap_or_else(|| {
                // Default to first storage location + /rl_policy.json
                if let Some(loc) = config.storage_locations.first() {
                    format!("{}/rl_policy.json", loc.path.trim_end_matches('/'))
                } else {
                    "./data/rl_policy.json".to_string()
                }
            });

            // Initialize RL planner with default config
            let rl_config = crate::query::rl_planner::RLPlannerConfig::default();
            crate::query::rl_planner::init_rl_planner(rl_config);

            // Try to load existing policy
            if let Some(planner) = crate::query::rl_planner::get_rl_planner() {
                if std::path::Path::new(&policy_path).exists() {
                    runtime.block_on(async {
                        match planner.load_policy(&policy_path).await {
                            Ok(()) => {
                                tracing::info!("🎯 EMBEDDED: RL policy loaded from {}", policy_path);
                            }
                            Err(e) => {
                                tracing::debug!("EMBEDDED: No existing RL policy (starting fresh): {}", e);
                            }
                        }
                    });
                }
            }

            tracing::info!("🎯 EMBEDDED: RL Query Planner initialized");
            Some(policy_path)
        } else {
            tracing::debug!("EMBEDDED: RL Query Planner disabled");
            None
        };

        Ok(Self {
            config,
            runtime,
            shared_services,
            collection_service,
            rl_policy_path,
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

        // Create optimized SST config for embedded mode
        // Adaptive bloom filters are automatically used by the SST writer
        let sst_config = crate::core::config::SstConfig {
            // Use default block size (256KB) optimized for NVMe
            block_size_kb: 256,
            // Enable bloom filters with adaptive sizing
            bloom_filter_config: Some(crate::core::bloom::BloomFilterConfig {
                enabled: true,
                strategy: crate::core::bloom::BloomStrategy::ByteAligned,
                bits_per_key: 10, // Base value, adaptive sizing will optimize this
                false_positive_rate: Some(0.01), // 1% FPR target
                expected_items: 10000,
                hash_algorithm: crate::core::bloom::HashAlgorithm::XXHash,
            }),
            // Enable LZ4 compression for better throughput
            compression: "lz4".to_string(),
            compression_level: 3,
            // Aggressive compaction for embedded mode
            compaction_threshold: 4, // Compact more frequently than server (5)
            compaction_strategy: "leveled".to_string(),
            // Optimize for embedded workloads
            cache_size_mb: (config.cache_size_mb / 4).max(32) as u64, // Reserve some cache for SST
            mmap_enabled: true,
            prefetch_enabled: true,
            prefetch_size_kb: 64,
            ..Default::default()
        };

        let prune_mode = match config.block_prune_mode.as_str() {
            "exact" => None,
            "sqrt" => Some(PruneModeConfig::Simple("sqrt".to_string())),
            "ratio" => Some(PruneModeConfig::Advanced(AdvancedPruneConfig {
                r#type: "ratio".to_string(),
                min_keep: Some(config.block_prune_min_keep),
                max_keep: Some(config.block_prune_max_keep),
                ratio: Some(config.block_prune_ratio),
            })),
            other => Some(PruneModeConfig::Simple(other.to_string())),
        };

        StorageConfig {
            storage_locations,
            metadata_url,
            cache_size_mb: config.cache_size_mb as u64,
            sst_config: Some(sst_config),
            // Enable bloom filters globally
            bloom_filter_config: Some(crate::core::bloom::BloomFilterConfig {
                enabled: true,
                strategy: crate::core::bloom::BloomStrategy::ByteAligned,
                bits_per_key: 10,
                false_positive_rate: Some(0.01),
                expected_items: 10000,
                hash_algorithm: crate::core::bloom::HashAlgorithm::XXHash,
            }),
            wal_config: crate::core::config::WriteBufferUserConfig {
                enable_wal: config.enable_wal,
                ..Default::default()
            },
            prune_mode,
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

        // Initialize hardware capabilities first
        if let Err(e) = crate::core::hardware_capabilities::initialize_hardware_capabilities_default() {
            tracing::warn!("Failed to initialize hardware capabilities: {}", e);
        }

        // Log hardware capabilities summary
        let _hw_summary = crate::core::hardware_capabilities::log_hardware_capabilities_summary();

        // Initialize global WAL manifest for proper WAL file cleanup
        // This is critical for embedded mode to avoid duplicate data
        Self::init_global_manifest(&storage_config).await?;

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

        // Set global metadata provider for WAL path resolution
        // This eliminates the "No metadata provider after 100ms" warning
        // by ensuring WAL operations can resolve collection paths immediately
        crate::storage::persistence::write_ahead_log::set_global_metadata_provider(
            collection_service.metadata_backend().clone()
        ).await;
        tracing::debug!("✅ Embedded: Global metadata provider set for WAL path resolution");

        Ok((shared_services, collection_service))
    }

    /// Initialize the global WAL manifest for proper WAL file cleanup
    async fn init_global_manifest(
        storage_config: &crate::core::config::StorageConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::storage::persistence::write_ahead_log::config::WALConfig;
        use crate::storage::persistence::write_ahead_log::manifest;

        // In embedded mode, always reset the manifest to support multiple database instances
        // with different storage locations in the same process
        if let Err(e) = manifest::reset().await {
            tracing::debug!("Note: manifest reset returned error (may not have been initialized): {}", e);
        }

        // Build WAL config from storage config
        let mut wal_config = WALConfig::default();

        // Set up data directories from storage locations
        wal_config.multi_disk.data_directories = storage_config
            .storage_locations
            .iter()
            .map(|loc| loc.url.clone())
            .collect();

        // Set the manifest URL to the first storage location + /manifest
        if let Some(first_loc) = storage_config.storage_locations.first() {
            let base_url = first_loc.url.trim_end_matches('/');
            wal_config.global_manifest_url = Some(format!("{}/manifest", base_url));
        }

        // Initialize the global manifest
        match manifest::init(&wal_config).await {
            Ok(_) => {
                tracing::info!("🗂️  Initialized global WAL manifest for embedded mode");
                Ok(())
            }
            Err(e) => {
                tracing::warn!("⚠️  Failed to initialize global WAL manifest: {}. WAL files may not be cleaned up after flush.", e);
                // Don't fail - embedded mode can still work, just with duplicate data
                Ok(())
            }
        }
    }

    /// Create a new collection
    pub fn create_collection(
        &self,
        name: &str,
        dimension: u32,
        engine: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{
            CollectionConfig, CompressionAlgorithm, HnswConfig, IndexConfig, IndexingAlgorithm,
            StorageConfig,
        };

        let storage_engine = match engine.unwrap_or(&self.config.default_engine).to_lowercase().as_str() {
            "sst" => crate::proto::proximadb_v1::StorageEngine::Sst,
            "helix" => crate::proto::proximadb_v1::StorageEngine::Helix,
            "viper" => crate::proto::proximadb_v1::StorageEngine::Viper,
            "nova" => crate::proto::proximadb_v1::StorageEngine::Nova,
            "swift" => crate::proto::proximadb_v1::StorageEngine::Swift,
            "raptor" => crate::proto::proximadb_v1::StorageEngine::Raptor,
            other => {
                return Err(format!("Unknown storage engine: {}", other).into());
            }
        };

        // Create default HNSW index config for automatic index building
        // This enables AXIS EventLog consumer to build HNSW indexes on flush
        let default_hnsw_config = IndexConfig {
            index_name: "default_hnsw".to_string(),
            algorithm: IndexingAlgorithm::Hnsw as i32,
            enabled: Some(true),
            hnsw_config: Some(HnswConfig {
                m: Some(16),                    // Balanced connectivity
                ef_construction: Some(200),     // Good build quality
                ef_search: Some(50),            // Fast search with good recall
                max_partition_size: Some(100_000),
                adaptive_parameters: Some(true),
                use_simd: Some(true),
                memory_limit_mb: Some(512),
                lazy_loading: Some(false),
            }),
            ..Default::default()
        };

        let collection_config = CollectionConfig {
            name: name.to_string(),
            dimension,
            storage_engine: Some(storage_engine as i32),
            storage_config: Some(StorageConfig {
                compression: Some(CompressionAlgorithm::CompressionLz4 as i32),
                ..Default::default()
            }),
            index_configs: vec![default_hnsw_config], // Enable HNSW by default
            ..Default::default()
        };

        self.runtime.block_on(async {
            let response = self.shared_services.collection_service
                .create_collection(&collection_config)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Check if the collection service returned an error in the response
            if !response.success {
                let error_msg = response.error_code.unwrap_or_else(|| "Unknown error".to_string());
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Failed to create collection '{}': {}", name, error_msg)
                )) as Box<dyn std::error::Error + Send + Sync>);
            }

            // Register the collection in the global cache for EventLog consumer
            // This enables AXIS index building when flush events occur
            if let Some(collection) = response.collection {
                crate::services::events::log::register_collection_in_cache(
                    std::sync::Arc::new(collection)
                );
                tracing::info!(
                    "📦 EMBEDDED: Registered collection '{}' in global cache for AXIS indexing",
                    name
                );
            }

            Ok(())
        })
    }

    /// Create a collection with explicit index configuration
    ///
    /// # Arguments
    /// * `name` - Collection name
    /// * `dimension` - Vector dimension
    /// * `engine` - Optional storage engine ("sst", "helix", "viper", "swift", "nova", "raptor")
    /// * `index_type` - Index type: "hnsw", "ivf", "flat", "lsh", or "none"
    ///
    /// # Example
    /// ```rust,ignore
    /// db.create_collection_with_index("my_collection", 768, Some("sst"), "hnsw")?;
    /// ```
    pub fn create_collection_with_index(
        &self,
        name: &str,
        dimension: u32,
        engine: Option<&str>,
        index_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{
            CollectionConfig, CompressionAlgorithm, HnswConfig, IndexConfig, IndexingAlgorithm,
            IvfConfig, LshConfig, StorageConfig,
        };

        let storage_engine = match engine.unwrap_or(&self.config.default_engine).to_lowercase().as_str() {
            "sst" => crate::proto::proximadb_v1::StorageEngine::Sst,
            "helix" => crate::proto::proximadb_v1::StorageEngine::Helix,
            "viper" => crate::proto::proximadb_v1::StorageEngine::Viper,
            "nova" => crate::proto::proximadb_v1::StorageEngine::Nova,
            "swift" => crate::proto::proximadb_v1::StorageEngine::Swift,
            "raptor" => crate::proto::proximadb_v1::StorageEngine::Raptor,
            other => {
                return Err(format!("Unknown storage engine: {}", other).into());
            }
        };

        // Build index config based on requested type
        let index_configs = match index_type.to_lowercase().as_str() {
            "hnsw" => {
                vec![IndexConfig {
                    index_name: "hnsw_index".to_string(),
                    algorithm: IndexingAlgorithm::Hnsw as i32,
                    enabled: Some(true),
                    hnsw_config: Some(HnswConfig {
                        m: Some(16),
                        ef_construction: Some(200),
                        ef_search: Some(50),
                        max_partition_size: Some(100_000),
                        adaptive_parameters: Some(true),
                        use_simd: Some(true),
                        memory_limit_mb: Some(512),
                        lazy_loading: Some(false),
                    }),
                    ..Default::default()
                }]
            }
            "ivf" => {
                // Use sqrt(n) clusters as default for IVF
                let n_clusters = ((dimension as f64).sqrt() * 4.0) as u32;
                vec![IndexConfig {
                    index_name: "ivf_index".to_string(),
                    algorithm: IndexingAlgorithm::Ivf as i32,
                    enabled: Some(true),
                    ivf_config: Some(IvfConfig {
                        n_lists: Some(n_clusters.max(64)),
                        n_probe: Some(10),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            }
            "lsh" => {
                vec![IndexConfig {
                    index_name: "lsh_index".to_string(),
                    algorithm: IndexingAlgorithm::Lsh as i32,
                    enabled: Some(true),
                    lsh_config: Some(LshConfig {
                        n_hash_tables: Some(10),
                        n_hash_functions: Some(8),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]
            }
            "flat" | "none" => {
                // No index - brute force search
                vec![]
            }
            other => {
                return Err(format!("Unknown index type: {}. Supported: hnsw, ivf, lsh, flat, none", other).into());
            }
        };

        let collection_config = CollectionConfig {
            name: name.to_string(),
            dimension,
            storage_engine: Some(storage_engine as i32),
            storage_config: Some(StorageConfig {
                compression: Some(CompressionAlgorithm::CompressionLz4 as i32),
                ..Default::default()
            }),
            index_configs,
            ..Default::default()
        };

        self.runtime.block_on(async {
            let response = self.shared_services.collection_service
                .create_collection(&collection_config)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            if !response.success {
                let error_msg = response.error_code.unwrap_or_else(|| "Unknown error".to_string());
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Failed to create collection '{}': {}", name, error_msg)
                )) as Box<dyn std::error::Error + Send + Sync>);
            }

            // Register the collection in the global cache for EventLog consumer
            if let Some(collection) = response.collection {
                crate::services::events::log::register_collection_in_cache(
                    std::sync::Arc::new(collection)
                );
                tracing::info!(
                    "📦 EMBEDDED: Registered collection '{}' with index type '{}' for AXIS indexing",
                    name, index_type
                );
            }

            Ok(())
        })
    }

    /// Delete a collection
    pub fn delete_collection(
        &self,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let response = self.collection_service
                .delete_collection(name)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Check if the collection service returned an error in the response
            if !response.success {
                let error_msg = response.error_code.unwrap_or_else(|| "Unknown error".to_string());
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Failed to delete collection '{}': {}", name, error_msg)
                )) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(())
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
        self.search_with_mode(collection, query_vector, top_k, _filter, None)
    }

    /// Search for similar vectors with explicit search mode
    ///
    /// # Arguments
    /// * `collection` - Name of the collection to search
    /// * `query_vector` - Query vector
    /// * `top_k` - Number of results to return
    /// * `filter` - Optional filter expression
    /// * `search_mode` - Search mode: "exact", "approximate", or "adaptive"
    ///   - "exact": 100% recall, searches all partitions (default)
    ///   - "approximate": Faster search using IVF-style partition pruning
    ///   - "approximate:N": Approximate with explicit nprobe value
    ///   - "adaptive": Auto-select based on dataset size
    ///   - "adaptive:N": Adaptive with explicit threshold
    ///
    /// # Example
    /// ```rust,ignore
    /// // Exact search (100% recall)
    /// let results = db.search_with_mode("my_collection", vec![0.1; 768], 10, None, Some("exact"))?;
    ///
    /// // Approximate search (faster, ~95% recall)
    /// let results = db.search_with_mode("my_collection", vec![0.1; 768], 10, None, Some("approximate"))?;
    ///
    /// // Approximate with custom nprobe
    /// let results = db.search_with_mode("my_collection", vec![0.1; 768], 10, None, Some("approximate:5"))?;
    /// ```
    pub fn search_with_mode(
        &self,
        collection: &str,
        query_vector: Vec<f32>,
        top_k: usize,
        _filter: Option<&str>,
        search_mode: Option<&str>,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::search::SearchMode;
        use crate::services::operations::vectors::UnifiedSearchConfig;

        // Parse search mode string into SearchMode enum
        let mode = match search_mode {
            None | Some("exact") => SearchMode::Exact,
            Some("approximate") => SearchMode::Approximate { nprobe: None },
            Some(s) if s.starts_with("approximate:") => {
                let nprobe_str = s.strip_prefix("approximate:").unwrap_or("0");
                let nprobe = nprobe_str.parse::<usize>().ok();
                SearchMode::Approximate { nprobe }
            }
            Some("adaptive") => SearchMode::Adaptive { threshold: 10000 }, // Default threshold
            Some(s) if s.starts_with("adaptive:") => {
                let threshold_str = s.strip_prefix("adaptive:").unwrap_or("10000");
                let threshold = threshold_str.parse::<usize>().unwrap_or(10000);
                SearchMode::Adaptive { threshold }
            }
            Some(_) => SearchMode::Exact, // Default fallback
        };

        // Create config with the search mode
        let config = UnifiedSearchConfig {
            search_mode: mode,
            ..Default::default()
        };

        self.runtime.block_on(async {
            // For now, don't support filter expressions in embedded mode
            // TODO: Parse filter string into FilterExpression
            let results = self
                .shared_services.vector_operations_service
                .unified_search_native(collection, query_vector, top_k, None, Some(config))
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
                    disk_usage_bytes: 0, // TODO: Calculate actual disk usage
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
                        disk_usage_bytes: 0, // TODO: Calculate actual disk usage
                    }
                })
                .collect())
        })
    }

    /// Flush all pending writes to disk
    ///
    /// This forces all in-memory data (memtable/WAL) to be persisted to storage engine files.
    /// It also triggers compaction to consolidate data into SST files for durability.
    pub fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            use crate::storage::persistence::write_ahead_log::get_global_write_buffer_behavior;
            use crate::storage::traits::{FlushParameters, UnifiedStorageEngine};
            use crate::proto::proximadb_v1::{Collection, CollectionConfig, StorageAssignment};

            tracing::info!("🛑 EMBEDDED: Flushing all unflushed data to storage engines...");

            // Get the base storage URL from our embedded config
            let base_storage_url = if let Some(loc) = self.config.storage_locations.first() {
                loc.to_url()
            } else {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "No storage locations configured",
                )) as Box<dyn std::error::Error + Send + Sync>);
            };
            tracing::debug!("EMBEDDED: Using base storage URL: {}", base_storage_url);

            // Get the global write buffer to access unflushed data
            let write_buffer = match get_global_write_buffer_behavior() {
                Some(wb) => wb,
                None => {
                    tracing::info!("📋 EMBEDDED: No global write buffer initialized, nothing to flush");
                    return Ok(());
                }
            };

            // Get list of collections with unflushed data
            let collections_to_flush = write_buffer.list_collections_with_unflushed_data().await;
            if collections_to_flush.is_empty() {
                tracing::info!("📋 EMBEDDED: No collections have unflushed data");
                return Ok(());
            }

            tracing::info!(
                "🔄 EMBEDDED: Found {} collections with unflushed data: {:?}",
                collections_to_flush.len(),
                collections_to_flush
            );

            let mut total_vectors_flushed = 0u64;
            let mut total_bytes_written = 0u64;
            let mut collections_flushed = 0usize;
            let mut failed_collections: Vec<(String, String)> = Vec::new();

            // Flush each collection directly using its configured storage engine
            // Idempotency is handled at the batch level by get_unflushed_batches() and clear_flushed()
            for collection_id in &collections_to_flush {
                tracing::info!("🔄 EMBEDDED: Flushing collection '{}'", collection_id);

                // Get the collection's metadata to find its configured storage engine
                let collection_metadata = self.collection_service
                    .get_collection_with_tenant_context(collection_id, None)
                    .await;

                // Resolve the canonical collection ID (UUID), storage path, engine, and dimension.
                // WAL uses the human-friendly collection name, but SST storage is keyed by UUID.
                let mut canonical_collection_id = collection_id.clone();
                let mut collection_name = collection_id.clone();
                let mut base_location_for_flush = base_storage_url.clone();
                let mut storage_engine_type = crate::proto::proximadb_v1::StorageEngine::Sst as i32;
                let mut collection_dimension: u32 = 0;

                if let Ok(Some(coll)) = &collection_metadata {
                    canonical_collection_id = coll.id.clone();
                    if let Some(cfg) = &coll.config {
                        collection_name = cfg.name.clone();
                        collection_dimension = cfg.dimension;
                        storage_engine_type = cfg
                            .storage_engine
                            .unwrap_or(crate::proto::proximadb_v1::StorageEngine::Sst as i32);
                    }

                    // Prefer the persisted storage assignment path so we flush into the same directory
                    if let Some(assign) = &coll.storage_assignment {
                        base_location_for_flush = assign.base_location.clone();
                        storage_engine_type = assign.engine;
                    }
                }

                // Create the correct storage engine for this collection
                let proto_engine = crate::proto::proximadb_v1::StorageEngine::try_from(storage_engine_type)
                    .unwrap_or(crate::proto::proximadb_v1::StorageEngine::Sst);

                let engine_name = format!("{:?}", proto_engine);
                tracing::info!(
                    "🔧 EMBEDDED: Collection '{}' uses {} engine",
                    collection_id, engine_name
                );

                let storage_engine = match crate::storage::engines::factory::StorageEngineFactory::create_from_proto_async(proto_engine).await {
                    Ok(engine) => engine,
                    Err(e) => {
                        tracing::warn!(
                            "❌ EMBEDDED: Failed to create {} engine for '{}': {}, falling back to SST",
                            engine_name, collection_id, e
                        );
                        // Fallback to unified SST engine
                        self.shared_services.vector_operations_service.unified_engine()
                    }
                };

                // Get unflushed batches for this collection
                match write_buffer.get_unflushed_batches(collection_id).await {
                    Ok(batches) => {
                        if batches.is_empty() {
                            tracing::debug!("📋 EMBEDDED: Collection '{}' has no unflushed batches", collection_id);
                            continue;
                        }

                        // Combine all vector records from unflushed batches
                        let vector_records: Vec<crate::proto::proximadb_v1::VectorRecord> = batches
                            .iter()
                            .flat_map(|batch| batch.vector_records.iter().cloned())
                            .collect();

                        let vector_count = vector_records.len();
                        tracing::info!(
                            "📋 EMBEDDED: Collection '{}' has {} vectors to flush from {} batches using {} engine",
                            collection_id, vector_count, batches.len(), engine_name
                        );

                        // Create a collection config with the correct storage assignment, engine type, and dimension
                        // This ensures the flush writes to our embedded data directory with correct format
                        // IMPORTANT: dimension is required for VIPER/NOVA flush to work properly
                        let collection_config = Collection {
                            id: canonical_collection_id.clone(),
                            storage_assignment: Some(StorageAssignment {
                                base_location: base_location_for_flush.clone(),
                                engine: storage_engine_type, // Pass the correct engine type
                                ..Default::default()
                            }),
                            config: Some(CollectionConfig {
                                name: collection_name.clone(),
                                storage_engine: Some(storage_engine_type), // Set engine in config too
                                dimension: collection_dimension, // CRITICAL: VIPER/NOVA require dimension for flush
                                ..Default::default()
                            }),
                            ..Default::default()
                        };

                        // Create flush parameters with the correct storage path
                        let flush_params = FlushParameters {
                            // Use the canonical UUID for on-disk layout while keeping WAL cleanup keyed by name
                            collection_id: Some(canonical_collection_id.clone()),
                            force: true,
                            synchronous: true,
                            vector_records,
                            // Propagate batch IDs so the engine can report/trace flushed batches
                            batch_ids: batches.iter().map(|b| b.batch_id.clone()).collect(),
                            collection_config: Some(collection_config),
                            ..Default::default()
                        };

                        // Execute flush via the public flush() method which includes validation
                        // and post-processing (do_flush is internal implementation)
                        match storage_engine.flush(flush_params).await {
                            Ok(result) => {
                                let entries = result.entries_flushed.unwrap_or(0) as u64;
                                let bytes = result.bytes_written.unwrap_or(0) as u64;

                                total_vectors_flushed += entries;
                                total_bytes_written += bytes;
                                collections_flushed += 1;

                                // Clear flushed batches from memtable (synchronous - eager cleanup)
                                if let Err(e) = write_buffer.clear_flushed(collection_id).await {
                                    tracing::warn!(
                                        "⚠️ EMBEDDED: Failed to clear flushed batches for '{}': {}",
                                        collection_id, e
                                    );
                                }

                                // Delete WAL files from disk after successful flush
                                // This prevents 2x storage overhead from keeping WAL + SST files
                                let batch_id_strings: Vec<String> = batches.iter()
                                    .map(|b| b.batch_id.to_base62())
                                    .collect();

                                if !batch_id_strings.is_empty() {
                                    match crate::storage::persistence::write_ahead_log::manifest::mark_flushed_and_delete_files(&batch_id_strings).await {
                                        Ok(deleted) => {
                                            tracing::info!(
                                                "🗑️ EMBEDDED: Deleted {} WAL files for collection '{}'",
                                                deleted, collection_id
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "⚠️ EMBEDDED: Failed to delete WAL files for '{}': {}",
                                                collection_id, e
                                            );
                                        }
                                    }
                                }

                                // Update collection stats after successful flush
                                // This is CRITICAL for query optimizer to know dataset size
                                // Without this, optimizer skips index lookup due to "0 vectors"
                                if let Err(e) = self.shared_services.collection_service
                                    .update_stats(&collection_name, vector_count as i64, bytes as i64)
                                    .await
                                {
                                    tracing::warn!(
                                        "⚠️ EMBEDDED: Failed to update stats for '{}': {}",
                                        collection_id, e
                                    );
                                } else {
                                    tracing::debug!(
                                        "📊 EMBEDDED: Updated stats for '{}': +{} vectors, +{} bytes",
                                        collection_id, vector_count, bytes
                                    );
                                    // Invalidate the collection cache so the next search loads fresh stats
                                    // This ensures query optimizer sees the updated vector_count
                                    self.shared_services.vector_operations_service
                                        .invalidate_collection_cache(&collection_name);
                                }

                                tracing::info!(
                                    "✅ EMBEDDED: Flushed collection '{}': {} vectors, {} bytes",
                                    collection_id, entries, bytes
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "❌ EMBEDDED: Failed to flush collection '{}': {}",
                                    collection_id, e
                                );
                                failed_collections.push((collection_id.clone(), e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "❌ EMBEDDED: Failed to get unflushed batches for '{}': {}",
                            collection_id, e
                        );
                        failed_collections.push((collection_id.clone(), e.to_string()));
                    }
                }
            }

            tracing::info!(
                "🛑 EMBEDDED: Flush complete - {} collections, {} vectors, {} bytes{}",
                collections_flushed,
                total_vectors_flushed,
                total_bytes_written,
                if failed_collections.is_empty() {
                    String::new()
                } else {
                    format!(", {} failures", failed_collections.len())
                }
            );

            // NOTE: We removed the redundant force_flush_all() call here.
            // The per-collection storage_engine.flush() above already flushes all data.
            // Calling force_flush_all() caused duplicate SST files (2x data overhead).

            if failed_collections.is_empty() {
                Ok(())
            } else {
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to flush {} collections: {:?}", failed_collections.len(), failed_collections),
                )) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
    }

    // ========================================================================
    // Vector CRUD Operations - Phase 1: GET
    // ========================================================================

    /// Get a vector by ID from a collection
    ///
    /// Returns the vector record with id, vector data, and metadata if found.
    /// Searches both unflushed data (WAL/memtable) and flushed storage (SST files).
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `vector_id` - ID of the vector to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(VectorRecord))` - Vector found
    /// * `Ok(None)` - Vector not found
    /// * `Err` - Error occurred during lookup
    ///
    /// # Example
    /// ```rust,ignore
    /// let record = db.get_vector("embeddings", "vec_123")?;
    /// if let Some(vec) = record {
    ///     println!("Found vector: {:?}", vec.vector);
    /// }
    /// ```
    pub fn get_vector(
        &self,
        collection: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::storage::persistence::write_ahead_log::get_global_write_buffer_behavior;

        self.runtime.block_on(async {
            // Step 1: Check WAL/memtable for unflushed data (most recent)
            if let Some(write_buffer) = get_global_write_buffer_behavior() {
                if let Ok(batches) = write_buffer.get_unflushed_batches(collection).await {
                    for batch in batches {
                        for record in batch.vector_records.iter() {
                            if record.id == vector_id {
                                // Found in unflushed data - check if it's a tombstone
                                if record.vector.is_empty() && record.version.is_none() {
                                    // This is a tombstone marker - vector was deleted
                                    return Ok(None);
                                }
                                return Ok(Some(record.clone()));
                            }
                        }
                    }
                }
            }

            // Step 2: Search in flushed storage using the unified storage engine
            // Use a filter-based search to find the specific vector by ID
            let results = self
                .shared_services.vector_operations_service
                .unified_search_by_id(collection, vector_id)
                .await;

            match results {
                Ok(Some(record)) => {
                    // Check if it's a tombstone
                    if record.vector.is_empty() && record.version.is_none() {
                        Ok(None)
                    } else {
                        Ok(Some(record))
                    }
                }
                Ok(None) => Ok(None),
                Err(e) => Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get vector: {}", e),
                )) as Box<dyn std::error::Error + Send + Sync>),
            }
        })
    }

    /// Check if a vector exists in a collection
    ///
    /// This is a fast existence check that uses bloom filters when available.
    /// More efficient than `get_vector` when you only need to check existence.
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `vector_id` - ID of the vector to check
    ///
    /// # Returns
    /// * `true` - Vector exists
    /// * `false` - Vector does not exist
    pub fn vector_exists(
        &self,
        collection: &str,
        vector_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // For now, use get_vector and check if Some
        // In the future, this could use bloom filter for faster negative checks
        Ok(self.get_vector(collection, vector_id)?.is_some())
    }

    /// Delete a single vector by ID (tombstone-based)
    ///
    /// This uses tombstone markers to logically delete the vector. The tombstone
    /// will be written to the WAL and will shadow the original vector in searches.
    /// Physical deletion happens during compaction.
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `vector_id` - ID of the vector to delete
    ///
    /// # Returns
    /// * `true` - Tombstone was written (doesn't guarantee vector existed)
    /// * `false` - Failed to write tombstone
    ///
    /// # Example
    /// ```rust,ignore
    /// let deleted = db.delete_vector("embeddings", "vec_123")?;
    /// ```
    pub fn delete_vector(
        &self,
        collection: &str,
        vector_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::VectorRecord;
        use std::sync::Arc;

        // Create tombstone record: empty vector + expires_at in past marks deletion
        // expires_at is set to 0 (epoch) to immediately mark as expired/deleted
        // Compaction will clean up these tombstones after they've been applied
        let now = chrono::Utc::now();
        let tombstone = VectorRecord {
            id: vector_id.to_string(),
            vector: vec![],  // Empty vector = tombstone marker
            metadata: std::collections::HashMap::new(),
            timestamp: Some(now.timestamp_millis()),
            updated_at: Some(now.timestamp_millis()),
            expires_at: Some(0), // Expired at epoch = tombstone marker (always in past)
            version: None,  // Version may be updated by MVCC later
            source: None,
        };

        let records = Arc::new(vec![tombstone]);

        self.runtime.block_on(async {
            self.shared_services
                .vector_operations_service
                .insert_vectors_direct(collection, records)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;
            Ok(true)
        })
    }

    /// Delete multiple vectors by IDs (batch tombstone operation)
    ///
    /// More efficient than calling `delete_vector` multiple times.
    /// All tombstones are written in a single batch operation.
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `vector_ids` - Vector of IDs to delete
    ///
    /// # Returns
    /// Number of tombstones written (equals input count)
    ///
    /// # Example
    /// ```rust,ignore
    /// let count = db.delete_vectors("embeddings", vec!["vec_1".to_string(), "vec_2".to_string()])?;
    /// assert_eq!(count, 2);
    /// ```
    pub fn delete_vectors(
        &self,
        collection: &str,
        vector_ids: Vec<String>,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::VectorRecord;
        use std::sync::Arc;

        if vector_ids.is_empty() {
            return Ok(0);
        }

        let now = chrono::Utc::now();

        // Create tombstone records for all IDs
        // expires_at is set to 0 (epoch) to immediately mark as expired/deleted
        let tombstones: Vec<VectorRecord> = vector_ids
            .iter()
            .map(|id| VectorRecord {
                id: id.clone(),
                vector: vec![],  // Empty vector = tombstone marker
                metadata: std::collections::HashMap::new(),
                timestamp: Some(now.timestamp_millis()),
                updated_at: Some(now.timestamp_millis()),
                expires_at: Some(0), // Expired at epoch = tombstone marker (always in past)
                version: None,  // Version may be updated by MVCC later
                source: None,
            })
            .collect();

        let count = tombstones.len();
        let records = Arc::new(tombstones);

        self.runtime.block_on(async {
            self.shared_services
                .vector_operations_service
                .insert_vectors_direct(collection, records)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;
            Ok(count)
        })
    }

    /// Upsert vectors (insert or update)
    ///
    /// This is an atomic operation that:
    /// 1. Checks which IDs already exist
    /// 2. Deletes existing vectors (creates tombstones)
    /// 3. Inserts all vectors as new records
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `ids` - Vector IDs
    /// * `vectors` - Vector data
    /// * `metadata` - Optional metadata for each vector
    ///
    /// # Returns
    /// Tuple of (inserted_count, updated_count)
    ///
    /// # Example
    /// ```rust,ignore
    /// let (inserted, updated) = db.upsert(
    ///     "embeddings",
    ///     vec!["vec_1".to_string(), "vec_2".to_string()],
    ///     vec![vec![0.1; 768], vec![0.2; 768]],
    ///     None,
    /// )?;
    /// println!("Inserted: {}, Updated: {}", inserted, updated);
    /// ```
    pub fn upsert(
        &self,
        collection: &str,
        ids: Vec<String>,
        vectors: Vec<Vec<f32>>,
        metadata: Option<Vec<std::collections::HashMap<String, serde_json::Value>>>,
    ) -> Result<(usize, usize), Box<dyn std::error::Error + Send + Sync>> {
        if ids.is_empty() {
            return Ok((0, 0));
        }

        if ids.len() != vectors.len() {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("IDs count ({}) must match vectors count ({})", ids.len(), vectors.len()),
            )) as Box<dyn std::error::Error + Send + Sync>);
        }

        // Check which IDs already exist
        let mut existing_ids = Vec::new();
        let mut inserted = 0;
        let mut updated = 0;

        for id in &ids {
            if self.vector_exists(collection, id)? {
                existing_ids.push(id.clone());
                updated += 1;
            } else {
                inserted += 1;
            }
        }

        // Delete existing vectors (creates tombstones)
        if !existing_ids.is_empty() {
            self.delete_vectors(collection, existing_ids)?;
        }

        // Insert all vectors as new records
        self.insert(collection, ids, vectors, metadata)?;

        Ok((inserted, updated))
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

    /// Close the embedded database gracefully
    ///
    /// This method should be called before dropping the database to:
    /// 1. Persist the RL planner policy to disk
    /// 2. Log final statistics
    ///
    /// Note: Calling `flush()` before `close()` is recommended to ensure
    /// all vector data is persisted.
    ///
    /// # Example
    /// ```rust,ignore
    /// let db = EmbeddedProximaDB::new(config)?;
    /// // ... use the database ...
    /// db.flush()?;  // Persist vector data
    /// db.close();   // Persist RL policy and cleanup
    /// ```
    pub fn close(&self) {
        // Persist RL planner policy if enabled
        if let Some(ref policy_path) = self.rl_policy_path {
            if let Some(planner) = crate::query::rl_planner::get_rl_planner() {
                self.runtime.block_on(async {
                    match planner.save_policy(policy_path).await {
                        Ok(()) => {
                            tracing::info!("🎯 EMBEDDED: RL policy persisted to {}", policy_path);
                        }
                        Err(e) => {
                            tracing::warn!("EMBEDDED: Failed to persist RL policy: {}", e);
                        }
                    }

                    // Log final stats
                    let stats = planner.get_action_stats().await;
                    if !stats.is_empty() {
                        tracing::info!(
                            "🎯 EMBEDDED: RL Planner final stats - {} actions tracked",
                            stats.len()
                        );
                    }
                });
            }
        }

        tracing::info!("🛑 EMBEDDED: Database closed");
    }

    // ========================================================================
    // Generic Graph Operations API
    // ========================================================================

    /// Create a graph collection
    ///
    /// A graph must be created before nodes and edges can be added.
    pub fn create_graph(
        &self,
        graph_id: &str,
        engine: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            let engine_config = engine.map(|e| crate::proto::proximadb_v1::GraphEngineConfig {
                engine_type: e.to_string(),
                memory_pool_size_mb: 0,
                csr_cache_size_mb: 0,
                enable_parallel_operations: true,
                max_traversal_depth: 10,
                advanced_config: std::collections::HashMap::new(),
            });

            let request = crate::proto::proximadb_v1::CreateGraphRequest {
                graph_id: graph_id.to_string(),
                name: Some(graph_id.to_string()),
                description: None,
                schema: None,
                storage_config: None,
                engine_config,
                access_control: None,
            };

            graph_service
                .create_graph_collection(request)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(())
        })
    }

    /// Create nodes in the graph
    ///
    /// Inserts nodes with their properties. Use labels and properties
    /// for domain-specific categorization and attributes.
    pub fn create_nodes(
        &self,
        graph_id: &str,
        nodes: Vec<GraphNode>,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            // Use batch API for optimal performance (100-500x faster than individual inserts)
            let proto_nodes: Vec<_> = nodes.into_iter().map(|n| n.to_proto()).collect();
            graph_service
                .batch_create_nodes(graph_id, proto_nodes)
                .await
                .map(|inserted| inserted.len())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })
        })
    }

    /// Create edges in the graph
    pub fn create_edges(
        &self,
        graph_id: &str,
        edges: Vec<GraphEdge>,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let _count = edges.len(); // Tracking original count for potential logging

        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            let proto_edges: Vec<_> = edges.into_iter().map(|e| e.to_proto()).collect();
            graph_service
                .batch_create_edges(graph_id, proto_edges)
                .await
                .map(|inserted| inserted.len())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })
        })
    }

    /// Get a node by ID
    pub fn get_node(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<Option<GraphNode>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;
            let node_id_string = node_id.to_string();

            let proto_node = graph_service
                .get_node(graph_id, &node_id_string)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(proto_node.map(|n| GraphNode::from_proto(&n)))
        })
    }

    /// Query nodes by labels
    pub fn query_nodes_by_labels(
        &self,
        graph_id: &str,
        labels: Vec<String>,
    ) -> Result<Vec<GraphNode>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            let node_query = crate::proto::proximadb_v1::NodeQuery {
                labels,
                ..Default::default()
            };

            let proto_nodes = graph_service
                .query_nodes(graph_id, node_query)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(proto_nodes.into_iter().map(|n| GraphNode::from_proto(&n)).collect())
        })
    }

    /// Get outgoing edges from a node
    pub fn get_outgoing_edges(
        &self,
        graph_id: &str,
        node_id: &str,
        edge_types: Option<Vec<String>>,
    ) -> Result<Vec<GraphEdge>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            let edge_query = crate::proto::proximadb_v1::EdgeQuery {
                from_node_id: Some(node_id.to_string()),
                to_node_id: None,
                edge_types: edge_types.unwrap_or_default(),
                ..Default::default()
            };

            let proto_edges = graph_service
                .query_edges(graph_id, edge_query)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(proto_edges.into_iter().map(|e| GraphEdge::from_proto(&e)).collect())
        })
    }

    /// Get incoming edges to a node
    pub fn get_incoming_edges(
        &self,
        graph_id: &str,
        node_id: &str,
        edge_types: Option<Vec<String>>,
    ) -> Result<Vec<GraphEdge>, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            let edge_query = crate::proto::proximadb_v1::EdgeQuery {
                from_node_id: None,
                to_node_id: Some(node_id.to_string()),
                edge_types: edge_types.unwrap_or_default(),
                ..Default::default()
            };

            let proto_edges = graph_service
                .query_edges(graph_id, edge_query)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(proto_edges.into_iter().map(|e| GraphEdge::from_proto(&e)).collect())
        })
    }

    /// Delete a node and its edges
    pub fn delete_node(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;
            let node_id_string = node_id.to_string();

            let deleted = graph_service
                .delete_node(graph_id, &node_id_string)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(deleted.is_some())
        })
    }

    /// Get graph statistics
    pub fn graph_stats(
        &self,
        graph_id: &str,
    ) -> Result<GraphStats, Box<dyn std::error::Error + Send + Sync>> {
        self.runtime.block_on(async {
            let graph_service = &self.shared_services.graph_service;

            let proto_stats = graph_service
                .get_stats(graph_id)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(GraphStats {
                total_nodes: proto_stats.total_nodes,
                total_edges: proto_stats.total_edges,
            })
        })
    }

    /// Delete entire graph
    pub fn delete_graph(
        &self,
        graph_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // remove_graph returns Option, not Result - it's always successful
        let _ = self.shared_services.graph_service.remove_graph(graph_id);
        Ok(())
    }

    // ========================================================================
    // Document Store Operations (MongoDB-like JSON documents)
    // ========================================================================

    /// Create a document collection for storing JSON documents
    ///
    /// # Arguments
    /// * `name` - Collection name
    /// * `indexes` - Optional list of JSON path expressions to index for faster queries
    ///
    /// # Example
    /// ```rust,ignore
    /// db.create_document_collection("products", Some(vec!["$.category", "$.price"]))?;
    /// ```
    pub fn create_document_collection(
        &self,
        name: &str,
        indexes: Option<Vec<String>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{DocIndexType, DocumentCollectionConfig, IndexDefinition};
        use crate::storage::document::DocumentService;

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            // Build index definitions from paths
            let index_defs: Vec<IndexDefinition> = indexes
                .unwrap_or_default()
                .into_iter()
                .map(|path| IndexDefinition {
                    name: Some(format!("idx_{}", path.replace("$.", "").replace('.', "_"))),
                    path: path.clone(),
                    index_type: DocIndexType::Btree as i32, // B-tree for range queries
                    unique: false,
                    ..Default::default()
                })
                .collect();

            let config = DocumentCollectionConfig {
                name: name.to_string(),
                indexes: index_defs,
                ..Default::default()
            };

            doc_service
                .create_collection(name, config)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            tracing::info!("📄 EMBEDDED: Created document collection '{}'", name);
            Ok(())
        })
    }

    /// Insert a JSON document into a collection
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `id` - Optional document ID (auto-generated UUID if not provided)
    /// * `document` - JSON document as serde_json::Value
    ///
    /// # Returns
    /// The document ID
    ///
    /// # Example
    /// ```rust,ignore
    /// let doc = serde_json::json!({
    ///     "name": "Widget",
    ///     "price": 99.99,
    ///     "tags": ["electronics", "sale"]
    /// });
    /// let id = db.insert_document("products", Some("prod_001"), doc)?;
    /// ```
    pub fn insert_document(
        &self,
        collection: &str,
        id: Option<&str>,
        document: serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::SqlObject;
        use crate::storage::document::DocumentService;

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            // Convert serde_json::Value to SqlObject
            let sql_object = Self::json_to_sql_object(&document);

            let record = doc_service
                .insert_document(collection, id, sql_object)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(record.id)
        })
    }

    /// Get a document by ID
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `id` - Document ID
    ///
    /// # Returns
    /// The document as serde_json::Value if found
    ///
    /// # Example
    /// ```rust,ignore
    /// if let Some(doc) = db.get_document("products", "prod_001")? {
    ///     println!("Product: {}", doc["name"]);
    /// }
    /// ```
    pub fn get_document(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::storage::document::DocumentService;

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            let record = doc_service
                .get_document(collection, id, None)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(record.map(|r| Self::sql_object_to_json(&r.document)))
        })
    }

    /// Query documents with a filter expression
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `filter` - JSON path filter expression (e.g., "$.price > 50 AND $.category = 'electronics'")
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    /// List of matching documents as serde_json::Value
    ///
    /// # Example
    /// ```rust,ignore
    /// let results = db.query_documents("products", "$.price > 50", 100)?;
    /// for doc in results {
    ///     println!("Found: {}", doc["name"]);
    /// }
    /// ```
    pub fn query_documents(
        &self,
        collection: &str,
        filter: &str,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{DocFilterCondition, DocFilterOperator, DocumentFilter};
        use crate::storage::document::{DocumentQueryParams, DocumentService};

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            // Parse filter expression into DocumentFilter
            let conditions = Self::parse_document_filter(filter);
            let doc_filter = if conditions.is_empty() {
                None
            } else {
                Some(DocumentFilter {
                    conditions,
                    or_filters: vec![],
                    and_filters: vec![],
                })
            };

            let params = DocumentQueryParams {
                filter: doc_filter,
                limit,
                ..Default::default()
            };

            let result = doc_service
                .query_documents(collection, params)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(result
                .documents
                .into_iter()
                .map(|r| Self::sql_object_to_json(&r.document))
                .collect())
        })
    }

    /// Update a document by ID with patch operations
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `id` - Document ID
    /// * `updates` - Map of field paths to new values
    ///
    /// # Example
    /// ```rust,ignore
    /// let mut updates = HashMap::new();
    /// updates.insert("$.price".to_string(), serde_json::json!(79.99));
    /// updates.insert("$.sale".to_string(), serde_json::json!(true));
    /// db.update_document("products", "prod_001", updates)?;
    /// ```
    pub fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::proto::proximadb_v1::{DocumentUpdate, UpdateOperation};
        use crate::storage::document::DocumentService;

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            // Convert updates to DocumentUpdate operations
            let doc_updates: Vec<DocumentUpdate> = updates
                .into_iter()
                .map(|(path, value)| DocumentUpdate {
                    operation: UpdateOperation::Set as i32,
                    path,
                    value: Some(Self::json_to_sql_value(&value)),
                })
                .collect();

            doc_service
                .update_document(collection, id, doc_updates, None)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(())
        })
    }

    /// Delete a document by ID
    ///
    /// # Arguments
    /// * `collection` - Collection name
    /// * `id` - Document ID
    ///
    /// # Returns
    /// true if document was deleted, false if not found
    ///
    /// # Example
    /// ```rust,ignore
    /// let deleted = db.delete_document("products", "prod_001")?;
    /// ```
    pub fn delete_document(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        use crate::storage::document::DocumentService;

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            doc_service
                .delete_document(collection, id)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })
        })
    }

    /// Delete a document collection
    ///
    /// # Arguments
    /// * `name` - Collection name
    ///
    /// # Returns
    /// true if collection was deleted, false if not found
    pub fn delete_document_collection(
        &self,
        name: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        use crate::storage::document::DocumentService;

        self.runtime.block_on(async {
            let engine = self.shared_services.vector_operations_service.unified_engine();
            let doc_service = DocumentService::new(engine);

            doc_service
                .delete_collection(name)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })
        })
    }

    // ========================================================================
    // Document Store Helper Methods
    // ========================================================================

    /// Convert serde_json::Value to SqlObject
    fn json_to_sql_object(value: &serde_json::Value) -> crate::proto::proximadb_v1::SqlObject {
        use crate::proto::proximadb_v1::SqlObject;

        let mut fields = std::collections::HashMap::new();
        if let serde_json::Value::Object(map) = value {
            for (key, val) in map {
                fields.insert(key.clone(), Self::json_to_sql_value(val));
            }
        }
        SqlObject { fields }
    }

    /// Convert serde_json::Value to SqlValue
    fn json_to_sql_value(value: &serde_json::Value) -> crate::proto::proximadb_v1::SqlValue {
        use crate::proto::proximadb_v1::{sql_value::Value, SqlArray, SqlObject, SqlValue};

        let inner = match value {
            serde_json::Value::Null => Value::NullValue(0),
            serde_json::Value::Bool(b) => Value::BoolValue(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int64Value(i)
                } else if let Some(f) = n.as_f64() {
                    Value::NumberValue(f)
                } else {
                    Value::StringValue(n.to_string())
                }
            }
            serde_json::Value::String(s) => Value::StringValue(s.clone()),
            serde_json::Value::Array(arr) => {
                let values: Vec<SqlValue> = arr.iter().map(Self::json_to_sql_value).collect();
                Value::ArrayValue(SqlArray { values })
            }
            serde_json::Value::Object(_) => {
                Value::ObjectValue(Self::json_to_sql_object(value))
            }
        };
        SqlValue { value: Some(inner) }
    }

    /// Convert SqlObject to serde_json::Value
    fn sql_object_to_json(obj: &crate::proto::proximadb_v1::SqlObject) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (key, value) in &obj.fields {
            map.insert(key.clone(), Self::sql_value_to_json(value));
        }
        serde_json::Value::Object(map)
    }

    /// Convert SqlValue to serde_json::Value
    fn sql_value_to_json(value: &crate::proto::proximadb_v1::SqlValue) -> serde_json::Value {
        use crate::proto::proximadb_v1::sql_value::Value;

        match &value.value {
            None | Some(Value::NullValue(_)) => serde_json::Value::Null,
            Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
            Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
            Some(Value::NumberValue(f)) => {
                serde_json::Number::from_f64(*f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
            Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(Value::ArrayValue(arr)) => {
                serde_json::Value::Array(arr.values.iter().map(Self::sql_value_to_json).collect())
            }
            Some(Value::ObjectValue(obj)) => Self::sql_object_to_json(obj),
            Some(Value::BytesValue(b)) => {
                // Encode binary as hex string
                let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                serde_json::Value::String(format!("0x{}", hex))
            }
        }
    }

    /// Parse a simple filter expression into DocumentFilter conditions
    fn parse_document_filter(filter: &str) -> Vec<crate::proto::proximadb_v1::DocFilterCondition> {
        use crate::proto::proximadb_v1::{DocFilterCondition, DocFilterOperator};

        let mut conditions = Vec::new();
        if filter.trim().is_empty() {
            return conditions;
        }

        // Split by AND for now (simplified parser)
        let parts: Vec<&str> = filter.split(" AND ").collect();
        for part in parts {
            let part = part.trim();

            // Try to parse operators: >=, <=, !=, =, >, <, CONTAINS
            let operators = [">=", "<=", "!=", "=", ">", "<", "CONTAINS"];
            for op_str in operators {
                if let Some(op_pos) = part.find(op_str) {
                    let path = part[..op_pos].trim().to_string();
                    let value_str = part[op_pos + op_str.len()..].trim();

                    let operator = match op_str {
                        "=" => DocFilterOperator::Eq as i32,
                        "!=" => DocFilterOperator::Ne as i32,
                        ">" => DocFilterOperator::Gt as i32,
                        ">=" => DocFilterOperator::Gte as i32,
                        "<" => DocFilterOperator::Lt as i32,
                        "<=" => DocFilterOperator::Lte as i32,
                        "CONTAINS" => DocFilterOperator::Contains as i32,
                        _ => DocFilterOperator::Eq as i32,
                    };

                    // Parse value
                    let value = Self::parse_filter_value(value_str);

                    conditions.push(DocFilterCondition {
                        path,
                        operator,
                        value: Some(value),
                        values: vec![], // Empty for non-IN operators
                    });
                    break;
                }
            }
        }

        conditions
    }

    /// Parse a value string into SqlValue
    fn parse_filter_value(value_str: &str) -> crate::proto::proximadb_v1::SqlValue {
        use crate::proto::proximadb_v1::{sql_value::Value, SqlValue};

        let value_str = value_str.trim();

        // Check for quoted string
        if (value_str.starts_with('\'') && value_str.ends_with('\''))
            || (value_str.starts_with('"') && value_str.ends_with('"'))
        {
            return SqlValue {
                value: Some(Value::StringValue(
                    value_str[1..value_str.len() - 1].to_string(),
                )),
            };
        }

        // Check for boolean
        if value_str == "true" {
            return SqlValue {
                value: Some(Value::BoolValue(true)),
            };
        }
        if value_str == "false" {
            return SqlValue {
                value: Some(Value::BoolValue(false)),
            };
        }

        // Check for integer
        if let Ok(i) = value_str.parse::<i64>() {
            return SqlValue {
                value: Some(Value::Int64Value(i)),
            };
        }

        // Check for float
        if let Ok(f) = value_str.parse::<f64>() {
            return SqlValue {
                value: Some(Value::NumberValue(f)),
            };
        }

        // Default to string
        SqlValue {
            value: Some(Value::StringValue(value_str.to_string())),
        }
    }

    // ========================================================================
    // Observability Operations (Cloud SIEM / Datadog-like logs & metrics)
    // ========================================================================

    /// Create an observability namespace for logs and metrics
    ///
    /// # Arguments
    /// * `name` - Namespace name
    /// * `retention_hours` - Optional data retention period in hours (default: 720 = 30 days)
    ///
    /// # Example
    /// ```rust,ignore
    /// db.create_observability_namespace("production", Some(720))?; // 30 days retention
    /// ```
    pub fn create_observability_namespace(
        &self,
        name: &str,
        retention_hours: Option<u64>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::observability::storage::ObservabilityStorage;
        use crate::proto::proximadb_v1::{ObservabilityNamespaceConfig, RetentionConfig};

        // Get base path for observability data
        let base_path = self.config.storage_locations
            .first()
            .map(|loc| format!("{}/observability", loc.path))
            .unwrap_or_else(|| "./data/observability".to_string());

        self.runtime.block_on(async {
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&base_path));

            // Default: 24h hot, 7 days warm, 30 days cold
            let retention_hours_val = retention_hours.unwrap_or(720);
            let retention_days = retention_hours_val / 24;
            let config = ObservabilityNamespaceConfig {
                name: name.to_string(),
                retention: Some(RetentionConfig {
                    hot_retention_hours: retention_hours_val.min(24),
                    warm_retention_days: retention_days.min(7),
                    cold_retention_days: retention_days,
                    archive_retention_days: 0, // No archive by default
                }),
                ..Default::default()
            };

            storage
                .create_namespace(name, &config)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            tracing::info!("📊 EMBEDDED: Created observability namespace '{}'", name);
            Ok(())
        })
    }

    /// Ingest log entries into a namespace
    ///
    /// # Arguments
    /// * `namespace` - Namespace name
    /// * `logs` - Log entries to ingest
    ///
    /// # Returns
    /// Number of logs ingested
    ///
    /// # Example
    /// ```rust,ignore
    /// let logs = vec![
    ///     EmbeddedLogEntry {
    ///         message: "User logged in".to_string(),
    ///         severity: "INFO".to_string(),
    ///         service: Some("auth".to_string()),
    ///         source: Some("api-gateway".to_string()),
    ///         fields: HashMap::new(),
    ///     },
    /// ];
    /// let count = db.ingest_logs("production", logs)?;
    /// ```
    pub fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<EmbeddedLogEntry>,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        use crate::observability::storage::ObservabilityStorage;
        use crate::proto::proximadb_v1::{LogEntry, Severity};

        let base_path = self.config.storage_locations
            .first()
            .map(|loc| format!("{}/observability", loc.path))
            .unwrap_or_else(|| "./data/observability".to_string());

        self.runtime.block_on(async {
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&base_path));

            // Convert EmbeddedLogEntry to proto LogEntry
            let proto_logs: Vec<LogEntry> = logs
                .into_iter()
                .map(|log| {
                    let severity = match log.severity.to_uppercase().as_str() {
                        "TRACE" => Severity::Trace,
                        "DEBUG" => Severity::Debug,
                        "INFO" => Severity::Info,
                        "WARN" | "WARNING" => Severity::Warn,
                        "ERROR" => Severity::Error,
                        "FATAL" | "CRITICAL" => Severity::Fatal,
                        _ => Severity::Unspecified,
                    };

                    LogEntry {
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        severity: severity as i32,
                        message: log.message,
                        fields: log.fields.into_iter().map(|(k, v)| {
                            (k, Self::json_to_sql_value(&v))
                        }).collect(),
                        source: log.source,
                        service: log.service,
                    }
                })
                .collect();

            let count = proto_logs.len() as u64;

            for log in proto_logs {
                storage
                    .write_log(namespace, &log)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                    })?;
            }

            Ok(count)
        })
    }

    /// Query logs from a namespace
    ///
    /// # Arguments
    /// * `namespace` - Namespace name
    /// * `query` - Query string (text search in message)
    /// * `start_time` - Start of time range (ISO 8601 or epoch millis)
    /// * `end_time` - End of time range (ISO 8601 or epoch millis)
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    /// List of matching log entries
    ///
    /// # Example
    /// ```rust,ignore
    /// let logs = db.query_logs(
    ///     "production",
    ///     Some("error"),
    ///     Some("2024-01-01T00:00:00Z"),
    ///     None,
    ///     100,
    /// )?;
    /// ```
    pub fn query_logs(
        &self,
        namespace: &str,
        query: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
        limit: u32,
    ) -> Result<Vec<EmbeddedLogEntry>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::observability::query::ObservabilityQueryEngine;
        use crate::observability::storage::ObservabilityStorage;
        use crate::observability::LogQueryParams;
        use crate::proto::proximadb_v1::Severity;

        let base_path = self.config.storage_locations
            .first()
            .map(|loc| format!("{}/observability", loc.path))
            .unwrap_or_else(|| "./data/observability".to_string());

        self.runtime.block_on(async {
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&base_path));
            let query_engine = ObservabilityQueryEngine::new(storage);

            let start_ns = Self::parse_time_to_nanos(start_time).unwrap_or(0);
            let end_ns = Self::parse_time_to_nanos(end_time).unwrap_or(i64::MAX);

            let params = LogQueryParams {
                start_time_ns: start_ns,
                end_time_ns: end_ns,
                query: query.map(|s| s.to_string()),
                severities: vec![],
                services: vec![],
                sources: vec![],
                limit,
                cursor: None,
            };

            let result = query_engine
                .query_logs(namespace, params)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Convert proto LogEntry to EmbeddedLogEntry
            let logs = result
                .logs
                .into_iter()
                .map(|log| {
                    let severity_str = match Severity::try_from(log.severity).unwrap_or(Severity::Unspecified) {
                        Severity::Trace => "TRACE",
                        Severity::Debug => "DEBUG",
                        Severity::Info => "INFO",
                        Severity::Warn => "WARN",
                        Severity::Error => "ERROR",
                        Severity::Fatal => "FATAL",
                        Severity::Unspecified => "UNKNOWN",
                    };

                    EmbeddedLogEntry {
                        message: log.message,
                        severity: severity_str.to_string(),
                        service: log.service,
                        source: log.source,
                        fields: log.fields.into_iter().map(|(k, v)| {
                            (k, Self::sql_value_to_json(&v))
                        }).collect(),
                    }
                })
                .collect();

            Ok(logs)
        })
    }

    /// Ingest metric samples into a namespace
    ///
    /// # Arguments
    /// * `namespace` - Namespace name
    /// * `metrics` - Metric samples to ingest
    ///
    /// # Returns
    /// Number of metrics ingested
    ///
    /// # Example
    /// ```rust,ignore
    /// let metrics = vec![
    ///     EmbeddedMetricSample {
    ///         name: "http_requests_total".to_string(),
    ///         value: 1234.0,
    ///         labels: [("endpoint".to_string(), "/api/v1/users".to_string())].into(),
    ///     },
    /// ];
    /// let count = db.ingest_metrics("production", metrics)?;
    /// ```
    pub fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<EmbeddedMetricSample>,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        use crate::observability::storage::ObservabilityStorage;
        use crate::proto::proximadb_v1::MetricSample;

        let base_path = self.config.storage_locations
            .first()
            .map(|loc| format!("{}/observability", loc.path))
            .unwrap_or_else(|| "./data/observability".to_string());

        self.runtime.block_on(async {
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&base_path));

            // Convert EmbeddedMetricSample to proto MetricSample
            let proto_metrics: Vec<MetricSample> = metrics
                .into_iter()
                .map(|m| MetricSample {
                    name: m.name,
                    timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                    value: m.value,
                    labels: m.labels,
                    ..Default::default()
                })
                .collect();

            let count = proto_metrics.len() as u64;

            for metric in proto_metrics {
                storage
                    .write_metric(namespace, &metric)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                    })?;
            }

            Ok(count)
        })
    }

    /// Aggregate metrics from a namespace
    ///
    /// # Arguments
    /// * `namespace` - Namespace name
    /// * `metric_name` - Name of the metric
    /// * `aggregation` - Aggregation function ("avg", "sum", "min", "max", "count", "p50", "p90", "p95", "p99")
    /// * `start_time` - Start of time range
    /// * `end_time` - End of time range
    /// * `step_seconds` - Resolution/step in seconds
    ///
    /// # Returns
    /// List of aggregated data points
    ///
    /// # Example
    /// ```rust,ignore
    /// let results = db.aggregate_metrics(
    ///     "production",
    ///     "http_latency_ms",
    ///     "avg",
    ///     Some("2024-01-01T00:00:00Z"),
    ///     None,
    ///     60, // 1 minute buckets
    /// )?;
    /// ```
    pub fn aggregate_metrics(
        &self,
        namespace: &str,
        metric_name: &str,
        aggregation: &str,
        start_time: Option<&str>,
        end_time: Option<&str>,
        step_seconds: u32,
    ) -> Result<Vec<EmbeddedDataPoint>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::observability::query::ObservabilityQueryEngine;
        use crate::observability::storage::ObservabilityStorage;
        use crate::observability::{MetricAggParams, MetricAggregation};

        let base_path = self.config.storage_locations
            .first()
            .map(|loc| format!("{}/observability", loc.path))
            .unwrap_or_else(|| "./data/observability".to_string());

        self.runtime.block_on(async {
            let storage = std::sync::Arc::new(ObservabilityStorage::new(&base_path));
            let query_engine = ObservabilityQueryEngine::new(storage);

            let start_ns = Self::parse_time_to_nanos(start_time).unwrap_or(0);
            let end_ns = Self::parse_time_to_nanos(end_time).unwrap_or(i64::MAX);

            let agg = match aggregation.to_lowercase().as_str() {
                "avg" | "average" => MetricAggregation::Avg,
                "sum" => MetricAggregation::Sum,
                "min" => MetricAggregation::Min,
                "max" => MetricAggregation::Max,
                "count" => MetricAggregation::Count,
                "rate" => MetricAggregation::Rate,
                "p50" => MetricAggregation::P50,
                "p90" => MetricAggregation::P90,
                "p95" => MetricAggregation::P95,
                "p99" => MetricAggregation::P99,
                _ => MetricAggregation::Avg,
            };

            let params = MetricAggParams {
                metric_name: metric_name.to_string(),
                start_time_ns: start_ns,
                end_time_ns: end_ns,
                aggregation: agg,
                step_seconds,
                label_filters: std::collections::HashMap::new(),
                group_by: vec![],
            };

            let result = query_engine
                .aggregate_metrics(namespace, params)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            // Flatten all series into data points
            let points: Vec<EmbeddedDataPoint> = result
                .series
                .into_iter()
                .flat_map(|series| {
                    series.points.into_iter().map(|p| EmbeddedDataPoint {
                        timestamp_ns: p.timestamp_ns,
                        value: p.value,
                    })
                })
                .collect();

            Ok(points)
        })
    }

    // ========================================================================
    // Observability Helper Methods
    // ========================================================================

    /// Parse time string to nanoseconds
    fn parse_time_to_nanos(time_str: Option<&str>) -> Option<i64> {
        let s = time_str?;

        // Try ISO 8601 format
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(dt.timestamp_nanos_opt().unwrap_or(0));
        }

        // Try epoch milliseconds
        if let Ok(ms) = s.parse::<i64>() {
            return Some(ms * 1_000_000); // Convert ms to ns
        }

        // Try relative time (e.g., "now-1h")
        if s.starts_with("now") {
            let now = chrono::Utc::now();
            if s == "now" {
                return Some(now.timestamp_nanos_opt().unwrap_or(0));
            }
            // Parse "now-1h", "now-30m", etc.
            if let Some(offset_str) = s.strip_prefix("now-") {
                if let Some(duration) = Self::parse_duration(offset_str) {
                    return Some((now - duration).timestamp_nanos_opt().unwrap_or(0));
                }
            }
        }

        None
    }

    /// Parse duration string (e.g., "1h", "30m", "1d")
    fn parse_duration(s: &str) -> Option<chrono::Duration> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let (num_str, unit) = s.split_at(s.len() - 1);
        let num: i64 = num_str.parse().ok()?;

        match unit {
            "s" => Some(chrono::Duration::seconds(num)),
            "m" => Some(chrono::Duration::minutes(num)),
            "h" => Some(chrono::Duration::hours(num)),
            "d" => Some(chrono::Duration::days(num)),
            "w" => Some(chrono::Duration::weeks(num)),
            _ => None,
        }
    }
}

// ============================================================================
// Embedded Mode Types for Observability
// ============================================================================

/// Log entry for embedded mode
#[derive(Debug, Clone)]
pub struct EmbeddedLogEntry {
    /// Log message
    pub message: String,
    /// Severity level (DEBUG, INFO, WARN, ERROR, FATAL)
    pub severity: String,
    /// Service name
    pub service: Option<String>,
    /// Source (hostname, component)
    pub source: Option<String>,
    /// Additional fields
    pub fields: std::collections::HashMap<String, serde_json::Value>,
}

impl EmbeddedLogEntry {
    /// Create a new log entry
    pub fn new(message: impl Into<String>, severity: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity: severity.into(),
            service: None,
            source: None,
            fields: std::collections::HashMap::new(),
        }
    }

    /// Set service name
    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    /// Set source
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Add a field
    pub fn with_field(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }
}

/// Metric sample for embedded mode
#[derive(Debug, Clone)]
pub struct EmbeddedMetricSample {
    /// Metric name
    pub name: String,
    /// Metric value
    pub value: f64,
    /// Labels (dimensions)
    pub labels: std::collections::HashMap<String, String>,
}

impl EmbeddedMetricSample {
    /// Create a new metric sample
    pub fn new(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            value,
            labels: std::collections::HashMap::new(),
        }
    }

    /// Add a label
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }
}

/// Data point from metric aggregation
#[derive(Debug, Clone)]
pub struct EmbeddedDataPoint {
    /// Timestamp in nanoseconds since epoch
    pub timestamp_ns: i64,
    /// Aggregated value
    pub value: f64,
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
