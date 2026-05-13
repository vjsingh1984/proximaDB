//! Engine Capabilities Trait (OCP Compliant)
//!
//! Provides a trait-based abstraction for engine capabilities, replacing
//! hardcoded match statements with registrable capability providers.
//!
//! ## Design Goals:
//!
//! 1. **Open/Closed Principle**: Add new engines without modifying existing code
//! 2. **Registration Pattern**: Engines register capabilities at construction
//! 3. **Bundled with Engine**: Capabilities paired with engine instance
//! 4. **Type Safety**: Static capability checks at compile time where possible
//!
//! ## Problem Solved:
//!
//! Previously, capabilities were checked via match on engine strategy:
//! ```rust,ignore
//! match self.strategy() {
//!     StorageEngineStrategy::Sst => ScanCapabilities { ... },
//!     StorageEngineStrategy::Viper => ScanCapabilities { ... },
//!     // Adding new engine requires modifying this match!
//! }
//! ```
//!
//! ## New Pattern:
//!
//! ```rust,ignore
//! // Each engine provides its capabilities
//! impl EngineCapabilities for SstCapabilities {
//!     fn scan_capabilities(&self) -> ScanCapabilities { ... }
//! }
//!
//! // Factory returns engine bundled with capabilities
//! let bundle = EngineFactory::create(config);
//! let caps = bundle.capabilities.scan_capabilities();
//! ```

use std::collections::HashSet;

use crate::proto::proximadb_v1::CompressionAlgorithm;
use crate::storage::scan_strategy::ScanCapabilities;

/// Flush thresholds configuration
#[derive(Debug, Clone)]
pub struct FlushThresholds {
    /// Memory threshold in bytes (flush when exceeded)
    pub memory_threshold_bytes: usize,
    /// Entry count threshold (flush when exceeded)
    pub entry_count_threshold: usize,
    /// Time threshold in seconds (flush if idle for this long)
    pub time_threshold_secs: u64,
    /// Whether to use global buffer behavior
    pub use_global_buffer: bool,
}

impl Default for FlushThresholds {
    fn default() -> Self {
        Self {
            memory_threshold_bytes: 16 * 1024 * 1024, // 16 MB
            entry_count_threshold: 10_000,
            time_threshold_secs: 60,
            use_global_buffer: true,
        }
    }
}

/// Compaction heuristics configuration
#[derive(Debug, Clone)]
pub struct CompactionHeuristics {
    /// Minimum number of SST files to trigger compaction
    pub min_files_to_compact: usize,
    /// Maximum file size ratio for level-based compaction
    pub max_level_ratio: f64,
    /// Target file size after compaction
    pub target_file_size_bytes: usize,
    /// Whether to use size-tiered compaction
    pub size_tiered: bool,
    /// Whether to use leveled compaction
    pub leveled: bool,
}

impl Default for CompactionHeuristics {
    fn default() -> Self {
        Self {
            min_files_to_compact: 4,
            max_level_ratio: 10.0,
            target_file_size_bytes: 64 * 1024 * 1024, // 64 MB
            size_tiered: true,
            leveled: false,
        }
    }
}

/// Engine capabilities trait (OCP-compliant interface)
///
/// Each storage engine implementation provides its capabilities through this trait.
/// This eliminates match statements that hardcode engine-specific behavior.
pub trait EngineCapabilities: Send + Sync {
    /// Engine name for logging/debugging
    fn engine_name(&self) -> &'static str;

    /// Get scan capabilities for this engine
    fn scan_capabilities(&self) -> ScanCapabilities;

    /// Whether this engine supports collection-level operations
    fn supports_collection_level_operations(&self) -> bool;

    /// Whether this engine supports atomic operations
    fn supports_atomic_operations(&self) -> bool;

    /// Get flush thresholds for this engine
    fn flush_thresholds(&self) -> FlushThresholds {
        FlushThresholds::default()
    }

    /// Get compaction heuristics for this engine
    fn compaction_heuristics(&self) -> CompactionHeuristics {
        CompactionHeuristics::default()
    }

    /// Get supported compression algorithms
    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        // Default: common algorithms
        let mut supported = HashSet::new();
        supported.insert(CompressionAlgorithm::CompressionNone);
        supported.insert(CompressionAlgorithm::CompressionZstd);
        supported.insert(CompressionAlgorithm::CompressionLz4);
        supported.insert(CompressionAlgorithm::CompressionSnappy);
        supported
    }

    /// Check if a specific compression algorithm is supported
    fn is_compression_supported(&self, algorithm: CompressionAlgorithm) -> bool {
        self.supported_compression().contains(&algorithm)
    }

    /// Whether this engine supports progressive quantization search
    fn supports_progressive_quantization(&self) -> bool {
        self.scan_capabilities().supports_progressive_quantization
    }

    /// Whether this engine supports zone maps for pruning
    fn supports_zone_maps(&self) -> bool {
        self.scan_capabilities().supports_zone_maps
    }

    /// Whether this engine supports bloom filters
    fn supports_bloom_filters(&self) -> bool {
        self.scan_capabilities().supports_bloom_filters
    }
}

