// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! Recall-target plumbing for collection creation.
//!
//! # Why
//!
//! Operators want one knob ("I need recall ≥ 0.95") instead of three
//! ("set m=32, ef_construction=256, ef_search=409"). The HNSW
//! parameter advisor at
//! `crate::index::axis::management::hnsw_param_advisor` turns that
//! single knob into the three concrete parameters. This module is
//! the **glue** between a `CollectionConfig` arriving on the wire
//! and the advisor.
//!
//! # Schema location (interim)
//!
//! `recall_target` is stored as a tag on `CollectionConfig.tags`
//! with the convention `"recall_target:0.95"` (any valid `f32` in
//! `[0.0, 1.0]`). It's a tag rather than a typed proto field because
//! the proto regeneration pipeline is currently manual — see the
//! comment block on field 26 in
//! `proto/proximadb/v1/collection_types.proto`. Promoting to a
//! typed field is a clean follow-up once regen is auto-wired.
//!
//! # What the wiring does
//!
//! For each `IndexConfig` in the collection whose `algorithm =
//! HNSW`:
//!
//! 1. If `hnsw_config` is `None` OR every numeric field
//!    (`m`, `ef_construction`, `ef_search`) is also `None`, the
//!    advisor's recommendation populates the full block.
//! 2. If the caller pinned ANY HNSW field, the wiring is a no-op —
//!    explicit caller intent always wins, the advisor never
//!    overwrites.
//!
//! Both cases emit a `tracing::info!` on `collection.recall_target`
//! so operators can see in logs / dashboards which params landed
//! and why.

use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::management::{HnswSizingInput, HnswSizingOutput, advise_hnsw_params};
use crate::proto::proximadb_v1::{
    CollectionConfig, DistanceMetric as ProtoDistanceMetric, HnswConfig, IndexConfig,
    IndexingAlgorithm,
};

/// Tag key prefix used to encode `recall_target` on
/// `CollectionConfig.tags`.
pub const RECALL_TARGET_TAG_PREFIX: &str = "recall_target:";

/// Hard clamp range for the parsed recall target. Values outside
/// this band are silently clamped — well below 0.50 is meaningless
/// (worse than random for k=10), above 0.999 the advisor's table
/// flattens and won't add more value.
pub const RECALL_TARGET_MIN: f32 = 0.50;
pub const RECALL_TARGET_MAX: f32 = 0.999;

/// Read `recall_target` from `config.tags`, returning `Some(value)`
/// only if the convention tag is present and parses as a clamped
/// `f32`. Multiple `recall_target:` tags → the last one wins (lets
/// downstream layers override an earlier-applied default).
pub fn parse_recall_target(config: &CollectionConfig) -> Option<f32> {
    let mut latest: Option<f32> = None;
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(RECALL_TARGET_TAG_PREFIX)
            && let Ok(v) = rest.trim().parse::<f32>()
            && v.is_finite()
        {
            latest = Some(v.clamp(RECALL_TARGET_MIN, RECALL_TARGET_MAX));
        }
    }
    latest
}

/// Walk `config.index_configs` and, for every HNSW entry that hasn't
/// been pinned by the caller, stamp advisor-recommended `m`,
/// `ef_construction`, and `ef_search`. Returns a list of
/// `(index_name, advisor_output)` for every index the advisor wrote
/// to — empty if nothing changed (no HNSW indexes, all pinned, or
/// no recall_target set).
///
/// Mutates `config.index_configs[*].hnsw_config` in place.
pub fn apply_advisor_to_hnsw_indexes(
    config: &mut CollectionConfig,
    recall_target: f32,
) -> Vec<(String, HnswSizingOutput)> {
    let n = config.dimension as u64; // placeholder until we have steady-state estimate
    let metric = convert_distance_metric(config.distance_metric);
    let dimension = config.dimension;

    // Use the **declared collection size** if the caller hinted it
    // via a `target_vector_count:` tag; otherwise default to a
    // mid-scale 100K which is the calibration anchor. This gives a
    // reasonable cold-start estimate; the AdaptiveIndexEngine retunes
    // as the real corpus grows past tier boundaries.
    let target_n = parse_target_vector_count(config).unwrap_or(100_000);

    let mut applied: Vec<(String, HnswSizingOutput)> = Vec::new();

    for idx in config.index_configs.iter_mut() {
        if idx.algorithm != IndexingAlgorithm::Hnsw as i32 {
            continue;
        }

        // Respect caller-pinned HNSW params — if any of m / efc /
        // ef_search is set, do nothing.
        let already_pinned = idx.hnsw_config.as_ref().is_some_and(|h| {
            h.m.is_some() || h.ef_construction.is_some() || h.ef_search.is_some()
        });
        if already_pinned {
            continue;
        }

        let out = advise_hnsw_params(HnswSizingInput {
            vector_count: target_n,
            top_k: 10, // common default; advisor floor still applies if caller asks for higher k
            recall_target,
            dimension,
            distance_metric: metric,
        });

        idx.hnsw_config = Some(HnswConfig {
            m: Some(out.m),
            ef_construction: Some(out.ef_construction),
            ef_search: Some(out.ef_search),
            ..idx.hnsw_config.clone().unwrap_or_default()
        });

        applied.push((idx.index_name.clone(), out));
    }

    let _unused = n; // silence — we used target_n instead

    applied
}

