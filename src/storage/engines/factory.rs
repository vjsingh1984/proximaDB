//! Storage Engine Factory
//!
//! ## Purpose:
//!
//! The StorageEngineFactory is the central point for creating storage engine
//! instances in ProximaDB. It provides a unified interface for instantiating
//! any of the 6 storage engines based on configuration or strategy.
//!
//! ## Design Pattern:
//!
//! Factory pattern with lazy initialization. Engines are created on-demand
//! with appropriate configuration and dependencies injected.
//!
//! ## Engine Selection Matrix:
//!
//! | Strategy | Engine | Use Case | Format |
//! |----------|--------|----------|--------|
//! | Viper | VIPER | Analytics, batch | Columnar (Parquet) |
//! | Lsm | SST | OLTP, real-time | Hybrid columnar (ProximaBlocks) |
//! | Hybrid | RAPTOR | Graph navigation | Matrix Trinity |
//! | Swift | SWIFT | Fast traversal | Row-based optimized |
//! | Nova | NOVA | Advanced analytics | Enhanced columnar |
//! | Helix | HELIX | PCA+Hilbert | Dimension-reduced |
//!
//! ## Engine Maturity Levels:
//!
//! **Production-Ready (Recommended for production use):**
//! - **SST**: Most mature, write-optimized, 253+ tests (unit + async)
//! - **VIPER**: Production columnar engine, 120+ tests, Parquet-based
//!
//! **Production-Ready (Specialized use cases):**
//! - **NOVA**: Advanced columnar analytics, 66+ tests, zone maps & predicate pushdown
//! - **HELIX**: High-dimensional data, 38+ tests, PCA dimension reduction
//!
//! **Experimental (Requires `experimental-engines` feature flag):**
//! - **SWIFT**: Fast traversal, 41+ tests, some methods return errors
//! - **RAPTOR**: Matrix Trinity navigation, 23+ tests, several TODOs for optimization
//!
//! To enable experimental engines: `cargo build --features experimental-engines`
//!
//! ## Selection Criteria:
//!
//! Choose your engine based on these workload characteristics:
//!
//! ### Write-Heavy Workloads:
//! - **SST**: Best for high write throughput, real-time ingestion, streaming data
//! - Performance: ~5.32ms for 10K vectors with LZ4 compression
//!
//! ### Analytical Workloads:
//! - **VIPER**: Best for batch analytics, read-heavy, Parquet ecosystem integration
//! - **NOVA**: Best for advanced analytics with predicate pushdown, zone maps
//! - Performance: VIPER ~89.5ms, NOVA ~101.6ms for 10K vectors
//!
//! ### Point Lookup Workloads:
//! - **SWIFT**: Best for low-latency ID lookups (<5K vectors optimal)
//! - Performance: ~95ms for 10K vectors, optimized for cache-friendly access
//!
//! ### High-Dimensional Data:
//! - **HELIX**: Best for dimensions > 512, uses PCA reduction + Hilbert curves
//! - Performance: ~13.2ms for 10K vectors with locality optimization
//!
//! ### Mixed/Hybrid Workloads:
//! - **RAPTOR**: Adaptive row-group sizing, Matrix Trinity navigation
//! - Performance: ~9.36ms for 10K vectors with dynamic optimization
//!
//! ## Usage Examples:
//!
//! ### Basic Engine Creation:
//! ```rust,ignore
//! use crate::storage::engines::factory::StorageEngineFactory;
//! use crate::proto::proximadb_v1::StorageEngine;
//!
//! // Create from proto enum (API requests)
//! let engine = StorageEngineFactory::create_from_proto(StorageEngine::Sst)?;
//!
//! // Create async version for async contexts
//! let engine = StorageEngineFactory::create_sst_async().await?;
//! ```
//!
//! ### Workload-Based Selection:
//! ```rust,ignore
//! use crate::storage::engines::factory::{StorageEngineFactory, WorkloadType};
//!
//! // Automatic selection based on workload
//! let engine = StorageEngineFactory::create_for_workload(WorkloadType::Analytics)?;
//! ```
//!
//! ### Requirements-Based Recommendation:
//! ```rust,ignore
//! use crate::storage::engines::factory::{StorageEngineFactory, EngineRequirements};
//!
//! let requirements = EngineRequirements {
//!     needs_columnar: true,
//!     needs_predicate_pushdown: true,
//!     needs_batch_operations: true,
//!     ..Default::default()
//! };
//! let recommended = StorageEngineFactory::recommend_engine(&requirements);
//! let engine = StorageEngineFactory::create_from_proto(recommended)?;
//! ```

