// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! HMGI (Hierarchical Modality-aware Graph Index) parameter
//! advisor — sizes per-modality HNSW partitions for collections
//! that declare multiple modalities.
//!
//! # When HMGI is the right algorithm
//!
//! HMGI is **not** an alternative to HNSW for single-modality
//! collections — single-partition HMGI is empirically identical
//! to standalone HNSW (verified by the matrix bench at m=16, 32,
//! 48: HMGI tracked HNSW to 4 decimal places when there was only
//! one partition).
//!
//! HMGI's value proposition activates when:
//!
//! * The collection holds vectors from **multiple modalities**
//!   (text + image + video, e.g. CLIP-style multi-modal corpora).
//! * Queries can be **modality-scoped** — most queries touch one
//!   or two modalities, not all of them.
//! * The combined corpus is large enough that a single HNSW
//!   would mix modalities into the same graph, degrading recall
//!   because cross-modality neighbours are nearly random.
//!
//! In that regime HMGI **partitions by modality tag**: each
//! partition is a standalone HNSW built only on vectors of one
//! modality, and the [`crate::index::axis::hmgi::HmgiRouter`]
//! routes queries to the relevant partitions only.
//!
//! # Sizing approach
//!
//! HMGI's own knobs are minimal: it just decides
//! `(partition_count, max_partitions_per_query)`. The per-partition
//! HNSW knobs (`m`, `ef_construction`, `ef_search`) are sized by
//! the [`super::HnswIndexAdvisor`] — one independent call per
//! modality. This composition is the whole point of using HNSW
//! under HMGI: every modality benefits from the same
//! formula-driven sizing the standalone HNSW advisor already
//! delivers.
//!
//! # Cost model
//!
//! Per-partition memory + work models reuse the HNSW estimates
//! (`m·4·dim·N_partition` for edges + `dim·4·N_partition` for
//! vectors). Total:
//!
//! ```text
//!   estimated_memory_mb       = Σ over modalities (HNSW memory)
//!   estimated_per_query_work  = max over probed modalities (HNSW work)
//!                               × max_partitions_per_query
//! ```
//!
//! The **max** rather than sum captures parallel fan-out: HMGI's
//! `HmgiRouter::search_partitions` launches partition queries in
//! parallel via tokio JoinSet, so wall-clock latency is bounded
//! by the slowest partition × parallelism overhead.
//!
//! # Recall semantics
//!
//! HMGI's end-to-end recall is bounded by the **worst** per-
//! partition recall — if the text partition delivers 0.95 and
//! the image partition delivers 0.90, the joint recall for a
//! multi-modal query is closer to 0.90. The advisor reports
//! `projected_recall_if_clamped = min(per-partition recall)`
//! when any partition's HNSW advisor clamps by budget.

use crate::index::axis::management::HnswIndexAdvisor;
use crate::index::axis::management::ann_advisor::{
    AnnAdvisorInput, AnnAdvisorOutput, AnnIndexAdvisor, SupportedAlgorithm,
};
use crate::index::axis::types::{HmgiPartitionAlgo, IndexAlgorithm};

/// HMGI impl of [`AnnIndexAdvisor`]. Stateless. Delegates per-
/// modality HNSW sizing to the [`HnswIndexAdvisor`] internally.
pub struct HmgiIndexAdvisor;

impl HmgiIndexAdvisor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HmgiIndexAdvisor {
    fn default() -> Self {
        Self::new()
    }
}

impl AnnIndexAdvisor for HmgiIndexAdvisor {
    fn algorithm(&self) -> SupportedAlgorithm {
        SupportedAlgorithm::Hmgi
    }

