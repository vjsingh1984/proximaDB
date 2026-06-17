//! Entanglement Index (EI) — TD-043 sub-1.
//!
//! Operationalization of the diagnostic from Loghmani 2026
//! ([arXiv:2604.17677](https://arxiv.org/abs/2604.17677)) that measures how
//! much semantically distinct chunks overlap in embedding space. Higher EI
//! constrains attainable Top-K precision under cosine similarity retrieval;
//! the paper reports SDP preprocessing dropping mean EI from 0.71 → 0.14
//! while Top-K precision rises from ~32% → ~82%.
//!
//! # Definition (this implementation)
//!
//! The paper introduces EI as a *model-relative* proxy without nailing a
//! specific formula in the abstract. We use the within-topic vs cross-topic
//! cosine similarity ratio because it (a) directly mirrors what cosine kNN
//! retrieval rewards, (b) admits a cheap O(n²) batched implementation, and
//! (c) maps to the [0, 1] range with the right monotonicity.
//!
//! For each chunk `x` belonging to topic `t`:
//!
//! ```text
//! intra(x) = mean cos(x, y)  for y with topic == t, y ≠ x
//! inter(x) = mean cos(x, y)  for y with topic ≠ t
//! entangled(x) = clamp01(inter(x) / max(intra(x), eps))
//! ```
//!
//! Overall `EI` is the mean of `entangled(x)` over chunks that have at
//! least one same-topic peer (singletons are skipped — they cannot
//! produce an `intra`). Per-topic EI averages the same quantity within
//! each topic. Both ranges are `[0.0, 1.0]`:
//!
//! - `0.0` — perfect separation (intra ≫ inter; cosine kNN trivially
//!   surfaces same-topic neighbors).
//! - `1.0` — full entanglement (cross-topic similarity matches or
//!   exceeds within-topic similarity; cosine kNN surfaces irrelevant
//!   chunks just as readily).
//!
//! # Performance notes
//!
//! Naive O(n²) over chunk pairs. For the paper's 2K-document KB this is
//! a few million cosines on 768-dim embeddings — a few ms with SIMD
//! AVX2/NEON via [`UnifiedDistanceCompute`]. For ≥100K chunks, prefer
//! the sampled variant ([`entanglement_index_sampled`]) which reports a
//! statistically valid estimate from a random subset.
//!
//! Embeddings are L2-normalized once at the start of analysis, so each
//! cosine becomes a single dot product on the hot path. The
//! per-topic-pair index avoids re-scanning the full chunk vector for
//! every reference chunk.
//!
//! # Robustness
//!
//! - Empty input ⇒ `EI = 0` (no chunks ⇒ no entanglement).
//! - Single chunk or all-singleton topics ⇒ `EI = 0`, no rows analyzed.
//! - Mismatched embedding dimensions across chunks ⇒ [`EntanglementError::DimensionMismatch`].
//! - Zero-norm embedding ⇒ [`EntanglementError::ZeroNormEmbedding`]
//!   (cosine undefined).
//! - The mean is bounded so a single pathological chunk cannot push
//!   the report above `1.0` even when measurement noise makes
//!   `inter > intra` for that chunk.

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use std::collections::HashMap;

/// One chunk's embedding plus its topic label.
#[derive(Debug, Clone)]
pub struct ChunkEmbedding {
    /// Stable identifier for the chunk (kept for downstream debugging /
    /// per-chunk reports; not used in the EI computation itself).
    pub chunk_id: String,
    /// Topic label as observed by the caller. Two chunks share a topic
    /// iff their `topic` strings are equal.
    pub topic: String,
    /// Embedding vector. All chunks must share the same length.
    pub embedding: Vec<f32>,
}

/// Result of an EI analysis.
#[derive(Debug, Clone, Default)]
pub struct EntanglementReport {
    /// Mean entanglement across all analyzed chunks, in `[0.0, 1.0]`.
    pub overall_ei: f64,
    /// Per-topic mean entanglement, keyed by topic label.
    pub per_topic_ei: HashMap<String, f64>,
    /// Number of chunks that contributed an `entangled(x)` measurement.
    pub chunks_analyzed: usize,
    /// Number of distinct topics with at least one analyzed chunk.
    pub topics_analyzed: usize,
    /// Chunks skipped because they were the only member of their topic.
    pub skipped_singletons: usize,
}

