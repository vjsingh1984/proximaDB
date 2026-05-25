//! Shared ONNX scorer primitives for ProximaDB ranking.
//!
//! Establishes the cache + token + LRU + batch protocol per spec §4.3.
//! Real `ort`-backed [`ScorerSession`] integration is deferred to R-5b
//! and gated behind the `real-onnx` cargo feature; R-5 itself ships
//! mock-driven primitives so the cache / refcount / eviction / batching
//! semantics can land cleanly and be tested without the `ort` dep.
//!
//! Component layout:
//! - [`descriptor`] — `ModelKey`, `ModelDescriptor`, `DType`, framework enum.
//! - [`scorer_session`] — `ScorerSession` trait + `MockScorerSession`.
//! - [`model_cache`] — `OnnxModelCache` + `ScorerToken` + LRU eviction.
//! - [`batched_scorer`] — `BatchedScorer` trait + `OnnxBatchedScorer`
//!   (cross-encoder-style chunked invocation).
//! - [`registry`] — `ModelRegistry` async trait + `InMemoryModelRegistry`.
//! - [`doc_feature_extractor`] — `DocFeatureExtractor` trait + helpers
//!   (R-7c.2; per-doc input row producer for batched scoring).
//! - [`second_phase_scorer`] — `OnnxSecondPhaseScorer` adapter that
//!   satisfies `proximadb_rank_core::SecondPhaseScorer` (R-7c.2).

pub mod batched_scorer;
pub mod descriptor;
pub mod doc_feature_extractor;
pub mod model_cache;
pub mod registry;
pub mod scorer_session;
pub mod second_phase_scorer;

pub use batched_scorer::{BatchInput, BatchOutput, BatchedScorer, OnnxBatchedScorer};
pub use descriptor::{DType, ModelDescriptor, ModelFramework, ModelKey, TensorIoSpec};
pub use doc_feature_extractor::{DocFeatureExtractor, FnDocFeatureExtractor, NoopDocFeatureExtractor};
pub use model_cache::{EvictionPolicy, OnnxModelCache, ScorerToken};
pub use registry::{InMemoryModelRegistry, ModelRegistry};
pub use scorer_session::{MockScorerSession, ScorerSession};
pub use second_phase_scorer::OnnxSecondPhaseScorer;
