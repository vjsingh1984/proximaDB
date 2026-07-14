// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! ProximaCodec — unified encoding/decoding for ProximaDB.
//!
//! This crate is the canonical home for the ProximaCodec: the public
//! [`ProximaCodec`] entry point, hardware-aware implementation registry
//! (baseline → SIMD → GPU), versioned wire format, strategy/analysis selection,
//! and 17+ encoding schemes (delta, double-delta, bit-pack, PFor, gorilla,
//! zigzag, RLE, …).
//!
//! It sits at the horizontal layer and depends only on foundation crates
//! (`proximadb-hardware-caps`, `proximadb-config`) plus `proximadb-runtime-common`.
//! Storage engines, the compute module, and modality crates consume it.
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

pub mod adaptive;
pub mod analysis;
pub mod baseline;
pub mod batching;
pub mod codec;
pub mod gpu;
pub mod impls;
pub mod profiling;
pub mod registry;
pub mod simd;
// Optional experimental SIMD entrypoint (forwards to the active SIMD module).
pub mod simd_analysis;
#[cfg(feature = "simd-experimental")]
pub mod simd_experimental;
pub mod strategy;
pub mod traits;
pub mod types;
pub mod wire_format;

// Top-level re-exports
pub use baseline::functions;
pub use baseline::functions::Sq8Params;
pub use baseline::functions::{RaBitQCode, RaBitQParams};
pub use codec::ProximaCodec;
pub use profiling::{
    CompressionBenchmarkRecord, CompressionExplainFields, CompressionStatsProfile,
    CompressionStatsRejectedCandidate,
};
pub use registry::ImplementationRegistry;
pub use simd_analysis::{simd_min_max_f32, simd_zero_count_f32};
pub use strategy::{
    AccessTemperature, AuthorityMode, BlockContext, CodecDecision, CodecParameters,
    CodecSelectionStrategy, ColumnModality, CompressionProfile, CorrelationGroupId, DataAnalysis,
    DataDomain, DictionaryScope, GraphLayoutHint, IntegerAnalysisStrategy, JsonLayoutHint,
    LayoutHints, LossPolicy, MlEmbeddingStrategy, PhysicalOrdering, RandomAccessGranularity,
    RejectedCodecCandidate, RejectionReason, SelectionContext, Sortedness, SparseDataStrategy,
    StorageSpecialization, StrategyRegistry, TimeSeriesStrategy, VectorLayoutHint, WorkloadProfile,
};
pub use traits::{RawDecoder, RawEncoder};
pub use types::{Decodable, Encodable, ProximaScheme, TypeId};
pub use wire_format::{WIRE_FORMAT_VERSION, WireFormatManager, WireHeader};

// Integration tests
#[cfg(test)]
mod tests;
