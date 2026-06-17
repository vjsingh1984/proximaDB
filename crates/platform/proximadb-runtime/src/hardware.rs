//! Hardware capability re-exports for the platform runtime.
//!
//! The canonical implementation lives in the `proximadb-hardware` foundation
//! crate so that lower-layer crates (modalities, query runtime) can detect
//! SIMD capabilities without a circular dependency on the platform runtime.
//!
//! Platform-runtime callers continue to `use proximadb_runtime::hardware::*`
//! without any change.

pub use proximadb_hardware::{
    HardwareCapabilities, SimdLevel, best_simd_level, hardware_capabilities,
};
