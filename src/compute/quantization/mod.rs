//! Vector Quantization Module
//!
//! Provides unified quantization APIs that work across all storage engines.
//! Includes support for various quantization levels and hardware acceleration.

pub mod unified;

pub use unified::{
    UnifiedQuantizationLevel, QuantizationLevelType,
    UniformQuantization, ProductQuantization, ScalarQuantization, 
    BinaryQuantization, CustomQuantization, NoQuantization,
    UnifiedQuantizationEngine, CodebookStore, Codebook, 
    TrainingConfig, CodebookData, QuantizedVector, 
    QuantizationMetadata, InMemoryCodebookStore
};