    fn advise(&self, input: &AnnAdvisorInput) -> Option<AnnAdvisorOutput> {
        // HMGI activates only when the collection declares ≥ 2
        // modalities. Single-modality collections get standalone
        // HNSW (the selector handles that via tie-break).
        if input.modalities.len() < 2 {
            return None;
        }

        // Per-partition vector count estimate. The advisor doesn't
        // know the actual per-modality split today; assume
        // uniform distribution. Operators with skewed modality
        // counts can override later via a `modality_weights:` tag
        // (P5+).
        let num_modalities = input.modalities.len();
        let per_partition_n = input.vector_count / num_modalities as u64;

        let hnsw_advisor = HnswIndexAdvisor::new();
        let mut per_modality_partitions: Vec<HmgiPartitionAlgo> =
            Vec::with_capacity(num_modalities);
        let mut total_memory_mb = 0.0_f64;
        let mut max_per_query_work = 0_u64;
        let mut any_clamped = false;
        let mut worst_projected_recall: Option<f32> = None;

        for modality in &input.modalities {
            // Each partition gets a fresh HnswIndexAdvisor call —
            // honors max_query_latency_ms / max_memory_mb per
            // partition. Memory budget is divided across
            // partitions so the selector's overall cap stays
            // honored.
            let per_partition_input = AnnAdvisorInput {
                vector_count: per_partition_n,
                top_k: input.top_k,
                recall_target: input.recall_target,
                dimension: input.dimension,
                distance_metric: input.distance_metric,
                max_query_latency_ms: input.max_query_latency_ms,
                max_memory_mb: input.max_memory_mb.map(|cap| cap / num_modalities as f64),
                binary_rerank_allowed: input.binary_rerank_allowed,
                // Clear modalities for the inner call so the
                // HNSW advisor doesn't loop back into HMGI.
                modalities: Vec::new(),
            };
            let partition_out = hnsw_advisor.advise(&per_partition_input)?;

            let (m, ef_construction, ef_search) = match &partition_out.algorithm {
                IndexAlgorithm::HNSW {
                    m,
                    ef_construction,
                    ef_search,
                    ..
                } => (*m, *ef_construction, *ef_search),
                _ => return None, // HNSW advisor should always return HNSW
            };

            per_modality_partitions.push(HmgiPartitionAlgo {
                modality_tag: modality.clone(),
                m,
                ef_construction,
                ef_search,
            });

            total_memory_mb += partition_out.estimated_memory_mb;
            max_per_query_work = max_per_query_work.max(partition_out.estimated_per_query_work);

            if partition_out.clamped_by_budget {
                any_clamped = true;
                if let Some(projected) = partition_out.projected_recall {
                    worst_projected_recall = Some(match worst_projected_recall {
                        Some(prev) => prev.min(projected),
                        None => projected,
                    });
                }
            }
        }

        // P3 default: every query fans out to every modality
        // partition (full coverage). Operators with modality-scoped
        // workloads can override via a `max_partitions_per_query:`
        // tag (P5+).
        let max_partitions_per_query = num_modalities as u32;

        let algorithm = IndexAlgorithm::HMGI {
            per_modality: per_modality_partitions.clone(),
            max_partitions_per_query,
        };

        let rationale = format!(
            "hmgi modalities={} partitions={} max_fanout={} per_partition_n={} \
             memory≈{:.1}MB max_work={}{}",
            input.modalities.join(","),
            per_modality_partitions.len(),
            max_partitions_per_query,
            per_partition_n,
            total_memory_mb,
            max_per_query_work,
            if any_clamped { " (clamped)" } else { "" },
        );

        Some(AnnAdvisorOutput {
            algorithm,
            kind: SupportedAlgorithm::Hmgi,
            clamped_by_budget: any_clamped,
            projected_recall: worst_projected_recall,
            estimated_memory_mb: total_memory_mb,
            // Parallel fan-out: wall-clock work is bounded by the
            // slowest partition. The selector compares this
            // against latency budgets.
            estimated_per_query_work: max_per_query_work,
            rationale,
        })
    }

