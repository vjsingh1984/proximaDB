//! Index Configuration System for ProximaDB
//!
//! This module provides comprehensive configuration for various indexing algorithms
//! and index update behaviors at the collection level.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Index update behavior modes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexUpdateMode {
    /// Block flush until index is updated (default for strong consistency)
    Synchronous,
    /// Return from flush immediately, update index in background
    Asynchronous,
    /// Sync for small batches, async for large batches
    Hybrid,
}

impl Default for IndexUpdateMode {
    fn default() -> Self {
        Self::Synchronous // Default to synchronous for data consistency
    }
}

/// HNSW algorithm configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HnswConfig {
    /// Number of bi-directional links for each node
    pub m: usize,
    /// Size of candidate set during construction
    pub ef_construction: usize,
    /// Search parameter for recall
    pub ef_search: usize,
    /// Maximum vectors per partition
    pub max_partition_size: usize,
    /// Dynamic parameter tuning
    pub adaptive_parameters: bool,
    /// SIMD optimizations
    pub use_simd: bool,
    /// Memory limit per partition (MB)
    pub memory_limit_mb: usize,
    /// Lazy loading for large partitions
    pub lazy_loading: bool,
    /// Connection pruning threshold (0 = disabled)
    pub prune_connections: usize,
    /// Level generation multiplier
    pub level_multiplier: f32,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            max_partition_size: 100_000,
            adaptive_parameters: true,
            use_simd: true,
            memory_limit_mb: 512,
            lazy_loading: true,
            prune_connections: 0,
            level_multiplier: 1.0 / 2.0_f32.ln(), // 1/ln(2)
        }
    }
}

/// IVF algorithm configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvfConfig {
    /// Number of clusters
    pub n_lists: usize,
    /// Number of clusters to search
    pub n_probe: usize,
    /// Bits for quantization
    pub quantization_bits: usize,
    /// Use product quantization
    pub use_pq: bool,
    /// PQ subspaces
    pub pq_subspaces: usize,
    /// Retrain on every insert
    pub train_on_insert: bool,
    /// Minimum size to trigger training
    pub min_train_size: usize,
}

impl Default for IvfConfig {
    fn default() -> Self {
        Self {
            n_lists: 1000, // Will be adjusted based on collection size
            n_probe: 1,
            quantization_bits: 8,
            use_pq: false,
            pq_subspaces: 8,
            train_on_insert: false,
            min_train_size: 1000,
        }
    }
}

/// Random projection types for LSH
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RandomProjection {
    Gaussian,
    Binary,
    Sparse,
}

impl Default for RandomProjection {
    fn default() -> Self {
        Self::Gaussian
    }
}

/// LSH algorithm configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LshConfig {
    /// Number of hash tables
    pub n_hash_tables: u32,
    /// Number of hash functions per table
    pub n_hash_functions: u32,
    /// Bucket width for LSH
    pub bucket_width: f32,
    /// Support binary vectors
    pub binary_vectors: bool,
    /// Maximum candidates to check during search
    pub max_candidates: u32,
    /// Random projection type
    pub projection: RandomProjection,
}

impl Default for LshConfig {
    fn default() -> Self {
        Self {
            n_hash_tables: 10,
            n_hash_functions: 8,
            bucket_width: 4.0,
            binary_vectors: false,
            max_candidates: 100,
            projection: RandomProjection::default(),
        }
    }
}

/// Comprehensive index configuration for collections
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Index update behavior
    pub update_mode: IndexUpdateMode,
    /// Timeout for async updates (ms)
    pub async_update_timeout_ms: Option<u64>,
    /// Batch size for async updates
    pub async_update_batch_size: Option<usize>,
    /// Background index optimization
    pub enable_background_optimization: bool,
    /// HNSW-specific configuration
    pub hnsw_config: Option<HnswConfig>,
    /// IVF-specific configuration
    pub ivf_config: Option<IvfConfig>,
    /// LSH-specific configuration
    pub lsh_config: Option<LshConfig>,
    /// Parallel index building
    pub build_concurrency: Option<usize>,
    /// Memory limit per index (MB)
    pub memory_limit_mb: Option<u64>,
    /// Index checkpoint frequency (ms)
    pub checkpoint_interval_ms: Option<u64>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            update_mode: IndexUpdateMode::default(),
            async_update_timeout_ms: Some(30000), // 30 seconds
            async_update_batch_size: Some(1000),
            enable_background_optimization: true,
            hnsw_config: None,                   // Will be set if HNSW is selected
            ivf_config: None,                    // Will be set if IVF is selected
            lsh_config: None,                    // Will be set if LSH is selected
            build_concurrency: None,             // Use system default
            memory_limit_mb: Some(1024),         // 1GB default
            checkpoint_interval_ms: Some(60000), // 1 minute
        }
    }
}

