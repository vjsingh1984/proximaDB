//! Vector Quantization Module
//!
//! Provides unified quantization APIs that work across all storage engines.
//! Includes support for various quantization levels and hardware acceleration.

pub mod types;
pub mod unified;
pub mod storage_engine;
pub mod smart_defaults;
pub mod hardware_accelerated;

pub use unified::{
    UnifiedQuantizationLevel, QuantizationLevelType,
    UniformQuantization, ProductQuantization, ScalarQuantization, 
    BinaryQuantization, CustomQuantization, NoQuantization,
    UnifiedQuantizationEngine, CodebookStore, Codebook, 
    TrainingConfig, CodebookData, QuantizedVector, 
    QuantizationMetadata, InMemoryCodebookStore
};

pub use storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig, 
};

pub use smart_defaults::{
    QuantizationSmartDefaults,
};