// ============================================================================
// Per-Engine Capability Implementations
// ============================================================================

/// SST engine capabilities
pub struct SstCapabilities;

impl EngineCapabilities for SstCapabilities {
    fn engine_name(&self) -> &'static str {
        "SST"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: false,
            supports_column_projection: false,
            supports_row_group_pruning: false,
            supports_parallel_column_evaluation: false,
            supports_bloom_filters: true,
            supports_block_cache: true,
            supports_range_scans: true,
            supports_index_scans: true,
            supports_progressive_quantization: false,
            supports_zone_maps: false,
            supports_streaming: false,
            supports_tier_aware_scanning: false,
            supports_consolidated_reading: false,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        false // SST operates on entire tree
    }

    fn supports_atomic_operations(&self) -> bool {
        false
    }

    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        let mut supported = HashSet::new();
        supported.insert(CompressionAlgorithm::CompressionNone);
        supported.insert(CompressionAlgorithm::CompressionZstd);
        supported.insert(CompressionAlgorithm::CompressionLz4);
        supported.insert(CompressionAlgorithm::CompressionSnappy);
        supported.insert(CompressionAlgorithm::CompressionGzip);
        supported.insert(CompressionAlgorithm::CompressionBrotli);
        supported.insert(CompressionAlgorithm::CompressionBzip2);
        supported.insert(CompressionAlgorithm::CompressionDeflate);
        supported.insert(CompressionAlgorithm::CompressionXz);
        supported.insert(CompressionAlgorithm::CompressionZlib);
        supported.insert(CompressionAlgorithm::CompressionLz4hc);
        supported.insert(CompressionAlgorithm::CompressionLzma);
        supported
    }
}

/// HELIX engine capabilities
pub struct HelixCapabilities;

impl EngineCapabilities for HelixCapabilities {
    fn engine_name(&self) -> &'static str {
        "HELIX"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: true,
            supports_column_projection: false,
            supports_row_group_pruning: false,
            supports_parallel_column_evaluation: false,
            supports_bloom_filters: true,
            supports_block_cache: true,
            supports_range_scans: true,
            supports_index_scans: true,
            supports_progressive_quantization: true,
            supports_zone_maps: true,
            supports_streaming: false,
            supports_tier_aware_scanning: false,
            supports_consolidated_reading: false,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        true // HELIX supports collection-level ops
    }

    fn supports_atomic_operations(&self) -> bool {
        true // HELIX provides atomic guarantees
    }

    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        // Same as SST (SST-based engine)
        SstCapabilities.supported_compression()
    }
}

/// VIPER engine capabilities
pub struct ViperCapabilities;

impl EngineCapabilities for ViperCapabilities {
    fn engine_name(&self) -> &'static str {
        "VIPER"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: true,
            supports_column_projection: true,
            supports_row_group_pruning: true,
            supports_parallel_column_evaluation: true,
            supports_bloom_filters: false,
            supports_block_cache: false,
            supports_range_scans: false,
            supports_index_scans: false,
            supports_progressive_quantization: false,
            supports_zone_maps: true,
            supports_streaming: true,
            supports_tier_aware_scanning: false,
            supports_consolidated_reading: false,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        true // VIPER supports collection-level ops
    }

    fn supports_atomic_operations(&self) -> bool {
        true
    }

    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        // Parquet-compatible algorithms
        let mut supported = HashSet::new();
        supported.insert(CompressionAlgorithm::CompressionNone);
        supported.insert(CompressionAlgorithm::CompressionZstd);
        supported.insert(CompressionAlgorithm::CompressionLz4);
        supported.insert(CompressionAlgorithm::CompressionSnappy);
        supported.insert(CompressionAlgorithm::CompressionGzip);
        supported.insert(CompressionAlgorithm::CompressionBrotli);
        supported
    }
}

/// SWIFT engine capabilities
pub struct SwiftCapabilities;

