// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Pure-Rust columnar encoding/decoding algorithms for ProximaDB.
//!
//! This crate contains the zero-SIMD-dependency baseline codec types,
//! data-pattern analysis, strategy selection, and 17+ encoding schemes
//! (delta, double-delta, bit-pack, PFor, gorilla, zigzag, RLE, …).
//!
//! It has no upward dependencies — it sits at the horizontal layer and
//! may be consumed by storage engines, the compute module, and modality crates.

pub mod analysis;
pub mod baseline;
pub mod profiling;
pub mod simd_analysis;
pub mod strategy;
pub mod types;

// Top-level re-exports
pub use baseline::functions;
pub use baseline::functions::Sq8Params;
pub use baseline::functions::{RaBitQCode, RaBitQParams};
pub use profiling::{
    CompressionBenchmarkRecord, CompressionExplainFields, CompressionStatsProfile,
    CompressionStatsRejectedCandidate,
};
pub use simd_analysis::{simd_min_max_f32, simd_zero_count_f32};
pub use strategy::{
    AccessTemperature, AuthorityMode, BlockContext, CodecDecision, CodecParameters,
    CodecSelectionStrategy, ColumnModality, CompressionProfile, CorrelationGroupId, DataAnalysis,
    DataDomain, DictionaryScope, GraphLayoutHint, IntegerAnalysisStrategy, JsonLayoutHint,
    LayoutHints, LossPolicy, MlEmbeddingStrategy, PhysicalOrdering, RandomAccessGranularity,
    RejectedCodecCandidate, RejectionReason, SelectionContext, Sortedness, SparseDataStrategy,
    StorageSpecialization, StrategyRegistry, TimeSeriesStrategy, VectorLayoutHint, WorkloadProfile,
};
pub use types::{Decodable, Encodable, ProximaScheme, TypeId};