use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

use crate::metrics::collectors::EngineMetricsCollector;
use crate::proto::proximadb_v1::StorageEngine as ProtoStorageEngine;
use crate::storage::traits::{StorageEngineStrategy, UnifiedStorageEngine};

use super::impls::{nova::NovaEngine, sst::SstEngine, viper::ViperEngine};
#[cfg(feature = "experimental-engines")]
use super::impls::{raptor::RaptorEngine, swift::SwiftEngine};

/// Storage engine factory for creating engine instances
///
/// ## Responsibilities:
///
/// 1. **Engine Creation**: Instantiate appropriate engine based on config
/// 2. **Dependency Injection**: Provide filesystem, distance compute, caches
/// 3. **Async Bridging**: Handle async engine initialization in sync context
/// 4. **Error Reporting**: Return explicit errors for unimplemented/misconfigured engines
///
/// ## Thread Safety:
///
/// All created engines are Arc-wrapped and thread-safe, suitable for
/// concurrent access across multiple tokio tasks.
pub struct StorageEngineFactory;

impl StorageEngineFactory {
    /// Create a storage engine from proto enum
    ///
    /// ## Proto Mapping:
    ///
    /// Maps protobuf StorageEngine enum to concrete implementations.
    /// This is the primary interface for gRPC/REST API requests.
    ///
    /// ### Fallback Strategy:
    /// - Unspecified → SST (most general purpose)
    /// - Unimplemented → SST with warning
    /// - Unknown → SST as safe default
    pub fn create_from_proto(
        engine_type: ProtoStorageEngine,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match engine_type {
            ProtoStorageEngine::Unspecified => {
                warn!("Unspecified storage engine, defaulting to SST (VIPER not available)");
                Self::create_sst()
            }
            ProtoStorageEngine::Viper => Self::create_viper(),
            ProtoStorageEngine::Sst => Self::create_sst(),
            ProtoStorageEngine::Helix => Self::create_helix(),
            ProtoStorageEngine::Swift => {
                #[cfg(feature = "experimental-engines")]
                {
                    Self::create_swift()
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "SWIFT engine is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            ProtoStorageEngine::Nova => Self::create_nova(),
            ProtoStorageEngine::Raptor => {
                #[cfg(feature = "experimental-engines")]
                {
                    Self::create_raptor()
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "RAPTOR engine is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            ProtoStorageEngine::Mmap => {
                warn!("MMAP engine not yet implemented, using SST");
                Self::create_sst()
            }
            ProtoStorageEngine::Hybrid => {
                warn!("Hybrid engine not yet implemented, using SST");
                Self::create_sst()
            }
        }
    }

    /// Async version of create_from_proto for use in async contexts (e.g., tests)
    pub async fn create_from_proto_async(
        engine_type: ProtoStorageEngine,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match engine_type {
            ProtoStorageEngine::Unspecified => {
                warn!("Unspecified storage engine, defaulting to SST (VIPER not available)");
                Self::create_sst_async().await
            }
            ProtoStorageEngine::Viper => Self::create_viper_async().await,
            ProtoStorageEngine::Sst => Self::create_sst_async().await,
            ProtoStorageEngine::Helix => Self::create_helix_async().await,
            ProtoStorageEngine::Swift => {
                #[cfg(feature = "experimental-engines")]
                {
                    Self::create_swift_async().await
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "SWIFT engine is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            ProtoStorageEngine::Nova => Self::create_nova_async().await,
            ProtoStorageEngine::Raptor => {
                #[cfg(feature = "experimental-engines")]
                {
                    Self::create_raptor_async().await
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "RAPTOR engine is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            ProtoStorageEngine::Mmap => {
                warn!("MMAP engine not yet implemented, using SST");
                Self::create_sst_async().await
            }
            ProtoStorageEngine::Hybrid => {
                warn!("Hybrid engine not yet implemented, using SST");
                Self::create_sst_async().await
            }
        }
    }

    /// Create a storage engine from strategy enum
    ///
    /// ## Strategy Mapping:
    ///
    /// Maps high-level storage strategies to concrete engines.
    /// Used by query planner and collection configuration.
    ///
    /// ### Strategy Selection:
    /// - **Viper**: Columnar for analytics/batch
    /// - **Lsm**: Row-based for OLTP
    /// - **Hybrid**: RAPTOR for mixed workloads
    /// - **Swift**: Optimized row-based for speed
    /// - **Nova**: Advanced columnar with zone maps
    /// - **Helix**: PCA+Hilbert for high dimensions
    pub fn create_from_strategy(
        strategy: StorageEngineStrategy,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match strategy {
            StorageEngineStrategy::Viper => Self::create_viper(),
            StorageEngineStrategy::Sst => Self::create_sst(),
            StorageEngineStrategy::Hybrid => {
                #[cfg(feature = "experimental-engines")]
                {
                    // RAPTOR uses hybrid strategy (row-aligned with columnar benefits)
                    info!("Creating RAPTOR engine for hybrid strategy");
                    Self::create_raptor()
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "RAPTOR engine (hybrid strategy) is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            StorageEngineStrategy::Swift => {
                #[cfg(feature = "experimental-engines")]
                {
                    info!("Creating SWIFT engine");
                    Self::create_swift()
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "SWIFT engine is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            StorageEngineStrategy::Nova => {
                info!("Creating NOVA engine");
                Self::create_nova()
            }
            StorageEngineStrategy::Raptor => {
                #[cfg(feature = "experimental-engines")]
                {
                    info!("Creating RAPTOR engine");
                    Self::create_raptor()
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    anyhow::bail!(
                        "RAPTOR engine is experimental and disabled in default builds. \
                         Use SST, VIPER, HELIX, or NOVA instead. \
                         To enable: cargo build --features experimental-engines"
                    )
                }
            }
            StorageEngineStrategy::Helix => {
                info!("Creating HELIX engine");
                Self::create_helix()
            }
        }
    }

    /// Create VIPER engine with default configuration
    ///
    /// ## VIPER Initialization:
    ///
    /// VIPER requires async initialization for:
    /// - Filesystem setup (S3/Azure/GCS support)
    /// - Quantization engine initialization
    /// - Footer cache warming
    ///
    /// Uses tokio runtime blocking to bridge async/sync gap.
    /// In production, prefer async factory methods.
    pub fn create_viper() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating VIPER storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async { ViperEngine::new().await })?;
        Ok(Arc::new(engine))
    }

    /// Async version for use within async contexts (e.g., tests)
    pub async fn create_viper_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating VIPER storage engine");
        let engine = ViperEngine::new().await?;
        Ok(Arc::new(engine))
    }

    /// Create SST engine with default configuration
    ///
    /// ## SST Initialization:
    ///
    /// SST requires async initialization for:
    /// - Compaction manager setup
    /// - Atomic coordinator creation
    /// - Decompression cache initialization
    ///
    /// SST serves as the default fallback engine due to its
    /// general-purpose nature and production stability.
    pub fn create_sst() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating SST storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async { SstEngine::new().await })?;
        Ok(Arc::new(engine))
    }

    /// Async version for use within async contexts (e.g., tests)
    pub async fn create_sst_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating SST storage engine");
        let engine = SstEngine::new().await?;
        Ok(Arc::new(engine))
    }

    /// Create SWIFT engine (Storage With Instant Fast Traversal)
    ///
    /// ## SWIFT Features:
    ///
    /// - Adaptive block sizing for optimal I/O
    /// - Superblock caching for hot data
    /// - Hierarchical ID indexing
    /// - Progressive search with early termination
    ///
    /// SWIFT is optimized for low-latency point lookups while
    /// maintaining good scan performance.
    ///
    /// **Requires `experimental-engines` feature flag.**
    #[cfg(feature = "experimental-engines")]
    pub fn create_swift() -> Result<Arc<dyn UnifiedStorageEngine>> {
        warn!("SWIFT engine is experimental and not production-ready");
        info!("Creating SWIFT (Storage With Instant Fast Traversal) storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async { SwiftEngine::new().await })?;
        Ok(Arc::new(engine))
    }

    /// Async version for use within async contexts (e.g., tests)
    ///
    /// **Requires `experimental-engines` feature flag.**
    #[cfg(feature = "experimental-engines")]
    pub async fn create_swift_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        warn!("SWIFT engine is experimental and not production-ready");
        info!("Creating SWIFT storage engine");
        let engine = SwiftEngine::new().await?;
        Ok(Arc::new(engine))
    }

    /// Create HELIX engine (Hierarchical Euclidean Layout with Indexed eXtensions)
    ///
    /// ## HELIX Features:
    ///
    /// - PCA dimension reduction (768 → 128)
    /// - Hilbert curve space-filling for locality
    /// - Liquid clustering for dynamic reorganization
    /// - Proxima encoding for SIMD operations
    ///
    /// HELIX excels at high-dimensional data by reducing dimensions
    /// while preserving 95%+ of variance.
    pub fn create_helix() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating HELIX storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async {
            use crate::storage::engines::impls::helix::HelixEngine;
            HelixEngine::new().await
        })?;
        Ok(Arc::new(engine))
    }