impl IndexConfig {
    /// Create IndexConfig from protobuf with smart defaults
    pub fn from_proto(proto: &crate::proto::proximadb_v1::IndexConfig) -> Result<Self> {
        let update_mode = match proto.update_mode.unwrap_or(0) {
            1 => IndexUpdateMode::Synchronous,
            2 => IndexUpdateMode::Asynchronous,
            3 => IndexUpdateMode::Hybrid,
            _ => IndexUpdateMode::Synchronous,
        };

        let hnsw_config = proto.hnsw_config.as_ref().map(|h| HnswConfig {
            m: h.m.unwrap_or(16) as usize,
            ef_construction: h.ef_construction.unwrap_or(200) as usize,
            ef_search: h.ef_search.unwrap_or(100) as usize,
            max_partition_size: h.max_partition_size.unwrap_or(100000) as usize,
            adaptive_parameters: h.adaptive_parameters.unwrap_or(true),
            use_simd: h.use_simd.unwrap_or(true),
            memory_limit_mb: h.memory_limit_mb.unwrap_or(512) as usize,
            lazy_loading: h.lazy_loading.unwrap_or(false),
            prune_connections: 0,  // Default value - field not in proto
            level_multiplier: 1.0, // Default value - field not in proto
        });

        let ivf_config = proto.ivf_config.as_ref().map(|i| IvfConfig {
            n_lists: i.n_lists.unwrap_or(1000) as usize,
            n_probe: i.n_probe.unwrap_or(10) as usize,
            quantization_bits: i.quantization_bits.unwrap_or(8) as usize,
            use_pq: i.use_pq.unwrap_or(false),
            pq_subspaces: i.pq_subspaces.unwrap_or(8) as usize,
            train_on_insert: i.train_on_insert.unwrap_or(false),
            min_train_size: i.min_train_size.unwrap_or(1000) as usize,
        });

        let lsh_config = proto.lsh_config.as_ref().map(|l| LshConfig {
            n_hash_tables: l.n_hash_tables.unwrap_or(10),
            n_hash_functions: l.n_hash_functions.unwrap_or(8),
            bucket_width: l.bucket_width.unwrap_or(4.0),
            binary_vectors: l.binary_vectors.unwrap_or(false),
            max_candidates: l.max_candidates.unwrap_or(100),
            projection: match l.projection.unwrap_or(0) {
                1 => RandomProjection::Binary,
                2 => RandomProjection::Sparse,
                _ => RandomProjection::Gaussian,
            },
        });

        Ok(Self {
            update_mode,
            async_update_timeout_ms: proto.async_update_timeout_ms.map(|t| t as u64),
            async_update_batch_size: proto.async_update_batch_size.map(|b| b as usize),
            enable_background_optimization: proto.enable_background_optimization.unwrap_or(true),
            hnsw_config,
            ivf_config,
            lsh_config,
            build_concurrency: proto.build_concurrency.map(|x| x as usize),
            memory_limit_mb: proto.memory_limit_mb.map(|x| x as u64),
            checkpoint_interval_ms: proto.checkpoint_interval_ms.map(|x| x as u64),
        })
    }

