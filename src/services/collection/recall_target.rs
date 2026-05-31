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
#[cfg(test)]
use crate::proto::proximadb_v1::IndexConfig;
use crate::proto::proximadb_v1::{
    CollectionConfig, DistanceMetric as ProtoDistanceMetric, HnswConfig, IndexingAlgorithm,
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
/// **Auto-add behavior**: if `config.index_configs` contains *zero*
/// HNSW entries and there's at least one filterable column or no
/// indexes at all, the function appends a fresh
/// `IndexConfig { algorithm: HNSW, … }` with name
/// `[`AUTO_HNSW_INDEX_NAME`]` so the caller's `recall_target:` tag
/// is honored end-to-end (otherwise the advisor would have nothing
/// to size and the operator would silently get the legacy default
/// HNSW parameters).
///
/// Mutates `config.index_configs` in place.
pub fn apply_advisor_to_hnsw_indexes(
    config: &mut CollectionConfig,
    recall_target: f32,
) -> Vec<(String, HnswSizingOutput)> {
    let metric = convert_distance_metric(config.distance_metric);
    let dimension = config.dimension;

    // Use the **declared collection size** if the caller hinted it
    // via a `target_vector_count:` tag; otherwise default to a
    // mid-scale 100K which is the calibration anchor. This gives a
    // reasonable cold-start estimate; the AdaptiveIndexEngine retunes
    // as the real corpus grows past tier boundaries.
    let target_n = parse_target_vector_count(config).unwrap_or(100_000);
    let top_k = resolve_top_k(config);

    // Auto-add a stub HNSW IndexConfig when the caller asked for a
    // recall_target but didn't attach an HNSW index. The advisor
    // loop below then fills in m / efc / ef_search on this stub.
    // Skips when the caller already attached a non-HNSW index they
    // want to use instead (IVF, etc.) — that's an explicit choice.
    let has_any_hnsw = config
        .index_configs
        .iter()
        .any(|idx| idx.algorithm == IndexingAlgorithm::Hnsw as i32);
    if !has_any_hnsw && config.index_configs.is_empty() && dimension > 0 {
        config
            .index_configs
            .push(crate::proto::proximadb_v1::IndexConfig {
                index_name: AUTO_HNSW_INDEX_NAME.to_string(),
                algorithm: IndexingAlgorithm::Hnsw as i32,
                ..Default::default()
            });
    }

    let mut applied: Vec<(String, HnswSizingOutput)> = Vec::new();

    for idx in config.index_configs.iter_mut() {
        if idx.algorithm != IndexingAlgorithm::Hnsw as i32 {
            continue;
        }

        // Respect caller-pinned HNSW params — if any of m / efc /
        // ef_search is set, do nothing.
        let already_pinned = idx
            .hnsw_config
            .as_ref()
            .is_some_and(|h| h.m.is_some() || h.ef_construction.is_some() || h.ef_search.is_some());
        if already_pinned {
            continue;
        }

        let out = advise_hnsw_params(HnswSizingInput {
            vector_count: target_n,
            top_k,
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

    applied
}

/// Index name used when `apply_advisor_to_hnsw_indexes` synthesizes
/// a stub HNSW IndexConfig from a bare `recall_target:` tag.
/// Exposed so operator tooling can detect "this index was auto-
/// created from a tag, not explicitly requested by the caller".
pub const AUTO_HNSW_INDEX_NAME: &str = "auto_hnsw_recall_target";

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

/// Default `top_k` the advisor + drift detector use when the
/// collection doesn't specify a `target_top_k:` tag. 10 matches the
/// historical hard-coded value across the route-health, recall-tune,
/// recluster, and sweeper surfaces — kept as the named constant so a
/// future change updates every site in lockstep.
pub const DEFAULT_TOP_K: u32 = 10;

/// Optional advisor `top_k` hint via `target_top_k:` tag. Workloads
/// that consistently request `top_k > 10` need a higher `ef_search`
/// floor to maintain recall — the advisor scales `ef ∝ k`, so the
/// recommendation goes from "fine at k=10" to "noticeably low at
/// k=100". Letting the operator declare the steady-state k as a tag
/// fixes the sizing without per-query plumbing.
///
/// Clamps to `[1, 1000]` — values outside that band are either
/// degenerate (`k=0`) or beyond the advisor's calibration envelope.
pub fn parse_target_top_k(config: &CollectionConfig) -> Option<u32> {
    const TAG: &str = "target_top_k:";
    let mut latest: Option<u32> = None;
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG)
            && let Ok(v) = rest.trim().parse::<u32>()
        {
            latest = Some(v.clamp(1, 1000));
        }
    }
    latest
}

/// Resolve the advisor `top_k` for a collection: tag if present,
/// otherwise the `DEFAULT_TOP_K` constant. Every consumer of the
/// advisor (route-health, recall-tune, recluster, sweeper, create-
/// time wiring) routes through this helper so the resolution rule
/// stays in one place.
pub fn resolve_top_k(config: &CollectionConfig) -> u32 {
    parse_target_top_k(config).unwrap_or(DEFAULT_TOP_K)
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
        let c = cfg("c", 128, &["recall_target:0.85", "recall_target:0.95"]);
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
    fn apply_advisor_auto_adds_hnsw_when_none_present() {
        // A recall_target tag with no index_configs at all should
        // get an auto-synthesized HNSW IndexConfig sized by the
        // advisor. Otherwise the recall_target would be a silent
        // no-op.
        let mut c = cfg("c_auto", 128, &["recall_target:0.95"]);
        assert!(c.index_configs.is_empty());

        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        assert_eq!(applied.len(), 1, "advisor must stamp the auto-added index");
        assert_eq!(c.index_configs.len(), 1);
        assert_eq!(c.index_configs[0].index_name, AUTO_HNSW_INDEX_NAME);
        assert_eq!(c.index_configs[0].algorithm, IndexingAlgorithm::Hnsw as i32);

        let h = c.index_configs[0].hnsw_config.as_ref().unwrap();
        assert!(h.m.is_some());
        assert!(h.ef_construction.is_some());
        assert!(h.ef_search.is_some());
    }

    #[test]
    fn apply_advisor_does_not_auto_add_when_caller_provided_ivf() {
        // The caller chose IVF explicitly — don't second-guess by
        // also adding an HNSW. (recall_target on an IVF-only
        // collection is a no-op today; that's the caller's
        // signal that they accept it.)
        let mut c = cfg("c_ivf", 128, &["recall_target:0.95"]);
        c.index_configs
            .push(crate::proto::proximadb_v1::IndexConfig {
                index_name: "explicit_ivf".to_string(),
                algorithm: IndexingAlgorithm::Ivf as i32,
                ..Default::default()
            });

        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        assert_eq!(applied.len(), 0, "no HNSW to stamp");
        assert_eq!(c.index_configs.len(), 1, "no auto-HNSW added");
        assert_eq!(c.index_configs[0].algorithm, IndexingAlgorithm::Ivf as i32);
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
    fn parse_target_top_k_happy_path() {
        let c = cfg("c", 128, &["target_top_k:100"]);
        assert_eq!(parse_target_top_k(&c), Some(100));
    }

    #[test]
    fn parse_target_top_k_missing_returns_none() {
        let c = cfg("c", 128, &["recall_target:0.95"]);
        assert_eq!(parse_target_top_k(&c), None);
    }

    #[test]
    fn parse_target_top_k_clamps_extremes() {
        // 0 → clamps to floor (1)
        assert_eq!(
            parse_target_top_k(&cfg("c", 128, &["target_top_k:0"])),
            Some(1)
        );
        // 100K → clamps to ceiling (1000)
        assert_eq!(
            parse_target_top_k(&cfg("c", 128, &["target_top_k:100000"])),
            Some(1000)
        );
    }

    #[test]
    fn resolve_top_k_uses_tag_when_present() {
        let c = cfg("c", 128, &["target_top_k:50"]);
        assert_eq!(resolve_top_k(&c), 50);
    }

    #[test]
    fn resolve_top_k_falls_back_to_default() {
        let c = cfg("c", 128, &["recall_target:0.95"]);
        assert_eq!(resolve_top_k(&c), DEFAULT_TOP_K);
        assert_eq!(DEFAULT_TOP_K, 10, "DEFAULT_TOP_K pinned at 10");
    }

    #[test]
    fn target_top_k_changes_advised_ef() {
        // Pinning the same N + recall_target, a larger k should
        // push the advisor toward a larger ef_search recommendation
        // (advisor: ef ∝ k).
        let mut c_small = cfg("small", 128, &["recall_target:0.95", "target_top_k:10"]);
        c_small.index_configs.push(hnsw_idx("p"));
        let small = apply_advisor_to_hnsw_indexes(&mut c_small, 0.95);

        let mut c_large = cfg("large", 128, &["recall_target:0.95", "target_top_k:100"]);
        c_large.index_configs.push(hnsw_idx("p"));
        let large = apply_advisor_to_hnsw_indexes(&mut c_large, 0.95);

        let ef_small = small[0].1.ef_search;
        let ef_large = large[0].1.ef_search;
        assert!(
            ef_large > ef_small,
            "target_top_k=100 must drive a larger ef than k=10 (small={}, large={})",
            ef_small,
            ef_large
        );
    }

    #[test]
    fn parse_target_vector_count_clamps() {
        assert_eq!(
            parse_target_vector_count(&cfg("c", 128, &["target_vector_count:50"])),
            Some(1_000)
        );
        assert_eq!(
            parse_target_vector_count(&cfg("c", 128, &["target_vector_count:50000000000"])),
            Some(1_000_000_000)
        );
    }
}