    /// Async version for use within async contexts (e.g., tests)
    pub async fn create_helix_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating HELIX storage engine");
        use crate::storage::engines::impls::helix::HelixEngine;
        let engine = HelixEngine::new().await?;
        Ok(Arc::new(engine))
    }

    /// Create NOVA engine (Next-gen Optimized Vector Analytics)
    ///
    /// ## NOVA Features:
    ///
    /// - Zone maps for predicate pushdown
    /// - Hierarchical statistics for pruning
    /// - Streaming search with bounded memory
    /// - Quantized columns for compression
    ///
    /// NOVA enhances columnar storage with advanced indexing
    /// and statistics for superior analytics performance.
    pub fn create_nova() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating NOVA (Next-gen Optimized Vector Analytics) storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(NovaEngine::new())?;
        Ok(Arc::new(engine))
    }

    /// Async version for use within async contexts (e.g., tests)
    pub async fn create_nova_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating NOVA storage engine");
        let engine = NovaEngine::new().await?;
        Ok(Arc::new(engine))
    }

    /// Create RAPTOR engine (Row-Aligned Predicated Tensor Optimized Repository)
    ///
    /// ## RAPTOR Architecture:
    ///
    /// RAPTOR uses Matrix Trinity navigation (P²+K²+P×K) instead of HNSW:
    /// - P²: Principal component space
    /// - K²: K-means cluster space
    /// - P×K: Cross-product space
    ///
    /// This provides 3x faster navigation with 50% less memory than HNSW.
    ///
    /// Note: Requires async initialization with collection metadata.
    ///
    /// **Requires `experimental-engines` feature flag.**
    #[cfg(feature = "experimental-engines")]
    pub fn create_raptor() -> Result<Arc<dyn UnifiedStorageEngine>> {
        warn!("RAPTOR engine is experimental and not production-ready");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async { RaptorEngine::new().await })?;
        Ok(Arc::new(engine))
    }

    /// Async version for use within async contexts (e.g., tests)
    ///
    /// **Requires `experimental-engines` feature flag.**
    #[cfg(feature = "experimental-engines")]
    pub async fn create_raptor_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        warn!("RAPTOR engine is experimental and not production-ready");
        info!("Creating RAPTOR storage engine");
        let engine = RaptorEngine::new().await?;
        Ok(Arc::new(engine))
    }

    /// Create a storage engine with metrics integration
    pub fn create_with_metrics(
        engine_type: ProtoStorageEngine,
        metrics_collector: Arc<EngineMetricsCollector>,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        let engine = Self::create_from_proto(engine_type)?;

        // Set up metrics for SWIFT and NOVA engines
        match engine_type {
            ProtoStorageEngine::Swift => {
                // TODO: Fix trait object downcasting - this is complex with Arc<dyn Trait>
                // Commented out until swift variable is properly defined
                // if false { // Temporarily disable this complex downcasting
                //     swift.set_metrics_collector(metrics_collector.clone());
                //     // Register engine with collector
                //     let weak_ref = Arc::downgrade(&(Arc::new(swift) as Arc<dyn UnifiedStorageEngine>));
                //     tokio::spawn(async move {
                //         metrics_collector.register_engine("SWIFT".to_string(), weak_ref).await;
                //     });
                //     return Ok(Arc::new(swift) as Arc<dyn UnifiedStorageEngine>);
                // }
            }
            ProtoStorageEngine::Nova => {
                // NOVA engine already created, just register it
                let weak_ref = Arc::downgrade(&engine);
                let collector = metrics_collector.clone();
                tokio::spawn(async move {
                    collector
                        .register_engine("NOVA".to_string(), weak_ref)
                        .await;
                });
            }
            _ => {
                // For other engines, just register without metrics modification
                let weak_ref = Arc::downgrade(&engine);
                let engine_name = format!("{:?}", engine_type);
                tokio::spawn(async move {
                    metrics_collector
                        .register_engine(engine_name, weak_ref)
                        .await;
                });
            }
        }

        Ok(engine)
    }

    /// Create the best engine for a given workload
    pub fn create_for_workload(workload: WorkloadType) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match workload {
            WorkloadType::Analytics => {
                info!("Analytics workload detected, using NOVA for advanced columnar analytics");
                Self::create_nova()
            }
            WorkloadType::Transactional => {
                #[cfg(feature = "experimental-engines")]
                {
                    info!("Transactional workload detected, using SWIFT for fast ID lookups");
                    Self::create_swift()
                }
                #[cfg(not(feature = "experimental-engines"))]
                {
                    info!(
                        "Transactional workload detected, using SST (SWIFT requires experimental-engines feature)"
                    );
                    Self::create_sst()
                }
            }
            WorkloadType::Mixed => {
                info!("Mixed workload detected, using VIPER for balanced performance");
                Self::create_viper()
            }
            WorkloadType::Experimental => {
                info!("Experimental workload, using HELIX for PCA + Hilbert clustering");
                Self::create_helix()
            }
        }
    }

    /// Get engine recommendations based on requirements
    pub fn recommend_engine(requirements: &EngineRequirements) -> ProtoStorageEngine {
        // Score each engine based on requirements
        let mut scores = vec![
            (ProtoStorageEngine::Viper, 0),
            (ProtoStorageEngine::Sst, 0),
            (ProtoStorageEngine::Swift, 0),
            (ProtoStorageEngine::Nova, 0),
        ];

        for (engine, score) in &mut scores {
            *score = Self::score_engine(*engine, requirements);
        }

        // Sort by score (highest first)
        scores.sort_by_key(|(_, score)| std::cmp::Reverse(*score));

        let (best_engine, best_score) = scores[0];
        info!(
            "Recommended engine: {:?} (similarity: {})",
            best_engine, best_score
        );

        best_engine
    }

    /// Score an engine based on requirements
    fn score_engine(engine: ProtoStorageEngine, req: &EngineRequirements) -> i32 {
        let mut score = 0;

        match engine {
            ProtoStorageEngine::Viper => {
                // VIPER: Good for general use, columnar storage
                if req.needs_columnar {
                    score += 20;
                }
                if req.needs_compression {
                    score += 15;
                }
                if req.needs_batch_operations {
                    score += 10;
                }
                score += 10; // Base score for maturity
            }
            ProtoStorageEngine::Sst => {
                // SST: Good for write-heavy, row-based
                if req.needs_fast_writes {
                    score += 20;
                }
                if req.needs_transactions {
                    score += 15;
                }
                if req.needs_id_lookup {
                    score += 10;
                }
                score += 10; // Base score for maturity
            }
            ProtoStorageEngine::Swift => {
                // SWIFT: Storage With Instant Fast Traversal - optimized for AXIS integration
                if req.needs_id_lookup {
                    score += 25;
                }
                if req.needs_progressive_search {
                    score += 20;
                }
                if req.needs_quantization {
                    score += 15;
                }
                if req.needs_zero_overhead {
                    score += 20;
                }
                score += 5; // Lower base score (newer)
            }
            ProtoStorageEngine::Nova => {
                // NOVA: Next-gen Optimized Vector Analytics - advanced columnar with dual-mode
                if req.needs_columnar {
                    score += 25;
                }
                if req.needs_predicate_pushdown {
                    score += 20;
                }
                if req.needs_projection {
                    score += 15;
                }
                if req.needs_progressive_search {
                    score += 15;
                }
                if req.needs_zero_overhead {
                    score += 20;
                }
                score += 5; // Lower base score (newer)
            }
            _ => {}
        }

        score
    }
}