    /// Convert to protobuf
    pub fn to_proto(&self) -> crate::proto::proximadb_v1::IndexConfig {
        let update_mode = match self.update_mode {
            IndexUpdateMode::Synchronous => 1,
            IndexUpdateMode::Asynchronous => 2,
            IndexUpdateMode::Hybrid => 3,
        };

        let hnsw_config =
            self.hnsw_config
                .as_ref()
                .map(|h| crate::proto::proximadb_v1::HnswConfig {
                    m: Some(h.m as u32),
                    ef_construction: Some(h.ef_construction as u32),
                    ef_search: Some(h.ef_search as u32),
                    max_partition_size: Some(h.max_partition_size as u32),
                    adaptive_parameters: Some(h.adaptive_parameters),
                    use_simd: Some(h.use_simd),
                    memory_limit_mb: Some(h.memory_limit_mb as u32),
                    lazy_loading: Some(h.lazy_loading),
                    // prune_connections and level_multiplier not in proto definition
                });

        let ivf_config = self
            .ivf_config
            .as_ref()
            .map(|i| crate::proto::proximadb_v1::IvfConfig {
                n_lists: Some(i.n_lists as u32),
                n_probe: Some(i.n_probe as u32),
                quantization_bits: Some(i.quantization_bits as u32),
                use_pq: Some(i.use_pq),
                pq_subspaces: Some(i.pq_subspaces as u32),
                train_on_insert: Some(i.train_on_insert),
                min_train_size: Some(i.min_train_size as u32),
            });

        let lsh_config = self
            .lsh_config
            .as_ref()
            .map(|l| crate::proto::proximadb_v1::LshConfig {
                n_hash_tables: Some(l.n_hash_tables),
                n_hash_functions: Some(l.n_hash_functions),
                bucket_width: Some(l.bucket_width),
                binary_vectors: Some(l.binary_vectors),
                max_candidates: Some(l.max_candidates),
                projection: Some(match l.projection {
                    RandomProjection::Gaussian => 0,
                    RandomProjection::Binary => 1,
                    RandomProjection::Sparse => 2,
                }),
            });

        crate::proto::proximadb_v1::IndexConfig {
            index_name: "default".to_string(), // Default index name
            algorithm: match &self.hnsw_config {
                Some(_) => crate::proto::proximadb_v1::IndexingAlgorithm::Hnsw as i32,
                None => match &self.ivf_config {
                    Some(_) => crate::proto::proximadb_v1::IndexingAlgorithm::Ivf as i32,
                    None => match &self.lsh_config {
                        Some(_) => crate::proto::proximadb_v1::IndexingAlgorithm::Lsh as i32,
                        None => crate::proto::proximadb_v1::IndexingAlgorithm::Flat as i32,
                    },
                },
            },
            parameters: std::collections::HashMap::new(), // Empty parameters map
            enabled: Some(true),                          // Index enabled by default
            update_mode: Some(update_mode),
            async_update_timeout_ms: self.async_update_timeout_ms.map(|t| t as u32),
            async_update_batch_size: self.async_update_batch_size.map(|b| b as u32),
            enable_background_optimization: Some(self.enable_background_optimization),
            hnsw_config,
            ivf_config,
            flat_config: None, // Flat config is algorithm-specific, set when FLAT algorithm is selected
            pq_config: None,   // PQ config is algorithm-specific, set when PQ algorithm is selected
            annoy_config: None, // Annoy config is algorithm-specific, set when ANNOY algorithm is selected
            lsh_config,
            build_concurrency: Some(self.build_concurrency.map(|x| x as u32).unwrap_or(1)),
            memory_limit_mb: Some(self.memory_limit_mb.map(|x| x as u32).unwrap_or(512)),
            checkpoint_interval_ms: Some(
                self.checkpoint_interval_ms
                    .map(|x| x as u32)
                    .unwrap_or(30000),
            ),
            is_primary: Some(true),           // Default to primary index
            use_cases: vec![],                // Default empty use cases
            selectivity_threshold: Some(0.0), // Default no selectivity threshold
            use_quantization: Some(false),    // Default: no quantization
            quantization_override: None,      // Default: no override
            queue_representation: Some("auto".to_string()), // Default: auto-detect from queue
        }
    }

