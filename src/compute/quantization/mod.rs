//! Vector Quantization Module
//!
//! Provides unified quantization APIs that work across all storage engines.
//! Includes support for various quantization levels and hardware acceleration.

pub mod compile_time;
pub mod hardware_accelerated;
pub mod quantization_engine;
pub mod smart_defaults;
pub mod storage_engine;
#[cfg(feature = "experimental-turboquant")]
pub mod turboquant_store_registry;
pub mod types;

// Storage↔compute glue relocated to the storage layer (quantization kernel split,
// step Q2). Re-exported here so existing `crate::compute::quantization::{global_cache,
// selection}::*` consumers (storage engines) keep their import paths unchanged.
pub use crate::storage::compute_bridge::{global_cache, selection};

// Re-export low-dependency quantization modules from vector modality during Phase 6 migration.
pub use proximadb_vector::quantization::compile_time::*;
pub use proximadb_vector::quantization::smart_defaults::QuantizationSmartDefaults;

pub use quantization_engine::{
    BinaryQuantization, Codebook, CodebookData, CodebookStore, CustomQuantization,
    InMemoryCodebookStore, NoQuantization, ProductQuantization, QuantizationLevel,
    QuantizationMetadata, QuantizedVector, ScalarQuantization, TrainingConfig,
    UnifiedQuantizationEngine, UnifiedQuantizationLevel, UniformQuantization,
};

pub use global_cache::{GlobalQuantizationCache, QuantizationCacheKey};
pub use selection::{
    QuantizationSelectionReason, QuantizationSelector, RecommendedQuantizationLevel,
};
pub use storage_engine::{StorageQuantizationConfig, StorageQuantizationEngine};