/// Workload type for engine selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadType {
    /// Analytics workload (read-heavy, aggregations)
    Analytics,
    /// Transactional workload (write-heavy, point queries)
    Transactional,
    /// Mixed workload (balanced read/write)
    Mixed,
    /// Experimental workload (testing new features)
    Experimental,
}

/// Engine requirements for recommendation
#[derive(Debug, Clone)]
pub struct EngineRequirements {
    pub needs_columnar: bool,
    pub needs_compression: bool,
    pub needs_batch_operations: bool,
    pub needs_fast_writes: bool,
    pub needs_transactions: bool,
    pub needs_id_lookup: bool,
    pub needs_progressive_search: bool,
    pub needs_quantization: bool,
    pub needs_predicate_pushdown: bool,
    pub needs_projection: bool,
    pub needs_zero_overhead: bool,
}

impl Default for EngineRequirements {
    fn default() -> Self {
        Self {
            needs_columnar: false,
            needs_compression: true,
            needs_batch_operations: true,
            needs_fast_writes: false,
            needs_transactions: false,
            needs_id_lookup: true,
            needs_progressive_search: false,
            needs_quantization: true,
            needs_predicate_pushdown: false,
            needs_projection: false,
            needs_zero_overhead: false,
        }
    }
}

/// Engine comparison result for decision-making
///
/// ## Maturity Score Interpretation:
/// - 90+: Production-ready, extensively tested
/// - 70-89: Production-ready for specialized use cases
/// - 50-69: Experimental, use with caution
/// - <50: Development/prototype stage
///
/// ## Performance Score Interpretation:
/// - 90+: Best-in-class for target workload
/// - 75-89: Good performance
/// - 60-74: Acceptable with trade-offs
/// - <60: Not optimized for this workload
#[derive(Debug)]
pub struct EngineComparison {
    /// Engine identifier (e.g., "SST", "VIPER")
    pub engine_name: String,
    /// Key advantages of this engine
    pub pros: Vec<String>,
    /// Limitations and trade-offs
    pub cons: Vec<String>,
    /// Workload types this engine excels at
    pub best_for: Vec<String>,
    /// Performance score (0-100) for optimal workloads
    pub performance_score: i32,
    /// Maturity score (0-100) based on test coverage and production use
    pub maturity_score: i32,
}