    /// Get configuration for specific algorithm
    pub fn algorithm_config(&self, algorithm: &str) -> HashMap<String, serde_json::Value> {
        let mut config = HashMap::new();

        match algorithm {
            "HNSW" => {
                if let Some(hnsw) = &self.hnsw_config {
                    config.insert("m".to_string(), serde_json::json!(hnsw.m));
                    config.insert(
                        "ef_construction".to_string(),
                        serde_json::json!(hnsw.ef_construction),
                    );
                    config.insert("ef_search".to_string(), serde_json::json!(hnsw.ef_search));
                    config.insert(
                        "max_partition_size".to_string(),
                        serde_json::json!(hnsw.max_partition_size),
                    );
                    config.insert(
                        "adaptive_parameters".to_string(),
                        serde_json::json!(hnsw.adaptive_parameters),
                    );
                    config.insert("use_simd".to_string(), serde_json::json!(hnsw.use_simd));
                    config.insert(
                        "memory_limit_mb".to_string(),
                        serde_json::json!(hnsw.memory_limit_mb),
                    );
                    config.insert(
                        "lazy_loading".to_string(),
                        serde_json::json!(hnsw.lazy_loading),
                    );
                    config.insert(
                        "prune_connections".to_string(),
                        serde_json::json!(hnsw.prune_connections),
                    );
                    config.insert(
                        "level_multiplier".to_string(),
                        serde_json::json!(hnsw.level_multiplier),
                    );
                }
            }
            "IVF" => {
                if let Some(ivf) = &self.ivf_config {
                    config.insert("n_lists".to_string(), serde_json::json!(ivf.n_lists));
                    config.insert("n_probe".to_string(), serde_json::json!(ivf.n_probe));
                    config.insert(
                        "quantization_bits".to_string(),
                        serde_json::json!(ivf.quantization_bits),
                    );
                    config.insert("use_pq".to_string(), serde_json::json!(ivf.use_pq));
                    config.insert(
                        "pq_subspaces".to_string(),
                        serde_json::json!(ivf.pq_subspaces),
                    );
                    config.insert(
                        "train_on_insert".to_string(),
                        serde_json::json!(ivf.train_on_insert),
                    );
                    config.insert(
                        "min_train_size".to_string(),
                        serde_json::json!(ivf.min_train_size),
                    );
                }
            }
            "LSH" => {
                if let Some(lsh) = &self.lsh_config {
                    config.insert(
                        "n_hash_tables".to_string(),
                        serde_json::json!(lsh.n_hash_tables),
                    );
                    config.insert(
                        "n_hash_functions".to_string(),
                        serde_json::json!(lsh.n_hash_functions),
                    );
                    config.insert(
                        "bucket_width".to_string(),
                        serde_json::json!(lsh.bucket_width),
                    );
                    config.insert(
                        "binary_vectors".to_string(),
                        serde_json::json!(lsh.binary_vectors),
                    );
                    config.insert(
                        "max_candidates".to_string(),
                        serde_json::json!(lsh.max_candidates),
                    );
                    config.insert(
                        "projection".to_string(),
                        serde_json::json!(format!("{:?}", lsh.projection)),
                    );
                }
            }
            _ => {}
        }

        // Add general config
        config.insert(
            "update_mode".to_string(),
            serde_json::json!(format!("{:?}", self.update_mode)),
        );
        config.insert(
            "enable_background_optimization".to_string(),
            serde_json::json!(self.enable_background_optimization),
        );

        if let Some(timeout) = self.async_update_timeout_ms {
            config.insert(
                "async_update_timeout_ms".to_string(),
                serde_json::json!(timeout),
            );
        }
        if let Some(batch_size) = self.async_update_batch_size {
            config.insert(
                "async_update_batch_size".to_string(),
                serde_json::json!(batch_size),
            );
        }
        if let Some(concurrency) = self.build_concurrency {
            config.insert(
                "build_concurrency".to_string(),
                serde_json::json!(concurrency),
            );
        }
        if let Some(memory_limit) = self.memory_limit_mb {
            config.insert(
                "memory_limit_mb".to_string(),
                serde_json::json!(memory_limit),
            );
        }
        if let Some(checkpoint) = self.checkpoint_interval_ms {
            config.insert(
                "checkpoint_interval_ms".to_string(),
                serde_json::json!(checkpoint),
            );
        }

        config
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        // Validate HNSW config
        if let Some(hnsw) = &self.hnsw_config {
            if hnsw.m == 0 {
                return Err(anyhow::anyhow!("HNSW m parameter must be greater than 0"));
            }
            if hnsw.ef_construction < hnsw.m {
                return Err(anyhow::anyhow!("HNSW ef_construction must be >= m"));
            }
            if hnsw.ef_search == 0 {
                return Err(anyhow::anyhow!("HNSW ef_search must be greater than 0"));
            }
            if hnsw.max_partition_size < 1000 {
                return Err(anyhow::anyhow!(
                    "HNSW max_partition_size should be at least 1000"
                ));
            }
            if hnsw.memory_limit_mb < 64 {
                return Err(anyhow::anyhow!(
                    "HNSW memory_limit_mb should be at least 64MB"
                ));
            }
            if hnsw.level_multiplier <= 0.0 {
                return Err(anyhow::anyhow!("HNSW level_multiplier must be positive"));
            }
        }

        // Validate IVF config
        if let Some(ivf) = &self.ivf_config {
            if ivf.n_lists == 0 {
                return Err(anyhow::anyhow!("IVF n_lists must be greater than 0"));
            }
            if ivf.n_probe == 0 {
                return Err(anyhow::anyhow!("IVF n_probe must be greater than 0"));
            }
            if ivf.n_probe > ivf.n_lists {
                return Err(anyhow::anyhow!(
                    "IVF n_probe cannot be greater than n_lists"
                ));
            }
            if ivf.quantization_bits == 0 || ivf.quantization_bits > 16 {
                return Err(anyhow::anyhow!(
                    "IVF quantization_bits must be between 1 and 16"
                ));
            }
            if ivf.use_pq && ivf.pq_subspaces == 0 {
                return Err(anyhow::anyhow!(
                    "IVF pq_subspaces must be greater than 0 when using PQ"
                ));
            }
            if ivf.min_train_size < 100 {
                return Err(anyhow::anyhow!("IVF min_train_size should be at least 100"));
            }
        }

        // Validate LSH config
        if let Some(lsh) = &self.lsh_config {
            if lsh.n_hash_tables == 0 {
                return Err(anyhow::anyhow!("LSH n_hash_tables must be greater than 0"));
            }
            if lsh.n_hash_functions == 0 {
                return Err(anyhow::anyhow!(
                    "LSH n_hash_functions must be greater than 0"
                ));
            }
            if lsh.bucket_width <= 0.0 {
                return Err(anyhow::anyhow!("LSH bucket_width must be positive"));
            }
            if lsh.max_candidates == 0 {
                return Err(anyhow::anyhow!("LSH max_candidates must be greater than 0"));
            }
            if lsh.n_hash_tables > 100 {
                return Err(anyhow::anyhow!(
                    "LSH n_hash_tables should not exceed 100 for reasonable performance"
                ));
            }
            if lsh.n_hash_functions > 32 {
                return Err(anyhow::anyhow!(
                    "LSH n_hash_functions should not exceed 32 for reasonable performance"
                ));
            }
        }

        // Validate general config
        if let Some(timeout) = self.async_update_timeout_ms {
            if timeout < 1000 {
                return Err(anyhow::anyhow!(
                    "async_update_timeout_ms should be at least 1000ms"
                ));
            }
        }

        if let Some(batch_size) = self.async_update_batch_size {
            if batch_size == 0 {
                return Err(anyhow::anyhow!(
                    "async_update_batch_size must be greater than 0"
                ));
            }
        }

        if let Some(concurrency) = self.build_concurrency {
            if concurrency == 0 {
                return Err(anyhow::anyhow!("build_concurrency must be greater than 0"));
            }
        }

        if let Some(memory_limit) = self.memory_limit_mb {
            if memory_limit < 64 {
                return Err(anyhow::anyhow!("memory_limit_mb should be at least 64MB"));
            }
        }

        if let Some(checkpoint) = self.checkpoint_interval_ms {
            if checkpoint < 1000 {
                return Err(anyhow::anyhow!(
                    "checkpoint_interval_ms should be at least 1000ms"
                ));
            }
        }

        Ok(())
    }

