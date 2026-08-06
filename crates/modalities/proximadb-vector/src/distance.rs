//! # Vector Distance Metrics
//!
//! Distance-metric *types and conversions* for the vector modality.
//!
//! The actual distance computation engine (SIMD kernels, `SimilarityResult`,
//! `UnifiedDistanceCompute`) lives in the `proximadb-distance-kernel`
//! foundation crate — this module deliberately carries NO compute. A full
//! duplicate engine (engine/impls/avx512/int8_simd + a second
//! `SimilarityResult` struct) used to live here with zero external users;
//! it was removed under TD-SEARCH-1 to keep one score/distance
//! implementation in the workspace.

pub mod conversion;

// Re-export proto DistanceMetric — the single canonical metric enum
// (identical to `proximadb_distance_kernel::DistanceMetric`).
pub use proximadb_proto::v1::DistanceMetric;

pub use conversion::{
    get_distance_metric_from_config, internal_distance_to_proto, proto_distance_to_internal,
};
