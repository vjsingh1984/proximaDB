//! Storage Engine Quantization Adapters
//!
//! This module provides storage-specific adapters for the common quantization
//! infrastructure in compute module, bridging computation and storage layers.

pub mod sst_adapter;
pub mod viper_adapter;

// Re-export common types for convenience
pub use crate::compute::quantization::storage_engine::{
    StorageQuantizationEngine,
    StorageQuantizationConfig,
    StorageQuantizedData,
    SearchStage,
    SearchStageResult,
    StageMetrics,
};

// Re-export engine-specific adapters
pub use sst_adapter::SstQuantizationAdapter;
pub use viper_adapter::ViperQuantizationAdapter;