    /// Create optimal configuration for specific algorithm with smart defaults
    pub fn create_for_algorithm(algorithm: &str, collection_size_hint: Option<usize>) -> Self {
        let mut config = Self::default();

        match algorithm {
            "HNSW" => {
                let mut hnsw = HnswConfig::default();

                // Adjust parameters based on collection size
                if let Some(size) = collection_size_hint {
                    if size < 10_000 {
                        // Small collection: optimize for accuracy
                        hnsw.ef_construction = 300;
                        hnsw.ef_search = 100;
                        hnsw.m = 32;
                        hnsw.max_partition_size = 10_000;
                        hnsw.memory_limit_mb = 256;
                        config.update_mode = IndexUpdateMode::Synchronous; // Small data, sync is fine
                    } else if size < 100_000 {
                        // Medium collection: balanced
                        hnsw.ef_construction = 200;
                        hnsw.ef_search = 50;
                        hnsw.m = 16;
                        hnsw.max_partition_size = 50_000;
                        hnsw.memory_limit_mb = 512;
                        config.update_mode = IndexUpdateMode::Hybrid; // Balanced approach
                    } else {
                        // Large collection: optimize for speed and throughput
                        hnsw.ef_construction = 150;
                        hnsw.ef_search = 30;
                        hnsw.m = 12;
                        hnsw.max_partition_size = 50_000;
                        hnsw.memory_limit_mb = 1024;
                        config.update_mode = IndexUpdateMode::Asynchronous; // Large data, async preferred
                        config.async_update_timeout_ms = Some(60000); // 1 minute for large batches
                        config.async_update_batch_size = Some(5000); // Larger batches for efficiency
                    }
                }

                config.hnsw_config = Some(hnsw);
            }
            "IVF" => {
                let mut ivf = IvfConfig::default();

                // Adjust parameters based on collection size
                if let Some(size) = collection_size_hint {
                    // Rule of thumb: n_lists = sqrt(N)
                    ivf.n_lists = (size as f64).sqrt().ceil() as usize;
                    ivf.n_lists = ivf.n_lists.max(100).min(10_000);

                    if size < 10_000 {
                        // Small collection: simple setup
                        ivf.n_probe = 4;
                        ivf.quantization_bits = 8;
                        config.update_mode = IndexUpdateMode::Synchronous;
                    } else if size < 100_000 {
                        // Medium collection: balanced
                        ivf.n_probe = 8;
                        ivf.quantization_bits = 8;
                        config.update_mode = IndexUpdateMode::Hybrid;
                    } else {
                        // Large collection: optimize for memory and speed
                        ivf.n_probe = 16;
                        ivf.use_pq = true;
                        ivf.pq_subspaces = 8;
                        ivf.quantization_bits = 8;
                        config.update_mode = IndexUpdateMode::Asynchronous;
                        config.async_update_timeout_ms = Some(120000); // 2 minutes for IVF training
                        config.async_update_batch_size = Some(10000); // Large batches for IVF efficiency
                    }
                }

                config.ivf_config = Some(ivf);
            }
            "FLAT" => {
                // FLAT index: simple brute force, always synchronous
                config.update_mode = IndexUpdateMode::Synchronous;
                config.async_update_timeout_ms = Some(10000); // Short timeout
                config.async_update_batch_size = Some(100); // Small batches
                config.memory_limit_mb = Some(256); // Low memory for brute force
                config.enable_background_optimization = false; // No optimization for flat
            }
            "PQ" => {
                // Product Quantization: memory-efficient approximate search
                config.update_mode = IndexUpdateMode::Asynchronous; // PQ training is expensive

                if let Some(size) = collection_size_hint {
                    if size < 50_000 {
                        // Small collection: simpler PQ setup
                        config.async_update_timeout_ms = Some(60000); // 1 minute
                        config.async_update_batch_size = Some(1000);
                        config.memory_limit_mb = Some(512);
                    } else {
                        // Large collection: optimize for compression
                        config.async_update_timeout_ms = Some(300000); // 5 minutes for training
                        config.async_update_batch_size = Some(10000);
                        config.memory_limit_mb = Some(1024);
                    }
                }

                // PQ typically uses IVF as the coarse quantizer
                let mut ivf = IvfConfig::default();
                if let Some(size) = collection_size_hint {
                    ivf.n_lists = ((size as f64).sqrt() / 4.0).ceil() as usize;
                    ivf.n_lists = ivf.n_lists.max(50).min(1000);
                    ivf.n_probe = 4;
                    ivf.use_pq = true;
                    ivf.pq_subspaces = 8; // Common PQ8 configuration
                    ivf.quantization_bits = 8;
                }
                config.ivf_config = Some(ivf);
            }
            "ANNOY" => {
                // Annoy (Approximate Nearest Neighbors Oh Yeah): tree-based index
                config.update_mode = IndexUpdateMode::Synchronous; // Annoy builds are relatively fast

                if let Some(size) = collection_size_hint {
                    if size < 10_000 {
                        // Small collection: more trees for accuracy
                        config.async_update_timeout_ms = Some(20000); // 20 seconds
                        config.async_update_batch_size = Some(500);
                        config.memory_limit_mb = Some(256);
                        config.build_concurrency = Some(4); // Parallel tree building
                    } else if size < 100_000 {
                        // Medium collection: balanced
                        config.async_update_timeout_ms = Some(60000); // 1 minute
                        config.async_update_batch_size = Some(2000);
                        config.memory_limit_mb = Some(512);
                        config.build_concurrency = Some(8);
                    } else {
                        // Large collection: optimize for speed
                        config.update_mode = IndexUpdateMode::Asynchronous;
                        config.async_update_timeout_ms = Some(180000); // 3 minutes
                        config.async_update_batch_size = Some(5000);
                        config.memory_limit_mb = Some(1024);
                        config.build_concurrency = Some(16);
                    }
                }

                // Annoy-specific parameters could be added to a new AnnoyConfig struct
                // For now, we use general config parameters
                config.checkpoint_interval_ms = Some(30000); // More frequent checkpoints for tree-based
            }
            "LSH" => {
                // Locality-Sensitive Hashing: probabilistic indexing
                let mut lsh = LshConfig::default();

                // Adjust parameters based on collection size
                if let Some(size) = collection_size_hint {
                    if size < 10_000 {
                        // Small collection: fewer hash tables, more accurate
                        lsh.n_hash_tables = 5;
                        lsh.n_hash_functions = 8;
                        lsh.bucket_width = 2.0;
                        lsh.max_candidates = 200;
                        config.update_mode = IndexUpdateMode::Synchronous;
                        config.memory_limit_mb = Some(256);
                    } else if size < 100_000 {
                        // Medium collection: balanced setup
                        lsh.n_hash_tables = 10;
                        lsh.n_hash_functions = 10;
                        lsh.bucket_width = 4.0;
                        lsh.max_candidates = 150;
                        config.update_mode = IndexUpdateMode::Asynchronous;
                        config.async_update_batch_size = Some(1000);
                        config.memory_limit_mb = Some(512);
                    } else if size < 1_000_000 {
                        // Large collection: optimize for speed
                        lsh.n_hash_tables = 15;
                        lsh.n_hash_functions = 12;
                        lsh.bucket_width = 6.0;
                        lsh.max_candidates = 100;
                        config.update_mode = IndexUpdateMode::Asynchronous;
                        config.async_update_batch_size = Some(5000);
                        config.memory_limit_mb = Some(1024);
                    } else {
                        // Very large collection: maximum scalability
                        lsh.n_hash_tables = 20;
                        lsh.n_hash_functions = 16;
                        lsh.bucket_width = 8.0;
                        lsh.max_candidates = 50;
                        config.update_mode = IndexUpdateMode::Asynchronous;
                        config.async_update_batch_size = Some(10000);
                        config.memory_limit_mb = Some(2048);
                    }
                }

                config.lsh_config = Some(lsh);
            }
            _ => {
                // Unknown algorithm: conservative defaults
                config.update_mode = IndexUpdateMode::Synchronous;
            }
        }

        config
    }