/// Optional steady-state size hint via `target_vector_count:` tag.
/// Operators use it to ask "size for the corpus I expect to have"
/// instead of the calibration default. Values silently clamp to
/// `[1_000, 1_000_000_000]`.
pub fn parse_target_vector_count(config: &CollectionConfig) -> Option<u64> {
    const TAG: &str = "target_vector_count:";
    let mut latest: Option<u64> = None;
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG)
            && let Ok(v) = rest.trim().parse::<u64>()
        {
            latest = Some(v.clamp(1_000, 1_000_000_000));
        }
    }
    latest
}

fn convert_distance_metric(raw: Option<i32>) -> DistanceMetric {
    match raw.and_then(|v| ProtoDistanceMetric::try_from(v).ok()) {
        Some(ProtoDistanceMetric::Cosine) => DistanceMetric::Cosine,
        Some(ProtoDistanceMetric::Euclidean) => DistanceMetric::Euclidean,
        Some(ProtoDistanceMetric::DotProduct) => DistanceMetric::DotProduct,
        _ => DistanceMetric::Cosine,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str, dim: u32, tags: &[&str]) -> CollectionConfig {
        CollectionConfig {
            name: name.to_string(),
            dimension: dim,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn hnsw_idx(name: &str) -> IndexConfig {
        IndexConfig {
            index_name: name.to_string(),
            algorithm: IndexingAlgorithm::Hnsw as i32,
            ..Default::default()
        }
    }

    #[test]
    fn parse_recall_target_happy_path() {
        let c = cfg("c", 128, &["recall_target:0.95"]);
        assert_eq!(parse_recall_target(&c), Some(0.95));
    }

    #[test]
    fn parse_recall_target_missing_returns_none() {
        let c = cfg("c", 128, &["something_else:xyz"]);
        assert_eq!(parse_recall_target(&c), None);
    }

    #[test]
    fn parse_recall_target_clamps_out_of_range() {
        assert_eq!(
            parse_recall_target(&cfg("c", 128, &["recall_target:1.50"])),
            Some(RECALL_TARGET_MAX)
        );
        assert_eq!(
            parse_recall_target(&cfg("c", 128, &["recall_target:0.10"])),
            Some(RECALL_TARGET_MIN)
        );
    }

    #[test]
    fn parse_recall_target_last_one_wins() {
        let c = cfg(
            "c",
            128,
            &["recall_target:0.85", "recall_target:0.95"],
        );
        assert_eq!(parse_recall_target(&c), Some(0.95));
    }

    #[test]
    fn parse_recall_target_ignores_garbage() {
        let c = cfg("c", 128, &["recall_target:not_a_number"]);
        assert_eq!(parse_recall_target(&c), None);
    }

    #[test]
    fn apply_advisor_stamps_unset_hnsw_config() {
        let mut c = cfg("c", 128, &["recall_target:0.95"]);
        c.index_configs.push(hnsw_idx("primary"));
        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        assert_eq!(applied.len(), 1);
        let h = c.index_configs[0].hnsw_config.as_ref().unwrap();
        assert!(h.m.unwrap() > 0);
        assert!(h.ef_construction.unwrap() > 0);
        assert!(h.ef_search.unwrap() > 0);
    }

    #[test]
    fn apply_advisor_respects_caller_pinned_m() {
        let mut c = cfg("c", 128, &["recall_target:0.95"]);
        let mut idx = hnsw_idx("primary");
        idx.hnsw_config = Some(HnswConfig {
            m: Some(64),
            ..Default::default()
        });
        c.index_configs.push(idx);

        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        assert_eq!(applied.len(), 0, "caller-pinned config should be untouched");
        let h = c.index_configs[0].hnsw_config.as_ref().unwrap();
        assert_eq!(h.m, Some(64));
        assert!(h.ef_search.is_none()); // wasn't pinned, but we don't touch the block
    }

    #[test]
    fn apply_advisor_skips_non_hnsw_indexes() {
        let mut c = cfg("c", 128, &["recall_target:0.95"]);
        let mut ivf_idx = IndexConfig {
            index_name: "ivf".to_string(),
            algorithm: IndexingAlgorithm::Ivf as i32,
            ..Default::default()
        };
        ivf_idx.hnsw_config = None;
        c.index_configs.push(ivf_idx);

        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        assert_eq!(applied.len(), 0);
        assert!(c.index_configs[0].hnsw_config.is_none());
    }

    #[test]
    fn target_vector_count_drives_advisor_n() {
        let mut c = cfg("c", 128, &["target_vector_count:1000000"]);
        c.index_configs.push(hnsw_idx("p"));
        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        let ef_at_1m = applied[0].1.ef_search;

        let mut c2 = cfg("c2", 128, &["target_vector_count:10000"]);
        c2.index_configs.push(hnsw_idx("p"));
        let applied2 = apply_advisor_to_hnsw_indexes(&mut c2, 0.95);
        let ef_at_10k = applied2[0].1.ef_search;

        assert!(
            ef_at_1m > ef_at_10k,
            "ef must scale up with target N: ef@1M={}, ef@10K={}",
            ef_at_1m,
            ef_at_10k
        );
    }

    #[test]
    fn parse_target_vector_count_clamps() {
        assert_eq!(
            parse_target_vector_count(&cfg("c", 128, &["target_vector_count:50"])),
            Some(1_000)
        );
        assert_eq!(
            parse_target_vector_count(&cfg(
                "c",
                128,
                &["target_vector_count:50000000000"]
            )),
            Some(1_000_000_000)
        );
    }
}