    fn recall_for(&self, algorithm: &IndexAlgorithm, vector_count: u64, top_k: u32) -> Option<f32> {
        let IndexAlgorithm::HMGI { per_modality, .. } = algorithm else {
            return None;
        };
        if per_modality.is_empty() {
            return None;
        }
        // Joint recall is bounded by the worst per-partition
        // HNSW recall. Each partition holds vector_count /
        // num_modalities vectors.
        let per_partition_n = vector_count / per_modality.len() as u64;
        let mut worst: Option<f32> = None;
        for partition in per_modality {
            let pred = crate::index::axis::management::hnsw_param_advisor::recall_for_ef(
                partition.m,
                partition.ef_search,
                per_partition_n,
                top_k,
            );
            worst = Some(match worst {
                Some(prev) => prev.min(pred),
                None => pred,
            });
        }
        worst
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::DistanceMetric;

    fn hmgi_input(recall: f32, modalities: Vec<&str>) -> AnnAdvisorInput {
        AnnAdvisorInput {
            vector_count: 100_000,
            top_k: 10,
            recall_target: recall,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_query_latency_ms: None,
            max_memory_mb: None,
            binary_rerank_allowed: false,
            modalities: modalities.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn hmgi_advisor_declines_when_no_modalities() {
        let advisor = HmgiIndexAdvisor::new();
        let input = hmgi_input(0.95, vec![]);
        assert!(
            advisor.advise(&input).is_none(),
            "HMGI must decline single-modality collections"
        );
    }

    #[test]
    fn hmgi_advisor_declines_when_one_modality() {
        let advisor = HmgiIndexAdvisor::new();
        let input = hmgi_input(0.95, vec!["text"]);
        assert!(
            advisor.advise(&input).is_none(),
            "HMGI must decline single-modality (one entry) collections"
        );
    }

    #[test]
    fn hmgi_advisor_sizes_per_modality() {
        let advisor = HmgiIndexAdvisor::new();
        let input = hmgi_input(0.95, vec!["text", "image", "video"]);
        let out = advisor.advise(&input).expect("3 modalities must size");
        assert_eq!(out.kind, SupportedAlgorithm::Hmgi);
        match out.algorithm {
            IndexAlgorithm::HMGI {
                per_modality,
                max_partitions_per_query,
            } => {
                assert_eq!(per_modality.len(), 3);
                assert_eq!(max_partitions_per_query, 3);
                // Each per-modality partition got HNSW-style sizing.
                for partition in &per_modality {
                    assert!(partition.m >= 8 && partition.m <= 56);
                    assert!(partition.ef_construction >= 100);
                    assert!(partition.ef_search >= 16);
                }
                // Modality tags preserved in order.
                let tags: Vec<&str> = per_modality
                    .iter()
                    .map(|p| p.modality_tag.as_str())
                    .collect();
                assert_eq!(tags, vec!["text", "image", "video"]);
            }
            other => panic!("expected HMGI, got {:?}", other),
        }
    }

    #[test]
    fn hmgi_per_partition_n_is_total_divided_by_modality_count() {
        // At 100K total with 4 modalities, each partition gets
        // 25K vectors. Smaller N → smaller ef_search per partition
        // (the HNSW advisor scales ef with log(N)). So the per-
        // partition ef_search is lower than a single-modality
        // collection's ef_search at the same total N.
        let advisor = HmgiIndexAdvisor::new();
        let four = advisor
            .advise(&hmgi_input(0.95, vec!["a", "b", "c", "d"]))
            .unwrap();
        let two = advisor.advise(&hmgi_input(0.95, vec!["a", "b"])).unwrap();

        let ef_four = match &four.algorithm {
            IndexAlgorithm::HMGI { per_modality, .. } => per_modality[0].ef_search,
            _ => unreachable!(),
        };
        let ef_two = match &two.algorithm {
            IndexAlgorithm::HMGI { per_modality, .. } => per_modality[0].ef_search,
            _ => unreachable!(),
        };
        assert!(
            ef_four <= ef_two,
            "More modalities → smaller per-partition N → lower ef. \
             4-mod ef={} should be ≤ 2-mod ef={}",
            ef_four,
            ef_two
        );
    }

    #[test]
    fn hmgi_estimated_memory_sums_partitions() {
        // Memory is summed across partitions — operators should see
        // the total memory the HMGI collection occupies.
        let advisor = HmgiIndexAdvisor::new();
        let out = advisor
            .advise(&hmgi_input(0.95, vec!["text", "image"]))
            .unwrap();
        // Conservative bound: at least 2x a single-partition's
        // memory contribution. The advisor's per-partition HNSW
        // memory model isn't algorithmically precise but we can
        // at least check the order of magnitude.
        assert!(
            out.estimated_memory_mb >= 10.0,
            "estimated_memory_mb {} too low for 2 partitions × 50K vectors × 128 dim",
            out.estimated_memory_mb
        );
    }

    #[test]
    fn hmgi_per_query_work_is_max_partition_not_sum() {
        // HMGI fans out in parallel, so wall-clock work tracks
        // the slowest partition × parallelism overhead, NOT the
        // sum across partitions. Verify the advisor reflects this.
        let advisor = HmgiIndexAdvisor::new();
        let hmgi_out = advisor
            .advise(&hmgi_input(0.95, vec!["text", "image"]))
            .unwrap();

        // Compare against a standalone HNSW for the per-partition N.
        // The HMGI per-query-work should be roughly equal to the
        // single-partition HNSW work (parallel fan-out), not 2x.
        let hnsw_advisor = HnswIndexAdvisor::new();
        let hnsw_out = hnsw_advisor
            .advise(&AnnAdvisorInput {
                vector_count: 50_000, // per-partition N
                top_k: 10,
                recall_target: 0.95,
                dimension: 128,
                distance_metric: DistanceMetric::Cosine,
                max_query_latency_ms: None,
                max_memory_mb: None,
                binary_rerank_allowed: false,
                modalities: vec![],
            })
            .unwrap();

        // HMGI's per-query work should be ~= single-partition HNSW work.
        // Allow a small margin since the modality count factors in.
        let ratio =
            (hmgi_out.estimated_per_query_work as f64) / (hnsw_out.estimated_per_query_work as f64);
        assert!(
            (0.9..=1.1).contains(&ratio),
            "HMGI work {} should be ~= single-partition HNSW work {} (ratio {:.2})",
            hmgi_out.estimated_per_query_work,
            hnsw_out.estimated_per_query_work,
            ratio
        );
    }

    #[test]
    fn recall_for_returns_worst_per_partition() {
        // Build an HMGI spec with two partitions of different m;
        // the joint recall should be the worse (lower-m) one.
        let advisor = HmgiIndexAdvisor::new();
        let spec = IndexAlgorithm::HMGI {
            per_modality: vec![
                HmgiPartitionAlgo {
                    modality_tag: "high_recall".into(),
                    m: 32,
                    ef_construction: 256,
                    ef_search: 400,
                },
                HmgiPartitionAlgo {
                    modality_tag: "low_recall".into(),
                    m: 16,
                    ef_construction: 128,
                    ef_search: 100,
                },
            ],
            max_partitions_per_query: 2,
        };
        let recall = advisor.recall_for(&spec, 100_000, 10).unwrap();
        // m=16 partition caps at 0.80; the joint is bounded by it.
        assert!(
            recall <= 0.80 + 0.001,
            "joint recall {} should be bounded by m=16 partition ceiling 0.80",
            recall
        );
    }

    #[test]
    fn recall_for_returns_none_on_wrong_variant() {
        let advisor = HmgiIndexAdvisor::new();
        let hnsw_spec = IndexAlgorithm::HNSW {
            m: 32,
            ef_construction: 256,
            ef_search: 400,
            max_elements: 100_000,
        };
        assert!(advisor.recall_for(&hnsw_spec, 100_000, 10).is_none());
    }
}