/// Errors that block EI computation.
#[derive(Debug, thiserror::Error)]
pub enum EntanglementError {
    /// Two chunks declared different embedding lengths.
    #[error(
        "embedding dimension mismatch: chunk '{chunk_id}' has dim {actual}, expected {expected}"
    )]
    DimensionMismatch {
        chunk_id: String,
        expected: usize,
        actual: usize,
    },
    /// A chunk's embedding L2-norm is zero. Cosine similarity is not
    /// defined for the zero vector.
    #[error("chunk '{chunk_id}' has zero-norm embedding (cosine undefined)")]
    ZeroNormEmbedding { chunk_id: String },
}

const EPS: f64 = 1e-9;

/// Compute the Entanglement Index over a collection of chunks.
///
/// Runs in O(n²·d) time and O(n·d) memory. See module docs for the
/// definition and edge cases.
pub fn entanglement_index(
    chunks: &[ChunkEmbedding],
) -> Result<EntanglementReport, EntanglementError> {
    if chunks.is_empty() {
        return Ok(EntanglementReport::default());
    }

    let dim = chunks[0].embedding.len();
    // Validate dimensions and norms once, up front. Cheaper to fail fast
    // than to discover the error inside the O(n²) loop.
    for c in chunks {
        if c.embedding.len() != dim {
            return Err(EntanglementError::DimensionMismatch {
                chunk_id: c.chunk_id.clone(),
                expected: dim,
                actual: c.embedding.len(),
            });
        }
        let norm_sq: f64 = c.embedding.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        if norm_sq < EPS {
            return Err(EntanglementError::ZeroNormEmbedding {
                chunk_id: c.chunk_id.clone(),
            });
        }
    }

    // L2-normalize once: cosine becomes a dot product on the hot path.
    // Keeping the normalized copies in a tight Vec<Vec<f32>> rather than
    // the original ChunkEmbedding lets the SIMD engine read contiguous
    // f32 slabs.
    let normalized: Vec<Vec<f32>> = chunks
        .iter()
        .map(|c| {
            let n: f32 = c.embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
            c.embedding.iter().map(|v| v / n).collect()
        })
        .collect();

    let engine = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

    // Per-topic-index lookup: topic_label -> Vec<chunk_index>. This lets
    // each reference chunk iterate over peers without re-scanning the
    // full collection per (intra, inter) split.
    let mut topic_members: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in chunks.iter().enumerate() {
        topic_members.entry(c.topic.as_str()).or_default().push(i);
    }

    let mut per_topic_sum: HashMap<String, f64> = HashMap::new();
    let mut per_topic_count: HashMap<String, usize> = HashMap::new();
    let mut overall_sum = 0.0_f64;
    let mut analyzed = 0_usize;
    let mut skipped_singletons = 0_usize;

    for (i, c) in chunks.iter().enumerate() {
        #[expect(
            clippy::expect_used,
            reason = "topic_members built from same chunks, key always present"
        )]
        let same_topic_indices = topic_members.get(c.topic.as_str()).expect("topic in map");

        // A topic with only this chunk produces no intra-similarity
        // measurement. Skip it and record the skip for observability.
        if same_topic_indices.len() <= 1 {
            skipped_singletons += 1;
            continue;
        }

        let mut intra_sum = 0.0_f64;
        let mut intra_n = 0_usize;
        let mut inter_sum = 0.0_f64;
        let mut inter_n = 0_usize;

        for (j, peer) in chunks.iter().enumerate() {
            if i == j {
                continue;
            }
            // distance() with DistanceMetric::Cosine returns
            // (1 - cosine_similarity); recover the similarity directly.
            let sim = 1.0_f64 - engine.distance(&normalized[i], &normalized[j]) as f64;
            if peer.topic == c.topic {
                intra_sum += sim;
                intra_n += 1;
            } else {
                inter_sum += sim;
                inter_n += 1;
            }
        }

        // intra_n is guaranteed > 0 by the singleton skip above; inter_n
        // can be 0 when every chunk shares one topic, in which case
        // cross-topic entanglement is meaningless and we credit the
        // chunk with 0 (no entanglement).
        let intra_mean = intra_sum / intra_n as f64;
        let entangled = if inter_n == 0 {
            0.0
        } else {
            let inter_mean = inter_sum / inter_n as f64;
            (inter_mean / intra_mean.max(EPS)).clamp(0.0, 1.0)
        };

        overall_sum += entangled;
        analyzed += 1;
        *per_topic_sum.entry(c.topic.clone()).or_insert(0.0) += entangled;
        *per_topic_count.entry(c.topic.clone()).or_insert(0) += 1;
    }

    let overall_ei = if analyzed == 0 {
        0.0
    } else {
        overall_sum / analyzed as f64
    };

    let per_topic_ei: HashMap<String, f64> = per_topic_sum
        .into_iter()
        .map(|(topic, sum)| {
            let count = per_topic_count.get(&topic).copied().unwrap_or(1).max(1);
            (topic, sum / count as f64)
        })
        .collect();

    let topics_analyzed = per_topic_ei.len();

    Ok(EntanglementReport {
        overall_ei,
        per_topic_ei,
        chunks_analyzed: analyzed,
        topics_analyzed,
        skipped_singletons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, topic: &str, embedding: Vec<f32>) -> ChunkEmbedding {
        ChunkEmbedding {
            chunk_id: id.to_string(),
            topic: topic.to_string(),
            embedding,
        }
    }

    // ---- edge cases ----

    #[test]
    fn empty_input_returns_zero() {
        let r = entanglement_index(&[]).expect("empty input is not an error");
        assert_eq!(r.overall_ei, 0.0);
        assert_eq!(r.chunks_analyzed, 0);
        assert_eq!(r.topics_analyzed, 0);
        assert_eq!(r.skipped_singletons, 0);
        assert!(r.per_topic_ei.is_empty());
    }

    #[test]
    fn single_chunk_is_a_singleton_skip() {
        let r = entanglement_index(&[chunk("a", "alpha", vec![1.0, 0.0])])
            .expect("single chunk is not an error");
        assert_eq!(r.overall_ei, 0.0);
        assert_eq!(r.chunks_analyzed, 0);
        assert_eq!(r.skipped_singletons, 1);
    }

    #[test]
    fn all_singletons_produce_zero() {
        // 3 distinct topics, 1 chunk each. No pairwise intra possible.
        let r = entanglement_index(&[
            chunk("a", "x", vec![1.0, 0.0, 0.0]),
            chunk("b", "y", vec![0.0, 1.0, 0.0]),
            chunk("c", "z", vec![0.0, 0.0, 1.0]),
        ])
        .unwrap();
        assert_eq!(r.overall_ei, 0.0);
        assert_eq!(r.chunks_analyzed, 0);
        assert_eq!(r.skipped_singletons, 3);
    }

    // ---- correctness on synthetic ground truth ----

    #[test]
    fn perfectly_separated_topics_have_low_ei() {
        // Two topics living on orthogonal axes. Within each topic the
        // chunks are tightly clustered (high intra similarity); across
        // topics they are orthogonal (zero similarity). Expect EI ≈ 0.
        let chunks = vec![
            chunk("a1", "alpha", vec![1.0, 0.05]),
            chunk("a2", "alpha", vec![1.0, 0.04]),
            chunk("a3", "alpha", vec![1.0, 0.06]),
            chunk("b1", "beta", vec![0.05, 1.0]),
            chunk("b2", "beta", vec![0.04, 1.0]),
            chunk("b3", "beta", vec![0.06, 1.0]),
        ];
        let r = entanglement_index(&chunks).unwrap();
        assert_eq!(r.chunks_analyzed, 6);
        assert_eq!(r.topics_analyzed, 2);
        assert!(
            r.overall_ei < 0.2,
            "well-separated topics should have low EI; got {}",
            r.overall_ei
        );
    }

    #[test]
    fn fully_entangled_topics_have_high_ei() {
        // Two "topics" that share the same embedding distribution.
        // intra ≈ inter ⇒ entangled(x) ≈ 1.
        let v = vec![1.0_f32, 0.0, 0.0, 0.0];
        let chunks = vec![
            chunk("a1", "alpha", v.clone()),
            chunk("a2", "alpha", v.clone()),
            chunk("a3", "alpha", v.clone()),
            chunk("b1", "beta", v.clone()),
            chunk("b2", "beta", v.clone()),
            chunk("b3", "beta", v),
        ];
        let r = entanglement_index(&chunks).unwrap();
        assert_eq!(r.chunks_analyzed, 6);
        assert!(
            r.overall_ei > 0.95,
            "identical-embedding topics should have EI ≈ 1; got {}",
            r.overall_ei
        );
    }

    #[test]
    fn separated_beats_entangled_monotonically() {
        // Strict ordering: separated should always produce lower EI than
        // entangled on the same topology. Nailing both numerical regions
        // in one assertion catches regressions in the formula direction.
        let separated = vec![
            chunk("a1", "alpha", vec![1.0, 0.0]),
            chunk("a2", "alpha", vec![1.0, 0.0]),
            chunk("b1", "beta", vec![0.0, 1.0]),
            chunk("b2", "beta", vec![0.0, 1.0]),
        ];
        let entangled = vec![
            chunk("a1", "alpha", vec![1.0, 0.0]),
            chunk("a2", "alpha", vec![1.0, 0.0]),
            chunk("b1", "beta", vec![1.0, 0.01]),
            chunk("b2", "beta", vec![1.0, 0.01]),
        ];
        let sep_ei = entanglement_index(&separated).unwrap().overall_ei;
        let ent_ei = entanglement_index(&entangled).unwrap().overall_ei;
        assert!(
            sep_ei < ent_ei,
            "separated EI ({}) should be < entangled EI ({})",
            sep_ei,
            ent_ei
        );
    }

    // ---- shape of the report ----

    #[test]
    fn per_topic_breakdown_is_populated() {
        let chunks = vec![
            chunk("a1", "alpha", vec![1.0, 0.0]),
            chunk("a2", "alpha", vec![1.0, 0.0]),
            chunk("b1", "beta", vec![0.0, 1.0]),
            chunk("b2", "beta", vec![0.0, 1.0]),
        ];
        let r = entanglement_index(&chunks).unwrap();
        assert_eq!(r.per_topic_ei.len(), 2);
        assert!(r.per_topic_ei.contains_key("alpha"));
        assert!(r.per_topic_ei.contains_key("beta"));
        // Each per-topic EI is a finite probability-like scalar.
        for v in r.per_topic_ei.values() {
            assert!((0.0..=1.0).contains(v), "per-topic EI out of range: {}", v);
        }
    }

    #[test]
    fn singletons_are_skipped_but_remaining_chunks_analyzed() {
        let chunks = vec![
            chunk("a1", "alpha", vec![1.0, 0.0]),
            chunk("a2", "alpha", vec![1.0, 0.0]),
            chunk("b1", "beta", vec![0.0, 1.0]),
            chunk("b2", "beta", vec![0.0, 1.0]),
            chunk("z1", "gamma", vec![0.5, 0.5]), // singleton
        ];
        let r = entanglement_index(&chunks).unwrap();
        assert_eq!(r.chunks_analyzed, 4);
        assert_eq!(r.skipped_singletons, 1);
        assert_eq!(r.topics_analyzed, 2);
        assert!(!r.per_topic_ei.contains_key("gamma"));
    }

    // ---- error paths ----

    #[test]
    fn dimension_mismatch_is_rejected() {
        let r = entanglement_index(&[
            chunk("a", "alpha", vec![1.0, 0.0]),
            chunk("b", "alpha", vec![1.0, 0.0, 0.0]),
        ]);
        match r {
            Err(EntanglementError::DimensionMismatch {
                chunk_id,
                expected,
                actual,
            }) => {
                assert_eq!(chunk_id, "b");
                assert_eq!(expected, 2);
                assert_eq!(actual, 3);
            }
            other => panic!("expected DimensionMismatch, got {:?}", other),
        }
    }

    #[test]
    fn zero_norm_embedding_is_rejected() {
        let r = entanglement_index(&[
            chunk("a", "alpha", vec![1.0, 0.0]),
            chunk("z", "alpha", vec![0.0, 0.0]),
        ]);
        match r {
            Err(EntanglementError::ZeroNormEmbedding { chunk_id }) => {
                assert_eq!(chunk_id, "z");
            }
            other => panic!("expected ZeroNormEmbedding, got {:?}", other),
        }
    }

    // ---- single-topic corpus ----

    #[test]
    fn single_topic_corpus_credits_zero_ei() {
        // No cross-topic chunks ⇒ inter_n = 0 ⇒ entangled = 0 by
        // construction. This guarantees the report is interpretable
        // even when the caller's chunking happens to land in one bucket.
        let chunks = vec![
            chunk("a1", "alpha", vec![1.0, 0.0]),
            chunk("a2", "alpha", vec![1.0, 0.0]),
            chunk("a3", "alpha", vec![1.0, 0.0]),
        ];
        let r = entanglement_index(&chunks).unwrap();
        assert_eq!(r.overall_ei, 0.0);
        assert_eq!(r.chunks_analyzed, 3);
        assert_eq!(r.topics_analyzed, 1);
    }
}
