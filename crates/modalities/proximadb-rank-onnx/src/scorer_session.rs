//! [`ScorerSession`] trait — the abstract scorer interface the cache
//! holds and `BatchedScorer` consumes.
//!
//! v1 keeps the surface deliberately minimal so the real `ort`-backed
//! impl (R-5b, feature-gated) and the test `MockScorerSession` can both
//! satisfy it.

use crate::descriptor::ModelDescriptor;
use proximadb_rank_core::RankResult;
use std::sync::atomic::{AtomicU64, Ordering};

/// Abstract scorer session — loaded model state plus a `score` entry
/// point. Sessions are immutable post-load (so they live behind `Arc`
/// in the cache and `Send + Sync` is required).
///
/// Inputs and outputs are concrete-impl-specific. v1 uses flat
/// `Vec<f32>` per row so a mock can produce deterministic scores from
/// closure-supplied logic; the `ort` impl will pre-tokenize at the
/// `BatchedScorer` boundary and call its native inputs API directly.
pub trait ScorerSession: Send + Sync + 'static {
    fn descriptor(&self) -> &ModelDescriptor;

    /// Estimated bytes of resident memory. Drives LRU eviction.
    /// Default delegates to the descriptor estimate.
    fn memory_bytes(&self) -> usize {
        self.descriptor().estimated_memory_bytes()
    }

    /// Score N rows. Each input row is a flat `Vec<f32>`. Returns one
    /// `f32` score per row. Errors propagate as [`proximadb_rank_core::RankError::ModelInference`].
    fn score(&self, rows: &[Vec<f32>]) -> RankResult<Vec<f32>>;
}

/// Boxed scoring closure used by [`MockScorerSession`]. Extracted to a
/// type alias to keep clippy happy and the struct definition readable.
type ScoringFn = Box<dyn Fn(&[Vec<f32>]) -> Vec<f32> + Send + Sync>;

/// Test-only [`ScorerSession`] impl that returns deterministic scores
/// via a closure. Useful for testing the cache + batching code paths
/// without dragging in `ort`.
pub struct MockScorerSession {
    descriptor: ModelDescriptor,
    scoring_fn: ScoringFn,
    call_count: AtomicU64,
    rows_seen: AtomicU64,
}

impl MockScorerSession {
    /// Build a mock that returns `f(rows)` on each call. The closure
    /// must produce exactly one score per input row.
    pub fn new<F>(descriptor: ModelDescriptor, scoring_fn: F) -> Self
    where
        F: Fn(&[Vec<f32>]) -> Vec<f32> + Send + Sync + 'static,
    {
        Self {
            descriptor,
            scoring_fn: Box::new(scoring_fn),
            call_count: AtomicU64::new(0),
            rows_seen: AtomicU64::new(0),
        }
    }

    /// Shortcut: each row scores `0.0`.
    pub fn zeros(descriptor: ModelDescriptor) -> Self {
        Self::new(descriptor, |rows| vec![0.0; rows.len()])
    }

    /// Shortcut: each row scores `score_per_row` (constant).
    pub fn constant(descriptor: ModelDescriptor, score_per_row: f32) -> Self {
        Self::new(descriptor, move |rows| vec![score_per_row; rows.len()])
    }

    /// How many times `score()` has been called.
    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Total rows scored across all calls.
    pub fn rows_seen(&self) -> u64 {
        self.rows_seen.load(Ordering::SeqCst)
    }
}

impl ScorerSession for MockScorerSession {
    fn descriptor(&self) -> &ModelDescriptor {
        &self.descriptor
    }

    fn score(&self, rows: &[Vec<f32>]) -> RankResult<Vec<f32>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.rows_seen
            .fetch_add(rows.len() as u64, Ordering::SeqCst);
        let scores = (self.scoring_fn)(rows);
        debug_assert_eq!(
            scores.len(),
            rows.len(),
            "MockScorerSession closure must return one score per row"
        );
        Ok(scores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelFramework, ModelKey};

    fn d() -> ModelDescriptor {
        ModelDescriptor {
            key: ModelKey::new("x", "1"),
            tenant: None,
            uri: "file:///tmp/x.onnx".into(),
            sha256: [0; 32],
            size_bytes: 1024,
            framework: ModelFramework::Onnx,
            dtype: DType::Fp32,
            input_spec: vec![],
            output_spec: vec![],
            max_batch_size: 8,
            seq: 0,
            created_at_ms: 0,
        }
    }

    #[test]
    fn mock_zeros_returns_zero_scores() {
        let s = MockScorerSession::zeros(d());
        let out = s.score(&[vec![1.0], vec![2.0], vec![3.0]]).unwrap();
        assert_eq!(out, vec![0.0, 0.0, 0.0]);
        assert_eq!(s.call_count(), 1);
        assert_eq!(s.rows_seen(), 3);
    }

    #[test]
    fn mock_constant_returns_same_score_per_row() {
        let s = MockScorerSession::constant(d(), 0.5);
        let rows: Vec<Vec<f32>> = vec![vec![0.0]; 5];
        let out = s.score(&rows).unwrap();
        assert_eq!(out, vec![0.5; 5]);
    }

    #[test]
    fn mock_counter_accumulates_across_calls() {
        let s = MockScorerSession::zeros(d());
        s.score(&[vec![0.0]]).unwrap();
        s.score(&[vec![0.0], vec![1.0]]).unwrap();
        s.score(&[vec![0.0], vec![1.0], vec![2.0]]).unwrap();
        assert_eq!(s.call_count(), 3);
        assert_eq!(s.rows_seen(), 6);
    }

    #[test]
    fn mock_memory_bytes_defaults_to_descriptor_estimate() {
        let s = MockScorerSession::zeros(d());
        assert_eq!(s.memory_bytes(), 1024);
    }

    #[test]
    fn mock_with_custom_closure() {
        let s = MockScorerSession::new(d(), |rows| rows.iter().map(|r| r[0] * 2.0).collect());
        let out = s.score(&[vec![1.0], vec![5.0], vec![10.0]]).unwrap();
        assert_eq!(out, vec![2.0, 10.0, 20.0]);
    }
}