impl EngineCapabilities for SwiftCapabilities {
    fn engine_name(&self) -> &'static str {
        "SWIFT"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: false,
            supports_column_projection: false,
            supports_row_group_pruning: false,
            supports_parallel_column_evaluation: false,
            supports_bloom_filters: true,
            supports_block_cache: true,
            supports_range_scans: true,
            supports_index_scans: true,
            supports_progressive_quantization: false,
            supports_zone_maps: false,
            supports_streaming: false,
            supports_tier_aware_scanning: true,
            supports_consolidated_reading: false,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        true // SWIFT supports collection-level ops
    }

    fn supports_atomic_operations(&self) -> bool {
        true // SWIFT provides atomic guarantees
    }

    fn flush_thresholds(&self) -> FlushThresholds {
        FlushThresholds {
            memory_threshold_bytes: 8 * 1024 * 1024, // 8 MB (smaller for low-latency)
            entry_count_threshold: 5_000,
            time_threshold_secs: 30,
            use_global_buffer: false, // SWIFT manages its own buffers
        }
    }
}

/// NOVA engine capabilities
pub struct NovaCapabilities;

impl EngineCapabilities for NovaCapabilities {
    fn engine_name(&self) -> &'static str {
        "NOVA"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: true,
            supports_column_projection: true,
            supports_row_group_pruning: true,
            supports_parallel_column_evaluation: true,
            supports_bloom_filters: false,
            supports_block_cache: false,
            supports_range_scans: false,
            supports_index_scans: false,
            supports_progressive_quantization: true,
            supports_zone_maps: true,
            supports_streaming: true,
            supports_tier_aware_scanning: false,
            supports_consolidated_reading: false,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        true
    }

    fn supports_atomic_operations(&self) -> bool {
        true
    }

    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        // Same as VIPER (columnar-based)
        ViperCapabilities.supported_compression()
    }
}

/// TST engine capabilities
pub struct TstCapabilities;

impl EngineCapabilities for TstCapabilities {
    fn engine_name(&self) -> &'static str {
        "TST"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: true,
            supports_column_projection: true,
            supports_row_group_pruning: true,
            supports_parallel_column_evaluation: true,
            supports_bloom_filters: false,
            supports_block_cache: true,
            supports_range_scans: true,
            supports_index_scans: false,
            supports_progressive_quantization: false,
            supports_zone_maps: true,
            supports_streaming: true,
            supports_tier_aware_scanning: true,
            supports_consolidated_reading: false,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        true
    }

    fn supports_atomic_operations(&self) -> bool {
        true
    }

    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        ViperCapabilities.supported_compression()
    }
}

/// RAPTOR engine capabilities
pub struct RaptorCapabilities;

impl EngineCapabilities for RaptorCapabilities {
    fn engine_name(&self) -> &'static str {
        "RAPTOR"
    }

    fn scan_capabilities(&self) -> ScanCapabilities {
        ScanCapabilities {
            supports_predicate_pushdown: true,
            supports_column_projection: true,
            supports_row_group_pruning: true,
            supports_parallel_column_evaluation: false,
            supports_bloom_filters: true,
            supports_block_cache: false,
            supports_range_scans: false,
            supports_index_scans: false,
            supports_progressive_quantization: false,
            supports_zone_maps: false,
            supports_streaming: true,
            supports_tier_aware_scanning: true,
            supports_consolidated_reading: true,
        }
    }

    fn supports_collection_level_operations(&self) -> bool {
        true // RAPTOR supports collection-level ops
    }

    fn supports_atomic_operations(&self) -> bool {
        false // RAPTOR uses eventual consistency
    }

    fn compaction_heuristics(&self) -> CompactionHeuristics {
        CompactionHeuristics {
            min_files_to_compact: 2, // RAPTOR uses consolidated reading
            max_level_ratio: 5.0,
            target_file_size_bytes: 128 * 1024 * 1024, // 128 MB (larger files)
            size_tiered: true,
            leveled: true, // RAPTOR uses hybrid approach
        }
    }

    fn supported_compression(&self) -> HashSet<CompressionAlgorithm> {
        // Same as VIPER (columnar-based)
        ViperCapabilities.supported_compression()
    }
}

/// Engine bundle: pairs engine instance with its capabilities
///
/// This is the return type from the factory, ensuring capabilities
/// are always available alongside the engine.
pub struct EngineBundle<E> {
    /// The engine instance
    pub engine: E,
    /// The engine's capabilities
    pub capabilities: Box<dyn EngineCapabilities>,
}

impl<E> EngineBundle<E> {
    /// Create a new engine bundle
    pub fn new(engine: E, capabilities: Box<dyn EngineCapabilities>) -> Self {
        Self {
            engine,
            capabilities,
        }
    }
}

/// Factory for creating capability instances based on engine strategy
pub struct CapabilityFactory;