    /// Create IndexConfig from protobuf with smart defaults and algorithm-aware filling
    pub fn from_proto_with_smart_defaults(
        proto: &crate::proto::proximadb_v1::IndexConfig,
        algorithm: &str,
        collection_size_hint: Option<usize>,
    ) -> Result<Self> {
        // Start with algorithm-specific optimal config
        let mut config = Self::create_for_algorithm(algorithm, collection_size_hint);

        // Override with user-provided values from proto
        if proto.update_mode.unwrap_or(0) != 0 {
            config.update_mode = match proto.update_mode.unwrap_or(0) {
                1 => IndexUpdateMode::Synchronous,
                2 => IndexUpdateMode::Asynchronous,
                3 => IndexUpdateMode::Hybrid,
                _ => config.update_mode, // Keep smart default
            };
        }

        // Apply user overrides while keeping smart defaults for missing values
        if let Some(timeout) = proto.async_update_timeout_ms {
            config.async_update_timeout_ms = Some(timeout as u64);
        }
        if let Some(batch_size) = proto.async_update_batch_size {
            config.async_update_batch_size = Some(batch_size as usize);
        }

        config.enable_background_optimization =
            proto.enable_background_optimization.unwrap_or(true);

        // Handle algorithm-specific overrides
        match algorithm {
            "HNSW" => {
                if let Some(user_hnsw) = &proto.hnsw_config {
                    if let Some(mut smart_hnsw) = config.hnsw_config.take() {
                        // Apply user overrides to smart defaults
                        if user_hnsw.m.unwrap_or(0) != 0 {
                            smart_hnsw.m = user_hnsw.m.unwrap_or(16) as usize;
                        }
                        if user_hnsw.ef_construction.unwrap_or(0) != 0 {
                            smart_hnsw.ef_construction =
                                user_hnsw.ef_construction.unwrap_or(200) as usize;
                        }
                        if user_hnsw.ef_search.unwrap_or(0) != 0 {
                            smart_hnsw.ef_search = user_hnsw.ef_search.unwrap_or(100) as usize;
                        }
                        if user_hnsw.max_partition_size.unwrap_or(0) != 0 {
                            smart_hnsw.max_partition_size =
                                user_hnsw.max_partition_size.unwrap_or(100000) as usize;
                        }
                        if user_hnsw.memory_limit_mb.unwrap_or(0) != 0 {
                            smart_hnsw.memory_limit_mb =
                                user_hnsw.memory_limit_mb.unwrap_or(512) as usize;
                        }
                        // prune_connections and level_multiplier are not in proto - use smart defaults

                        // Boolean fields: use user value if explicitly set, otherwise keep smart default
                        if let Some(adaptive) = user_hnsw.adaptive_parameters {
                            smart_hnsw.adaptive_parameters = adaptive;
                        }
                        if let Some(simd) = user_hnsw.use_simd {
                            smart_hnsw.use_simd = simd;
                        }
                        if let Some(lazy) = user_hnsw.lazy_loading {
                            smart_hnsw.lazy_loading = lazy;
                        }

                        config.hnsw_config = Some(smart_hnsw);
                    }
                }
            }
            "IVF" => {
                if let Some(user_ivf) = &proto.ivf_config {
                    if let Some(mut smart_ivf) = config.ivf_config.take() {
                        // Apply user overrides to smart defaults
                        if user_ivf.n_lists.unwrap_or(0) != 0 {
                            smart_ivf.n_lists = user_ivf.n_lists.unwrap_or(1000) as usize;
                        }
                        if user_ivf.n_probe.unwrap_or(0) != 0 {
                            smart_ivf.n_probe = user_ivf.n_probe.unwrap_or(10) as usize;
                        }
                        if user_ivf.quantization_bits.unwrap_or(0) != 0 {
                            smart_ivf.quantization_bits =
                                user_ivf.quantization_bits.unwrap_or(8) as usize;
                        }
                        if user_ivf.pq_subspaces.unwrap_or(0) != 0 {
                            smart_ivf.pq_subspaces = user_ivf.pq_subspaces.unwrap_or(8) as usize;
                        }
                        if user_ivf.min_train_size.unwrap_or(0) != 0 {
                            smart_ivf.min_train_size =
                                user_ivf.min_train_size.unwrap_or(1000) as usize;
                        }

                        // Boolean fields: use user value if explicitly set, otherwise keep smart default
                        if let Some(pq) = user_ivf.use_pq {
                            smart_ivf.use_pq = pq;
                        }
                        if let Some(train) = user_ivf.train_on_insert {
                            smart_ivf.train_on_insert = train;
                        }

                        config.ivf_config = Some(smart_ivf);
                    }
                }
            }
            _ => {}
        }

        // Apply general overrides
        if proto.build_concurrency.unwrap_or(0) > 0 {
            config.build_concurrency = Some(proto.build_concurrency.unwrap_or(1) as usize);
        }
        if proto.memory_limit_mb.unwrap_or(0) > 0 {
            config.memory_limit_mb = Some(proto.memory_limit_mb.unwrap_or(512) as u64);
        }
        if proto.checkpoint_interval_ms.unwrap_or(0) > 0 {
            config.checkpoint_interval_ms =
                Some(proto.checkpoint_interval_ms.unwrap_or(60000) as u64);
        }

        // Validate the final configuration
        config.validate()?;

        Ok(config)
    }