impl StorageEngineFactory {
    /// Compare all available engines
    pub fn compare_engines() -> Vec<EngineComparison> {
        vec![
            EngineComparison {
                engine_name: "VIPER".to_string(),
                pros: vec![
                    "Columnar storage for analytics".to_string(),
                    "Excellent compression".to_string(),
                    "Parquet integration".to_string(),
                ],
                cons: vec![
                    "Higher write latency".to_string(),
                    "Complex compaction_info".to_string(),
                ],
                best_for: vec![
                    "Analytics workloads".to_string(),
                    "Read-heavy applications".to_string(),
                ],
                performance_score: 80,
                maturity_score: 90,
            },
            EngineComparison {
                engine_name: "SST".to_string(),
                pros: vec![
                    "Fast writes".to_string(),
                    "Simple design".to_string(),
                    "Good for streaming".to_string(),
                ],
                cons: vec![
                    "Less compression".to_string(),
                    "Row-based storage".to_string(),
                ],
                best_for: vec![
                    "Write-heavy workloads".to_string(),
                    "Real-time ingestion".to_string(),
                ],
                performance_score: 75,
                maturity_score: 90,
            },
            EngineComparison {
                engine_name: "SWIFT".to_string(),
                pros: vec![
                    "Zero-overhead with AXIS".to_string(),
                    "B+ tree ID indexing".to_string(),
                    "Progressive search".to_string(),
                    "Dual-mode operation".to_string(),
                ],
                cons: vec![
                    "Newer, less tested".to_string(),
                    "Complex architecture".to_string(),
                ],
                best_for: vec![
                    "AXIS integration".to_string(),
                    "ID-based lookups".to_string(),
                    "Memory-constrained environments".to_string(),
                ],
                performance_score: 85,
                maturity_score: 60,
            },
            EngineComparison {
                engine_name: "NOVA".to_string(),
                pros: vec![
                    "Columnar with dual-mode".to_string(),
                    "Predicate pushdown".to_string(),
                    "Projection support".to_string(),
                    "Best compression".to_string(),
                ],
                cons: vec![
                    "Newer engine (66+ tests)".to_string(),
                    "Complex configuration".to_string(),
                ],
                best_for: vec![
                    "Advanced analytics".to_string(),
                    "Large-scale deployments".to_string(),
                    "Predicate-heavy queries".to_string(),
                ],
                performance_score: 90,
                maturity_score: 75,
            },
            EngineComparison {
                engine_name: "HELIX".to_string(),
                pros: vec![
                    "PCA dimension reduction (768 -> 128)".to_string(),
                    "Hilbert curve locality optimization".to_string(),
                    "95%+ variance preservation".to_string(),
                    "Liquid clustering".to_string(),
                ],
                cons: vec![
                    "Only optimal for high dimensions (>512)".to_string(),
                    "PCA training overhead".to_string(),
                ],
                best_for: vec![
                    "High-dimensional embeddings".to_string(),
                    "LLM vector storage".to_string(),
                    "Dimension-reduced search".to_string(),
                ],
                performance_score: 85,
                maturity_score: 75,
            },
            EngineComparison {
                engine_name: "RAPTOR".to_string(),
                pros: vec![
                    "Matrix Trinity navigation (P2+K2+PxK)".to_string(),
                    "Adaptive row-group sizing".to_string(),
                    "3x faster than HNSW navigation".to_string(),
                    "50% less memory than HNSW".to_string(),
                ],
                cons: vec![
                    "Experimental (23+ tests)".to_string(),
                    "Several TODOs remaining".to_string(),
                    "Complex matrix computation".to_string(),
                ],
                best_for: vec![
                    "Graph-like traversal patterns".to_string(),
                    "Mixed read/write workloads".to_string(),
                    "Dynamic clustering scenarios".to_string(),
                ],
                performance_score: 80,
                maturity_score: 55,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_recommendation() {
        // Test for analytics workload
        let req = EngineRequirements {
            needs_columnar: true,
            needs_compression: true,
            needs_predicate_pushdown: true,
            needs_projection: true,
            ..Default::default()
        };

        let engine = StorageEngineFactory::recommend_engine(&req);
        assert_eq!(engine, ProtoStorageEngine::Nova);

        // Test for transactional workload
        let req = EngineRequirements {
            needs_fast_writes: true,
            needs_transactions: true,
            needs_id_lookup: true,
            ..Default::default()
        };

        let engine = StorageEngineFactory::recommend_engine(&req);
        assert!(matches!(
            engine,
            ProtoStorageEngine::Sst | ProtoStorageEngine::Swift
        ));
    }

    #[test]
    fn test_workload_based_selection() {
        // Analytics should prefer NOVA
        let engine = StorageEngineFactory::create_for_workload(WorkloadType::Analytics);
        assert!(engine.is_ok());

        // Transactional should prefer SWIFT
        let engine = StorageEngineFactory::create_for_workload(WorkloadType::Transactional);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_engine_comparison() {
        let comparisons = StorageEngineFactory::compare_engines();

        // Now includes all 6 engines: SST, VIPER, SWIFT, NOVA, HELIX, RAPTOR
        assert_eq!(comparisons.len(), 6);

        // NOVA should have highest performance score
        let nova = comparisons
            .iter()
            .find(|c| c.engine_name == "NOVA")
            .unwrap();
        assert_eq!(nova.performance_score, 90);

        // VIPER and SST should have highest maturity
        let viper = comparisons
            .iter()
            .find(|c| c.engine_name == "VIPER")
            .unwrap();
        assert_eq!(viper.maturity_score, 90);

        // RAPTOR should be marked as experimental (lower maturity)
        let raptor = comparisons
            .iter()
            .find(|c| c.engine_name == "RAPTOR")
            .unwrap();
        assert!(raptor.maturity_score < 70);
    }
}
