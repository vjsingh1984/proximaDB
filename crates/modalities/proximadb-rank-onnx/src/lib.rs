//! Shared ONNX scorer primitives for ProximaDB ranking.
//!
//! Establishes the cache + token + LRU + batch protocol per spec §4.3.
//! Real `ort`-backed [`ScorerSession`] (R-5b) ships behind the
//! `real-onnx` cargo feature; the default build keeps the
//! mock-driven primitives so the cache / refcount / eviction /
//! batching semantics can be tested without pulling in the ort dep.
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
//! - [`ort_scorer_session`] (R-5b, `real-onnx` only) — concrete
//!   `OrtScorerSession` wrapping `ort::Session` for live inference.

pub mod batched_scorer;
pub mod descriptor;
pub mod doc_feature_extractor;
pub mod model_cache;
pub mod registry;
pub mod scorer_session;
pub mod second_phase_scorer;
/// R-5b.1 — tokenized scorer session trait + mock for BERT-family
/// cross-encoders that take int64 token tensors instead of float
/// feature tensors.
pub mod tokenized_scorer_session;

#[cfg(feature = "real-onnx")]
pub mod ort_scorer_session;
#[cfg(feature = "real-onnx")]
/// R-5b.1 — concrete ort-backed `TokenizedScorerSession` for BERT
/// cross-encoders. Gated behind `real-onnx` so the default build
/// stays light.
pub mod ort_tokenized_scorer_session;

pub use batched_scorer::{BatchInput, BatchOutput, BatchedScorer, OnnxBatchedScorer};
pub use descriptor::{DType, ModelDescriptor, ModelFramework, ModelKey, TensorIoSpec};
pub use doc_feature_extractor::{DocFeatureExtractor, FnDocFeatureExtractor, NoopDocFeatureExtractor};
pub use model_cache::{EvictionPolicy, OnnxModelCache, ScorerToken};
pub use registry::{InMemoryModelRegistry, ModelRegistry};
pub use scorer_session::{MockScorerSession, ScorerSession};
pub use second_phase_scorer::OnnxSecondPhaseScorer;
pub use tokenized_scorer_session::{
    MockTokenizedScorerSession, TokenizedBatch, TokenizedScorerSession,
};

#[cfg(feature = "real-onnx")]
pub use ort_scorer_session::OrtScorerSession;
#[cfg(feature = "real-onnx")]
pub use ort_tokenized_scorer_session::OrtTokenizedScorerSession;
