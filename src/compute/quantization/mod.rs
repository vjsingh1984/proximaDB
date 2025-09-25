//! Vector Quantization Module
//!
//! Provides unified quantization APIs that work across all storage engines.
//! Includes support for various quantization levels and hardware acceleration.

pub mod compile_time;
pub mod global_cache;
pub mod hardware_accelerated;
pub mod selection;
pub mod smart_defaults;
pub mod storage_engine;
pub mod types;
pub mod unified;

pub use unified::{
    BinaryQuantization, Codebook, CodebookData, CodebookStore, CustomQuantization,
    InMemoryCodebookStore, NoQuantization, ProductQuantization, QuantizationLevel,
    QuantizationMetadata, QuantizedVector, ScalarQuantization, TrainingConfig,
    UnifiedQuantizationEngine, UnifiedQuantizationLevel, UniformQuantization,
};

pub use storage_engine::{StorageQuantizationConfig, StorageQuantizationEngine};
pub use global_cache::{GlobalQuantizationCache, QuantizationCacheKey};
pub use selection::{QuantizationSelector, QuantizationSelectionReason, RecommendedQuantizationLevel};

pub use smart_defaults::QuantizationSmartDefaults;
