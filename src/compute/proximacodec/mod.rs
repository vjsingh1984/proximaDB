// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! ProximaCodec - Unified encoding/decoding for ProximaDB
//!
//! This module provides a clean, unified API for all encoding/decoding operations.
//! It replaces the old ProximaEncoder/ProximaDecoder with a modern architecture:
//!
//! - Single entry point: `ProximaCodec::global()`
//! - Hardware-aware routing: GPU → SIMD → Baseline
//! - Unified metrics integration
//! - Versioned wire format
//! - Platform-specific conditional compilation
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │     ProximaCodec (Public API)           │
//! │  - Global singleton                     │
//! │  - Hardware detection                   │
//! │  - Metrics integration                  │
//! └─────────────────────────────────────────┘
//!           │
//!     ┌─────┴──────┐
//!     ▼            ▼
//! WireFormat   Registry
//!  (Headers)   (HW Routing)
//!                  │
//!     ┌────────────┼────────────┐
//!     ▼            ▼            ▼
//! Baseline      SIMD          GPU
//! (always)   (conditional) (conditional)
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::ops::proximacodec::{ProximaCodec, ProximaScheme};
//!
//! let codec = ProximaCodec::global();
//!
//! // Encode
//! let values = vec![1.0f32, 2.0, 3.0];
//! let encoded = codec.encode(&values, ProximaScheme::Delta { base: 0 })?;
//!
//! // Decode
//! let decoded: Vec<f32> = codec.decode(&encoded)?;
//! assert_eq!(values, decoded);
//! ```

pub mod adaptive;
pub mod analysis;
pub mod baseline;
pub mod codec;
pub mod gpu;
pub mod registry;
pub mod simd_analysis;
pub mod strategy;
pub mod traits;
pub mod types;
pub mod wire_format;

// Hardware-accelerated implementations (SIMD + GPU)
pub mod simd;  // simd/ directory - consolidated SIMD implementation
// Experimental SIMD prototype lives under archive; opt-in only.
#[cfg(feature = "simd-experimental")]
pub mod simd_experimental;

// Hardware-aware batching framework (common across SIMD, GPU, Scalar)
pub mod batching;

// Re-export main types
pub use codec::ProximaCodec;
pub use registry::ImplementationRegistry;
pub use strategy::{
    CodecSelectionStrategy, DataAnalysis, DataDomain, IntegerAnalysisStrategy, MlEmbeddingStrategy,
    SelectionContext, SparseDataStrategy, StrategyRegistry, TimeSeriesStrategy,
};
pub use traits::{RawDecoder, RawEncoder};
pub use types::{Decodable, Encodable, ProximaScheme, TypeId};
pub use wire_format::{WIRE_FORMAT_VERSION, WireFormatManager, WireHeader};

// Implementations
pub mod impls;

// Integration tests
#[cfg(test)]
mod tests;
