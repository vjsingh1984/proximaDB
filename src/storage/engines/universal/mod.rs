//! Universal Storage Engine Adapter System
//!
//! This module provides a unified interface for distance computations across all storage engines.
//! It integrates the PQ and INT8 optimized distance computations and provides progressive 
//! refinement capabilities for all storage engines (PRISM, NOVA, SWIFT, VIPER, SST).
//!
//! ## Key Features
//!
//! - **Unified Interface**: Single API for all storage engines
//! - **Quantized Distance Support**: INT8 and PQ distance computations
//! - **Progressive Refinement**: Binary → INT8 → PQ → FP32 pipeline
//! - **Hardware Acceleration**: Automatic SIMD optimization
//! - **Format Conversion**: Seamless conversion between storage formats
//! - **Distance Table Caching**: Optimized PQ distance table caching
//! - **Storage Engine Agnostic**: Works with any underlying storage format
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                  Universal Adapter                         │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
//! │  │   Quantized     │  │   Progressive   │  │    Hardware     │ │
//! │  │   Distance      │  │   Refinement    │  │  Acceleration   │ │
//! │  │   Calculator    │  │   Pipeline      │  │    Manager      │ │
//! │  └─────────────────┘  └─────────────────┘  └─────────────────┘ │
//! ├─────────────────────────────────────────────────────────────┤
//! │ PRISM │ NOVA │ SWIFT │ VIPER │ SST │ Future Engines...      │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage Examples
//!
//! ```rust
//! use proximadb::storage::engines::universal::{
//!     UniversalDistanceAdapter, DistanceComputationRequest, 
//!     ProgressiveRefinementConfig, StorageFormat
//! };
//!
//! // Create universal adapter
//! let adapter = UniversalDistanceAdapter::new().await?;
//!
//! // Progressive refinement search
//! let request = DistanceComputationRequest {
//!     query_vector: query,
//!     candidates: candidates,
//!     storage_format: StorageFormat::QuantizedPQ { segments: 8, bits: 8 },
//!     refinement_config: ProgressiveRefinementConfig::default(),
//!     max_results: 10,
//! };
//!
//! let results = adapter.compute_progressive_distance(request).await?;
//! ```

pub mod adapter;
pub mod config;
pub mod conversion;
pub mod hardware_manager;
pub mod progressive_refinement;
pub mod quantized_calculator;
pub mod storage_integration;
pub mod tests;

// Public re-exports
pub use adapter::{
    UniversalDistanceAdapter, DistanceComputationRequest, DistanceComputationResult,
    CandidateVector, PerformanceMetrics, AdapterError, AdapterResult,
};

pub use config::{
    UniversalAdapterConfig, ProgressiveRefinementConfig, CacheConfig,
    HardwareAccelerationConfig, StorageEngineConfig,
};

pub use conversion::{
    StorageFormat, FormatConverter, ConversionError, ConversionResult,
    QuantizedFormat, CompressionFormat,
};

pub use hardware_manager::{
    HardwareAccelerationManager, AccelerationCapabilities, 
    OptimizationStrategy, SIMDCapabilities,
};

pub use progressive_refinement::{
    ProgressiveRefinementPipeline, RefinementStage, RefinementStrategy,
    QualityMetrics,
};

pub use quantized_calculator::{
    UniversalQuantizedCalculator, // QuantizedDistanceConfig, 
    DistanceTableCache, CacheEvictionPolicy,
};

pub use storage_integration::{
    StorageEngineAdapter, EngineType, IntegrationError,
    PRISMAdapter, NOVAAdapter, SWIFTAdapter, VIPERAdapter, SSTAdapter,
};

/// Current version of the universal adapter system
pub const UNIVERSAL_ADAPTER_VERSION: &str = "1.0.0";

/// Default configuration for the universal adapter system
impl Default for UniversalAdapterConfig {
    fn default() -> Self {
        Self {
            enable_progressive_refinement: true,
            enable_hardware_acceleration: true,
            enable_distance_caching: true,
            max_cache_size_mb: 256,
            simd_threshold: 64,
            refinement_stages: vec![
                RefinementStage::Binary,
                RefinementStage::INT8,
                RefinementStage::PQ,
                RefinementStage::FP32,
            ],
            hardware_acceleration: HardwareAccelerationConfig::default(),
            cache_config: CacheConfig::default(),
            storage_engines: vec![
                StorageEngineConfig::prism_default(),
                StorageEngineConfig::nova_default(),
                StorageEngineConfig::swift_default(),
                StorageEngineConfig::viper_default(),
                StorageEngineConfig::sst_default(),
            ],
        }
    }
}