    /// Create IndexConfig with smart defaults when no config is provided
    pub fn create_smart_default(
        algorithm: &str,
        dimension: usize,
        collection_size_hint: Option<usize>,
    ) -> Self {
        let mut config = Self::create_for_algorithm(algorithm, collection_size_hint);

        // Adjust based on vector dimension
        if dimension > 1024 {
            // High-dimensional vectors: optimize for memory
            config.memory_limit_mb = Some(2048); // 2GB for high-dim vectors
            config.checkpoint_interval_ms = Some(30000); // More frequent checkpoints

            if let Some(hnsw) = &mut config.hnsw_config {
                hnsw.memory_limit_mb = 1024; // 1GB per partition
                hnsw.max_partition_size = 25_000; // Smaller partitions for high-dim
            }
        } else if dimension < 128 {
            // Low-dimensional vectors: optimize for speed
            config.memory_limit_mb = Some(512); // 512MB for low-dim vectors
            config.checkpoint_interval_ms = Some(120000); // Less frequent checkpoints

            if let Some(hnsw) = &mut config.hnsw_config {
                hnsw.memory_limit_mb = 256; // 256MB per partition
                hnsw.max_partition_size = 200_000; // Larger partitions for low-dim
            }
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_index_config() {
        let config = IndexConfig::default();
        assert_eq!(config.update_mode, IndexUpdateMode::Synchronous);
        assert_eq!(config.async_update_timeout_ms, Some(30000));
        assert_eq!(config.async_update_batch_size, Some(1000));
        assert!(config.enable_background_optimization);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_hnsw_config_validation() {
        let mut config = IndexConfig::default();
        config.hnsw_config = Some(HnswConfig {
            m: 0, // Invalid
            ..Default::default()
        });
        assert!(config.validate().is_err());

        config.hnsw_config = Some(HnswConfig {
            m: 16,
            ef_construction: 10, // Invalid: < m
            ..Default::default()
        });
        assert!(config.validate().is_err());

        config.hnsw_config = Some(HnswConfig::default());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_ivf_config_validation() {
        let mut config = IndexConfig::default();
        config.ivf_config = Some(IvfConfig {
            n_lists: 0, // Invalid
            ..Default::default()
        });
        assert!(config.validate().is_err());

        config.ivf_config = Some(IvfConfig {
            n_lists: 100,
            n_probe: 150, // Invalid: > n_lists
            ..Default::default()
        });
        assert!(config.validate().is_err());

        config.ivf_config = Some(IvfConfig::default());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_create_for_algorithm() {
        let small_config = IndexConfig::create_for_algorithm("HNSW", Some(5_000));
        assert!(small_config.hnsw_config.is_some());
        let hnsw = small_config.hnsw_config.unwrap();
        assert_eq!(hnsw.ef_construction, 300);
        assert_eq!(hnsw.m, 32);

        let large_config = IndexConfig::create_for_algorithm("HNSW", Some(500_000));
        assert!(large_config.hnsw_config.is_some());
        let hnsw = large_config.hnsw_config.unwrap();
        assert_eq!(hnsw.ef_construction, 150);
        assert_eq!(hnsw.m, 12);
        assert_eq!(hnsw.max_partition_size, 50_000);

        let ivf_config = IndexConfig::create_for_algorithm("IVF", Some(100_000));
        assert!(ivf_config.ivf_config.is_some());
        let ivf = ivf_config.ivf_config.unwrap();
        assert_eq!(ivf.n_lists, 317); // sqrt(100000) = 316.22, ceil(316.22) = 317
    }

    #[test]
    fn test_get_algorithm_config() {
        let mut config = IndexConfig::default();
        config.hnsw_config = Some(HnswConfig::default());

        let hnsw_config = config.algorithm_config("HNSW");
        assert!(hnsw_config.contains_key("m"));
        assert!(hnsw_config.contains_key("ef_construction"));
        assert!(hnsw_config.contains_key("update_mode"));

        let empty_config = config.algorithm_config("UNKNOWN");
        assert!(empty_config.contains_key("update_mode")); // Should contain general config
        assert!(!empty_config.contains_key("m")); // Should not contain HNSW-specific config
    }
}