impl CapabilityFactory {
    /// Create capabilities for a given engine strategy
    pub fn create(
        strategy: crate::storage::traits::StorageEngineStrategy,
    ) -> Box<dyn EngineCapabilities> {
        use crate::storage::traits::StorageEngineStrategy;

        match strategy {
            StorageEngineStrategy::Sst => Box::new(SstCapabilities),
            StorageEngineStrategy::Helix => Box::new(HelixCapabilities),
            StorageEngineStrategy::Viper => Box::new(ViperCapabilities),
            StorageEngineStrategy::Swift => Box::new(SwiftCapabilities),
            StorageEngineStrategy::Nova => Box::new(NovaCapabilities),
            StorageEngineStrategy::TimeSeries => Box::new(TstCapabilities),
            StorageEngineStrategy::Raptor => Box::new(RaptorCapabilities),
            // Default to SST capabilities for unknown strategies
            _ => Box::new(SstCapabilities),
        }
    }

    /// Create capabilities from proto StorageEngine enum (for static utility bridge)
    ///
    /// This method bridges the static `EngineCapabilities` utility with the trait-based
    /// capability system, enabling OCP compliance while maintaining the static API.
    pub fn from_proto_engine(
        engine: crate::proto::proximadb_v1::StorageEngine,
    ) -> Box<dyn EngineCapabilities> {
        use crate::proto::proximadb_v1::StorageEngine;

        match engine {
            StorageEngine::Sst => Box::new(SstCapabilities),
            StorageEngine::Helix => Box::new(HelixCapabilities),
            StorageEngine::Viper => Box::new(ViperCapabilities),
            StorageEngine::Swift => Box::new(SwiftCapabilities),
            StorageEngine::Nova => Box::new(NovaCapabilities),
            StorageEngine::Tst => Box::new(TstCapabilities),
            StorageEngine::Raptor => Box::new(RaptorCapabilities),
            // Default to SST capabilities for unknown engines
            _ => Box::new(SstCapabilities),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sst_capabilities() {
        let caps = SstCapabilities;
        assert_eq!(caps.engine_name(), "SST");
        assert!(caps.supports_bloom_filters());
        assert!(!caps.supports_zone_maps());
        assert!(!caps.supports_collection_level_operations());
    }

    #[test]
    fn test_viper_capabilities() {
        let caps = ViperCapabilities;
        assert_eq!(caps.engine_name(), "VIPER");
        assert!(caps.scan_capabilities().supports_predicate_pushdown);
        assert!(caps.scan_capabilities().supports_column_projection);
        assert!(caps.supports_collection_level_operations());
    }

    #[test]
    fn test_helix_capabilities() {
        let caps = HelixCapabilities;
        assert_eq!(caps.engine_name(), "HELIX");
        assert!(caps.supports_progressive_quantization());
        assert!(caps.supports_zone_maps());
    }

    #[test]
    fn test_nova_capabilities() {
        let caps = NovaCapabilities;
        assert_eq!(caps.engine_name(), "NOVA");
        assert!(caps.supports_progressive_quantization());
        assert!(caps.supports_atomic_operations());
    }

    #[test]
    fn test_raptor_capabilities() {
        let caps = RaptorCapabilities;
        assert_eq!(caps.engine_name(), "RAPTOR");
        assert!(caps.scan_capabilities().supports_consolidated_reading);
        assert!(caps.scan_capabilities().supports_tier_aware_scanning);

        let heuristics = caps.compaction_heuristics();
        assert_eq!(heuristics.min_files_to_compact, 2);
    }

    #[test]
    fn test_swift_capabilities() {
        let caps = SwiftCapabilities;
        assert_eq!(caps.engine_name(), "SWIFT");

        let thresholds = caps.flush_thresholds();
        assert_eq!(thresholds.memory_threshold_bytes, 8 * 1024 * 1024);
        assert!(!thresholds.use_global_buffer);
    }

    #[test]
    fn test_compression_support() {
        let sst = SstCapabilities;
        assert!(sst.is_compression_supported(CompressionAlgorithm::CompressionZstd));
        assert!(sst.is_compression_supported(CompressionAlgorithm::CompressionLz4));

        let viper = ViperCapabilities;
        // VIPER supports fewer algorithms (Parquet-compatible)
        assert!(viper.is_compression_supported(CompressionAlgorithm::CompressionZstd));
    }

    #[test]
    fn test_capability_factory() {
        use crate::storage::traits::StorageEngineStrategy;

        let sst_caps = CapabilityFactory::create(StorageEngineStrategy::Sst);
        assert_eq!(sst_caps.engine_name(), "SST");

        let helix_caps = CapabilityFactory::create(StorageEngineStrategy::Helix);
        assert_eq!(helix_caps.engine_name(), "HELIX");
    }

    #[test]
    fn test_engine_bundle() {
        struct MockEngine;

        let bundle = EngineBundle::new(MockEngine, Box::new(SstCapabilities));
        assert_eq!(bundle.capabilities.engine_name(), "SST");
    }
}
