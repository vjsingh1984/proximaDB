//! # UnifiedProximaSIMD - Modular Hardware-Accelerated Proxima Encoding System
//!
//! ## Architecture Overview
//!
//! **UnifiedProximaSIMD** is the **production-grade, hardware-accelerated** encoding layer
//! for all storage engines (SST, SWIFT, RAPTOR, HELIX, VIPER, NOVA). It provides 2-5x faster
//! encoding than the baseline ProximaEncoder through SIMD intrinsics (AVX2, AVX-512, NEON, SSE).
//!
//! ### Modular Architecture
//!
//! The module is organized into logical submodules for maintainability:
//!
//! ```text
//! unified_proxima_simd/
//! ├── mod.rs              # Public API, re-exports
//! ├── impl_current.rs     # Current implementation (monolithic, being refactored)
//! ├── config.rs           # EngineProfile, SIMDConfig, SIMDEngineConfig
//! ├── stats.rs            # SIMDVectorStats, pattern detection
//! ├── patterns.rs         # SIMDVectorPattern, DataType enum
//! ├── encoding/
//! │   ├── mod.rs         # Encoding public API
//! │   ├── f32_encoder.rs # f32-specific encoding
//! │   ├── i64_encoder.rs # Integer encoding
//! │   └── batch.rs       # Batch operations
//! └── decoding/
//!     ├── mod.rs         # Decoding public API
//!     ├── f32_decoder.rs # f32-specific decoding
//!     └── i64_decoder.rs # Integer decoding
//! ```
//!
//! ### Data Type Support
//!
//! The system now supports multiple data types beyond f32 vectors:
//! - **F32Vector**: Vector dimensions (existing functionality)
//! - **I64Timestamp**: Timestamp columns (Delta, DoubleDelta, PForDelta)
//! - **I64Id**: ID columns (VByte, Simple8b, PForDelta)
//! - **I64Count**: Count/size columns (Delta, PForDelta, Simple8b)
//! - **U64Hash**: Hash values (BitPacked, Simple8b)
//!
//! ### Benchmark-Driven Pattern Detection
//!
//! Based on comprehensive benchmarking (67% speed / 33% compression weighting):
//!
//! - **Normalized Pattern**: Simple8b (26x speedup, 25x compression)
//! - **Sequential Pattern**: PForDelta (2.94 score)
//! - **Sparse Pattern**: Simple8b (5.00 score)
//! - **Random Pattern**: PForDelta (1.90 score)
//! - **Constant Pattern**: RunLength (75.74 score, perfect compression)
//!
//! ### Encoding Scheme Suitability
//!
//! **Good for F32 Vectors**:
//! - Delta ✓, BitPacked ✓, PForDelta ✓, Simple8b ✓, VByte ✓, Sparse schemes ✓
//!
//! **NOT for F32 Vectors** (use for integer metadata only):
//! - DoubleDelta ❌, FrameOfReference ❌, Zigzag ❌
//!
//! **Excellent for Integer Metadata**:
//! - Delta ✓, DoubleDelta ✓, Zigzag ✓, PForDelta ✓, Simple8b ✓, VByte ✓

// Modular submodules (clean architecture)
pub mod config;
pub mod stats;
pub mod patterns;
pub mod encoding;
pub mod decoding;

// Current implementation (uses modular types)
#[path = "impl_current.rs"]
mod impl_current;

// Re-export ALL public items from current implementation
pub use impl_current::*;

// Re-export modular types for direct access
pub use config::{EngineProfile, SIMDConfig, SIMDEngineConfig};
pub use stats::SIMDVectorStats;
pub use patterns::{SIMDVectorPattern, DataType};
pub use encoding::{F32Encoder, I64Encoder, BatchEncoder};
pub use decoding::{F32Decoder, I64Decoder};
