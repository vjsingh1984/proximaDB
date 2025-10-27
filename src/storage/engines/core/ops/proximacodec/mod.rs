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

pub mod analysis;
pub mod codec;
pub mod registry;
pub mod simd_analysis;
pub mod traits;
pub mod types;
pub mod wire_format;

// Hardware-accelerated implementations (SIMD + GPU)
pub mod simd;

// Hardware-aware batching framework (common across SIMD, GPU, Scalar)
pub mod batching;

// Re-export main types
pub use codec::ProximaCodec;
pub use registry::ImplementationRegistry;
pub use traits::{RawDecoder, RawEncoder};
pub use types::{Decodable, Encodable, ProximaScheme, TypeId};
pub use wire_format::{WIRE_FORMAT_VERSION, WireFormatManager, WireHeader};

// Implementations (will be added in next phases)
pub mod impls;

// Integration tests
#[cfg(test)]
mod tests;
