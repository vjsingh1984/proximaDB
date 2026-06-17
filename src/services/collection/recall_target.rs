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
use crate::index::axis::management::{
    AnnIndexAdvisor, HnswSizingInput, HnswSizingOutput, advise_hnsw_params,
};
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
    let max_ef_search = parse_max_ef_search(config);

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
            max_ef_search,
        });

        idx.hnsw_config = Some(HnswConfig {
            m: Some(out.m),
            ef_construction: Some(out.ef_construction),
            ef_search: Some(out.ef_search),
            ..idx.hnsw_config.unwrap_or_default()
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

/// Index name used when the [`apply_advisor_to_indexes`] selector
/// picks IVF and synthesizes a stub IndexConfig from a bare
/// `recall_target:` tag.
pub const AUTO_IVF_INDEX_NAME: &str = "auto_ivf_recall_target";

/// Index name used when the [`apply_advisor_to_indexes`] selector
/// picks HMGI (≥ 2 modalities) and synthesizes a per-partition
/// HNSW IndexConfig as the wire-level representation. HMGI has no
/// proto `IndexingAlgorithm` variant — the `modalities:` tag at
/// the collection level marks the collection as multi-modal at
/// the route-health surface.
pub const AUTO_HMGI_INDEX_NAME: &str = "auto_hmgi_recall_target";

/// Result of an algorithm-agnostic advisor pass over a
/// `CollectionConfig`. One entry per HNSW / IVF / … index the
/// advisor sized, carrying the discriminator + full
/// [`AnnAdvisorOutput`] (which itself carries the sized
/// `IndexAlgorithm` + memory/work estimates + rationale).
#[derive(Debug, Clone)]
pub struct AppliedAdvice {
    /// The IndexConfig name that received the sized params.
    /// `AUTO_HNSW_INDEX_NAME` / `AUTO_IVF_INDEX_NAME` when the
    /// IndexConfig was synthesized from the tag set.
    pub index_name: String,
    /// Full advisor output — algorithm-tagged with kind, carries
    /// memory + work estimates + projected recall when clamped.
    pub output: crate::index::axis::management::AnnAdvisorOutput,
}

/// Algorithm-agnostic advisor entry point. Walks the config's
/// `index_configs`:
///
/// * **HNSW** entries with unpinned params: size via the HNSW
///   per-algo path (preserves the existing
///   `apply_advisor_to_hnsw_indexes` behavior).
/// * **IVF** entries with unpinned params: size via the IVF
///   per-algo path (new in P1).
/// * **No index_configs** + `dimension > 0`: invoke the
///   [`AnnSelector`] to pick HNSW vs IVF based on the declared
///   budgets, then synthesize a stub IndexConfig (named
///   `AUTO_HNSW_INDEX_NAME` or `AUTO_IVF_INDEX_NAME`) and stamp
///   the sized params.
/// * **Caller-pinned** indexes (any of `m` / `efc` / `ef_search` /
///   `nlist` / `nprobe` set): skip; explicit caller intent always
///   wins.
///
/// The legacy [`apply_advisor_to_hnsw_indexes`] is still public
/// (used by tests + the existing manager call site) — it now
/// delegates to this function and filters the result to HNSW
/// entries for back-compat with callers that expected
/// `Vec<(String, HnswSizingOutput)>`.
pub fn apply_advisor_to_indexes(
    config: &mut CollectionConfig,
    recall_target: f32,
) -> Vec<AppliedAdvice> {
    use crate::index::axis::management::{AnnAdvisorInput, AnnSelector, SupportedAlgorithm};
    use crate::index::axis::types::IndexAlgorithm;

    let metric = convert_distance_metric(config.distance_metric);
    let dimension = config.dimension;
    let target_n = parse_target_vector_count(config).unwrap_or(100_000);
    let top_k = resolve_top_k(config);
    let max_ef_search_legacy = parse_max_ef_search(config);
    let max_query_latency_ms = parse_max_query_latency_ms(config);
    let max_memory_mb = parse_max_memory_mb(config);
    let binary_rerank_allowed = parse_binary_rerank_allowed(config);
    let modalities = parse_modalities(config);

    // Auto-add a stub IndexConfig when no indexes exist. Use the
    // selector to decide HNSW vs IVF.
    if config.index_configs.is_empty() && dimension > 0 {
        let selector = AnnSelector::default_set();
        let input = AnnAdvisorInput {
            vector_count: target_n,
            top_k,
            recall_target,
            dimension,
            distance_metric: metric,
            max_query_latency_ms,
            max_memory_mb,
            binary_rerank_allowed,
            modalities: modalities.clone(),
        };
        if let Some(picked) = selector.select_and_advise(&input) {
            match &picked.algorithm {
                IndexAlgorithm::HNSW {
                    m,
                    ef_construction,
                    ef_search,
                    ..
                } => {
                    let m = *m;
                    let ef_construction = *ef_construction;
                    let ef_search = *ef_search;
                    config
                        .index_configs
                        .push(crate::proto::proximadb_v1::IndexConfig {
                            index_name: AUTO_HNSW_INDEX_NAME.to_string(),
                            algorithm: IndexingAlgorithm::Hnsw as i32,
                            hnsw_config: Some(HnswConfig {
                                m: Some(m),
                                ef_construction: Some(ef_construction),
                                ef_search: Some(ef_search),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                    return vec![AppliedAdvice {
                        index_name: AUTO_HNSW_INDEX_NAME.to_string(),
                        output: picked,
                    }];
                }
                IndexAlgorithm::IVF { nlist, nprobe, .. } => {
                    let nlist = *nlist;
                    let nprobe = *nprobe;
                    config
                        .index_configs
                        .push(crate::proto::proximadb_v1::IndexConfig {
                            index_name: AUTO_IVF_INDEX_NAME.to_string(),
                            algorithm: IndexingAlgorithm::Ivf as i32,
                            ivf_config: Some(crate::proto::proximadb_v1::IvfConfig {
                                n_lists: Some(nlist),
                                n_probe: Some(nprobe),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                    return vec![AppliedAdvice {
                        index_name: AUTO_IVF_INDEX_NAME.to_string(),
                        output: picked,
                    }];
                }
                IndexAlgorithm::HMGI { per_modality, .. } => {
                    // P3: HMGI has no wire-IndexingAlgorithm proto
                    // variant. Synthesize the canonical per-partition
                    // HNSW config — HMGI's advisor sizes every
                    // partition with the same HnswIndexAdvisor call,
                    // so the first partition's `(m, ef_construction,
                    // ef_search)` tuple is identical to every other
                    // partition's. The collection's `modalities:` tag
                    // marks it as HMGI at the route-health surface;
                    // the runtime HMGI router still partitions by
                    // modality_tag at query time.
                    if let Some(first) = per_modality.first() {
                        let m = first.m;
                        let ef_construction = first.ef_construction;
                        let ef_search = first.ef_search;
                        config
                            .index_configs
                            .push(crate::proto::proximadb_v1::IndexConfig {
                                index_name: AUTO_HMGI_INDEX_NAME.to_string(),
                                algorithm: IndexingAlgorithm::Hnsw as i32,
                                hnsw_config: Some(HnswConfig {
                                    m: Some(m),
                                    ef_construction: Some(ef_construction),
                                    ef_search: Some(ef_search),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });
                        return vec![AppliedAdvice {
                            index_name: AUTO_HMGI_INDEX_NAME.to_string(),
                            output: picked,
                        }];
                    }
                    // Empty per_modality (shouldn't happen — HMGI
                    // advisor declines instead) → no-op fallthrough.
                }
                _ => {
                    // Selector returned an algorithm the auto-add
                    // path doesn't synthesize for (PQ/Annoy in P2/P3).
                    // No-op for now.
                }
            }
        }
        // Selector declined every advisor (e.g. all clamped + no
        // best-effort). Fall through with no auto-add.
    }

    // Walk pre-existing index_configs and size HNSW / IVF entries
    // per their algorithm-specific advisor.
    let mut applied: Vec<AppliedAdvice> = Vec::new();
    for idx in config.index_configs.iter_mut() {
        let algo =
            IndexingAlgorithm::try_from(idx.algorithm).unwrap_or(IndexingAlgorithm::Unspecified);
        match algo {
            IndexingAlgorithm::Hnsw => {
                // Skip caller-pinned HNSW.
                let already_pinned = idx.hnsw_config.as_ref().is_some_and(|h| {
                    h.m.is_some() || h.ef_construction.is_some() || h.ef_search.is_some()
                });
                if already_pinned {
                    continue;
                }
                let advisor = crate::index::axis::management::HnswIndexAdvisor::new();
                if let Some(out) = advisor.advise(&AnnAdvisorInput {
                    vector_count: target_n,
                    top_k,
                    recall_target,
                    dimension,
                    distance_metric: metric,
                    // Honor either the legacy max_ef_search tag OR
                    // the new max_query_latency_ms — HNSW advisor
                    // accepts either. Tag-level precedence: explicit
                    // max_ef_search wins (operator already knows the
                    // HNSW knob); else fall back to the latency
                    // budget translated via the advisor's cost model.
                    max_query_latency_ms: if max_ef_search_legacy.is_some() {
                        None
                    } else {
                        max_query_latency_ms
                    },
                    max_memory_mb,
                    binary_rerank_allowed,
                    modalities: modalities.clone(),
                }) && let IndexAlgorithm::HNSW {
                    m,
                    ef_construction,
                    ef_search,
                    ..
                } = out.algorithm
                {
                    idx.hnsw_config = Some(HnswConfig {
                        m: Some(m),
                        ef_construction: Some(ef_construction),
                        // If the operator pinned max_ef_search
                        // (legacy tag), clamp here too — the
                        // HnswIndexAdvisor::advise call above
                        // skipped max_query_latency_ms for that
                        // reason, so we re-apply.
                        ef_search: Some(
                            max_ef_search_legacy
                                .map(|cap| ef_search.min(cap))
                                .unwrap_or(ef_search),
                        ),
                        ..idx.hnsw_config.unwrap_or_default()
                    });
                    applied.push(AppliedAdvice {
                        index_name: idx.index_name.clone(),
                        output: out,
                    });
                }
            }
            IndexingAlgorithm::Ivf => {
                // Skip caller-pinned IVF.
                let already_pinned = idx
                    .ivf_config
                    .as_ref()
                    .is_some_and(|i| i.n_lists.is_some() || i.n_probe.is_some());
                if already_pinned {
                    continue;
                }
                let advisor = crate::index::axis::management::IvfIndexAdvisor::new();
                if let Some(out) = advisor.advise(&AnnAdvisorInput {
                    vector_count: target_n,
                    top_k,
                    recall_target,
                    dimension,
                    distance_metric: metric,
                    max_query_latency_ms,
                    max_memory_mb,
                    binary_rerank_allowed,
                    modalities: modalities.clone(),
                }) && let IndexAlgorithm::IVF { nlist, nprobe, .. } = out.algorithm
                {
                    idx.ivf_config = Some(crate::proto::proximadb_v1::IvfConfig {
                        n_lists: Some(nlist),
                        n_probe: Some(nprobe),
                        ..idx.ivf_config.unwrap_or_default()
                    });
                    applied.push(AppliedAdvice {
                        index_name: idx.index_name.clone(),
                        output: out,
                    });
                }
            }
            _ => continue, // PQ/Annoy/LSH/etc — skip in P1.
        }
    }

    // Silence unused — the legacy `Hnsw` discriminator is referenced
    // above; this asserts `SupportedAlgorithm::Hnsw` is reachable so
    // the trait re-export survives `cargo fix` cleanups.
    let _ = SupportedAlgorithm::Hnsw;
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

/// Optional latency-budget cap on `ef_search` via `max_ef_search:`
/// tag. When set, the advisor never recommends an ef above this
/// value — it clamps to the cap and surfaces the resulting
/// (possibly lower) recall on route-health as
/// `projected_recall_at_clamped_ef`. Lets operators trade off recall
/// vs query latency without manual ef tuning.
///
/// Clamps to `[EF_SEARCH_MIN, EF_SEARCH_MAX]` = `[16, 2048]` — same
/// bounds the advisor enforces internally on the unclamped ef.
///
/// **HNSW-specific**. For the **algorithm-agnostic** latency budget
/// (used by the [`AnnSelector`] across HNSW + IVF), prefer
/// [`parse_max_query_latency_ms`] — the selector translates it to
/// `max_ef_search` for HNSW and `max_nprobe` for IVF via per-algo
/// cost models.
pub fn parse_max_ef_search(config: &CollectionConfig) -> Option<u32> {
    use crate::index::axis::management::{EF_SEARCH_MAX, EF_SEARCH_MIN};
    const TAG: &str = "max_ef_search:";
    let mut latest: Option<u32> = None;
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG)
            && let Ok(v) = rest.trim().parse::<u32>()
        {
            latest = Some(v.clamp(EF_SEARCH_MIN, EF_SEARCH_MAX));
        }
    }
    latest
}

/// Optional per-query latency budget in milliseconds via
/// `max_query_latency_ms:` tag. Algorithm-agnostic: the
/// [`AnnSelector`] hands this to each advisor, which maps it to
/// algorithm-specific budgets (HNSW `max_ef_search`, IVF
/// `max_nprobe`) via internal cost models. Lets operators declare
/// "I'd rather miss recall than blow my latency SLO" without
/// knowing the indexing internals.
///
/// Parses an `f64` from the tag value. Clamps to `[0.1, 10000.0]`
/// — below 100μs the per-algo cost models are noise-dominated;
/// above 10s the budget is effectively unbounded.
pub fn parse_max_query_latency_ms(config: &CollectionConfig) -> Option<f64> {
    const TAG: &str = "max_query_latency_ms:";
    let mut latest: Option<f64> = None;
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG)
            && let Ok(v) = rest.trim().parse::<f64>()
            && v.is_finite()
        {
            latest = Some(v.clamp(0.1, 10_000.0));
        }
    }
    latest
}

/// Optional memory budget in MB via `max_memory_mb:` tag.
/// Algorithm-agnostic: the selector compares each candidate
/// algorithm's `estimated_memory_mb` against this cap and excludes
/// those that exceed it. Drives the HNSW↔IVF (and later HNSW↔IVF+PQ)
/// selection when memory is the binding constraint.
///
/// Parses an `f64` MB value. Clamps to `[1.0, 1_048_576.0]` (1 MB to
/// 1 TB) — anything outside is degenerate or unbounded.
pub fn parse_max_memory_mb(config: &CollectionConfig) -> Option<f64> {
    const TAG: &str = "max_memory_mb:";
    let mut latest: Option<f64> = None;
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG)
            && let Ok(v) = rest.trim().parse::<f64>()
            && v.is_finite()
        {
            latest = Some(v.clamp(1.0, 1_048_576.0));
        }
    }
    latest
}

/// Modality tags declared by the operator via the `modalities:`
/// tag. Convention: comma-separated lowercase identifiers, e.g.
/// `modalities:text,image,video`. Activates the
/// [`crate::index::axis::management::HmgiIndexAdvisor`] when ≥ 2
/// modalities are present.
///
/// Returns an empty `Vec` for single-modality collections (the
/// default). Each modality_tag is trimmed and lower-cased so
/// "Text" / "TEXT" / "text" all normalise to the same partition
/// key — matches HMGI router behavior.
///
/// Multiple `modalities:` tags on the same collection (rare but
/// possible) are unioned; duplicates are removed preserving the
/// first occurrence order.
pub fn parse_modalities(config: &CollectionConfig) -> Vec<String> {
    const TAG: &str = "modalities:";
    let mut out: Vec<String> = Vec::new();
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG) {
            for raw in rest.split(',') {
                let normalised = raw.trim().to_ascii_lowercase();
                if !normalised.is_empty() && !out.contains(&normalised) {
                    out.push(normalised);
                }
            }
        }
    }
    out
}

/// Operator opt-in for IVF binary / PQ rerank via the
/// `binary_rerank:enabled` tag. When present, the IVF advisor
/// lifts its recall ceiling from ~0.74 to ~0.95 and emits an
/// `IndexAlgorithm::IVF` with `quantizer: Some(Box<PQ {...}>)`.
///
/// Defaults to `false` for backward compatibility — legacy IVF
/// collections without this tag stay on the single-stage path
/// with the 0.74 ceiling. The matching `BINARY_TIER_ENV`
/// process-level env knob still works for global rollouts.
///
/// Recognised values: `enabled`, `on`, `true`, `1`,
/// `yes` (case-insensitive). Anything else parses as disabled.
pub fn parse_binary_rerank_allowed(config: &CollectionConfig) -> bool {
    const TAG: &str = "binary_rerank:";
    for tag in &config.tags {
        if let Some(rest) = tag.strip_prefix(TAG) {
            let val = rest.trim().to_ascii_lowercase();
            return matches!(val.as_str(), "enabled" | "on" | "true" | "1" | "yes");
        }
    }
    false
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

    // ───── max_ef_search latency budget ────────────────────────

    #[test]
    fn parse_max_ef_search_happy_path() {
        let c = cfg("c", 128, &["max_ef_search:300"]);
        assert_eq!(parse_max_ef_search(&c), Some(300));
    }

    #[test]
    fn parse_max_ef_search_missing_returns_none() {
        let c = cfg("c", 128, &["recall_target:0.95"]);
        assert_eq!(parse_max_ef_search(&c), None);
    }

    #[test]
    fn parse_max_ef_search_clamps_extremes() {
        // Below EF_SEARCH_MIN (16) → clamps up
        assert_eq!(
            parse_max_ef_search(&cfg("c", 128, &["max_ef_search:5"])),
            Some(16)
        );
        // Above EF_SEARCH_MAX (2048) → clamps down
        assert_eq!(
            parse_max_ef_search(&cfg("c", 128, &["max_ef_search:99999"])),
            Some(2048)
        );
    }

    #[test]
    fn max_ef_search_clamps_advised_ef_at_create_time() {
        // r=0.95 at N=100K, m=32 wants ef≈405. Cap to 300 — the
        // advisor must clamp + the stamped HnswConfig.ef_search
        // should be 300 (not 405).
        let mut c = cfg(
            "c_clamp",
            128,
            &[
                "recall_target:0.95",
                "max_ef_search:300",
                "target_vector_count:100000",
            ],
        );
        c.index_configs.push(hnsw_idx("primary"));

        let applied = apply_advisor_to_hnsw_indexes(&mut c, 0.95);
        assert_eq!(applied.len(), 1);
        let out = &applied[0].1;
        assert!(out.clamped_by_max_ef, "advisor must report the clamp");
        assert_eq!(out.ef_search, 300);

        // The stamped HnswConfig matches the clamped ef.
        let h = c.index_configs[0].hnsw_config.as_ref().unwrap();
        assert_eq!(h.ef_search, Some(300));
    }

    // ───── max_memory_mb / max_query_latency_ms parsers ──────

    #[test]
    fn parse_max_memory_mb_happy_path() {
        let c = cfg("c", 128, &["max_memory_mb:128.5"]);
        assert_eq!(parse_max_memory_mb(&c), Some(128.5));
    }

    #[test]
    fn parse_max_memory_mb_clamps_extremes() {
        // Below 1 MB → clamps up
        let c1 = cfg("c", 128, &["max_memory_mb:0.5"]);
        assert_eq!(parse_max_memory_mb(&c1), Some(1.0));
        // Above 1 TB → clamps down
        let c2 = cfg("c", 128, &["max_memory_mb:9999999"]);
        assert_eq!(parse_max_memory_mb(&c2), Some(1_048_576.0));
    }

    #[test]
    fn parse_max_query_latency_ms_happy_path() {
        let c = cfg("c", 128, &["max_query_latency_ms:5.0"]);
        assert_eq!(parse_max_query_latency_ms(&c), Some(5.0));
    }

    #[test]
    fn parse_max_query_latency_ms_clamps_extremes() {
        // Below 0.1 ms (= 100μs) → clamps up
        let c1 = cfg("c", 128, &["max_query_latency_ms:0.001"]);
        assert_eq!(parse_max_query_latency_ms(&c1), Some(0.1));
        // Above 10s → clamps down
        let c2 = cfg("c", 128, &["max_query_latency_ms:60000"]);
        assert_eq!(parse_max_query_latency_ms(&c2), Some(10_000.0));
    }

    #[test]
    fn parse_max_memory_mb_missing_returns_none() {
        let c = cfg("c", 128, &[]);
        assert_eq!(parse_max_memory_mb(&c), None);
    }

    // ───── apply_advisor_to_indexes (algorithm-agnostic) ─────

    #[test]
    fn apply_advisor_to_indexes_auto_synthesizes_hnsw_for_high_recall() {
        // r=0.95: HNSW reaches; IVF declines (above ceiling).
        // Selector picks HNSW → synthesizes auto_hnsw_recall_target.
        let mut c = cfg("c_hnsw", 128, &["recall_target:0.95"]);
        let advice = apply_advisor_to_indexes(&mut c, 0.95);
        assert_eq!(advice.len(), 1);
        assert_eq!(advice[0].index_name, AUTO_HNSW_INDEX_NAME);
        assert_eq!(
            advice[0].output.kind,
            crate::index::axis::management::SupportedAlgorithm::Hnsw
        );
        // The synthesized IndexConfig must carry the sized HNSW
        // params verbatim.
        assert_eq!(c.index_configs.len(), 1);
        assert_eq!(c.index_configs[0].algorithm, IndexingAlgorithm::Hnsw as i32);
        let h = c.index_configs[0].hnsw_config.as_ref().unwrap();
        assert!(h.m.is_some() && h.ef_search.is_some());
    }

    #[test]
    fn apply_advisor_to_indexes_sizes_existing_ivf_index() {
        // Caller attached an IVF IndexConfig with no params. The
        // advisor sizes nlist + nprobe from a sub-ceiling recall
        // target.
        let mut c = cfg(
            "c_ivf",
            128,
            &["recall_target:0.70", "target_vector_count:100000"],
        );
        c.index_configs
            .push(crate::proto::proximadb_v1::IndexConfig {
                index_name: "explicit_ivf".to_string(),
                algorithm: IndexingAlgorithm::Ivf as i32,
                ..Default::default()
            });
        let advice = apply_advisor_to_indexes(&mut c, 0.70);
        assert_eq!(advice.len(), 1);
        assert_eq!(advice[0].index_name, "explicit_ivf");
        assert_eq!(
            advice[0].output.kind,
            crate::index::axis::management::SupportedAlgorithm::Ivf
        );
        let i = c.index_configs[0].ivf_config.as_ref().unwrap();
        assert!(i.n_lists.is_some() && i.n_probe.is_some());
    }

    #[test]
    fn parse_binary_rerank_allowed_variants() {
        for raw in ["enabled", "ENABLED", "on", "true", "1", "yes"] {
            let c = cfg("c", 128, &[&format!("binary_rerank:{}", raw)]);
            assert!(
                parse_binary_rerank_allowed(&c),
                "binary_rerank:{} must parse as true",
                raw
            );
        }
        for raw in ["off", "disabled", "0", "no", "garbage", ""] {
            let c = cfg("c", 128, &[&format!("binary_rerank:{}", raw)]);
            assert!(
                !parse_binary_rerank_allowed(&c),
                "binary_rerank:{} must parse as false",
                raw
            );
        }
        // Missing tag → false.
        assert!(!parse_binary_rerank_allowed(&cfg("c", 128, &[])));
    }

    #[test]
    fn binary_rerank_tag_lifts_ivf_ceiling_end_to_end() {
        // At r=0.85 + binary_rerank, IVF *qualifies* (ceiling lifts
        // 0.74 → 0.95) but HNSW still wins per-query work in an
        // unconstrained tie-break (~78 vs ~12K candidates visited).
        // To force the selector down the IVF arm we constrain
        // memory: HNSW (m=32, N=100K, dim=128) needs ~61 MiB of
        // graph+vector storage, IVF+PQ (m=8, nbits=8) fits in
        // ~1 MiB. A `max_memory_mb:30` cap excludes HNSW and
        // admits IVF — and the binary_rerank tag is what lets
        // IVF reach r=0.85 in the first place.
        let mut c = cfg(
            "c_rerank",
            128,
            &[
                "recall_target:0.85",
                "binary_rerank:enabled",
                "max_memory_mb:30",
            ],
        );
        let advice = apply_advisor_to_indexes(&mut c, 0.85);
        assert_eq!(advice.len(), 1);
        // Verify the IVF path was picked.
        assert_eq!(
            advice[0].output.kind,
            crate::index::axis::management::SupportedAlgorithm::Ivf
        );
        // And the stamped IndexConfig is IVF — the synthesised
        // name reflects which path the selector took.
        assert_eq!(c.index_configs[0].algorithm, IndexingAlgorithm::Ivf as i32);
    }

    #[test]
    fn parse_modalities_happy_path() {
        let c = cfg("c", 128, &["modalities:text,image,video"]);
        assert_eq!(
            parse_modalities(&c),
            vec!["text".to_string(), "image".to_string(), "video".to_string()]
        );
    }

    #[test]
    fn parse_modalities_empty_returns_empty_vec() {
        let c = cfg("c", 128, &["recall_target:0.95"]);
        assert!(parse_modalities(&c).is_empty());
    }

    #[test]
    fn parse_modalities_normalises_case_and_whitespace() {
        // "Text" / "TEXT" / "  text  " should all collapse to "text".
        let c = cfg("c", 128, &["modalities: Text , IMAGE,image"]);
        let out = parse_modalities(&c);
        assert_eq!(out, vec!["text".to_string(), "image".to_string()]);
    }

    #[test]
    fn parse_modalities_unions_across_multiple_tags() {
        let c = cfg(
            "c",
            128,
            &["modalities:text,image", "modalities:video,image"],
        );
        let out = parse_modalities(&c);
        assert_eq!(
            out,
            vec!["text".to_string(), "image".to_string(), "video".to_string()],
            "duplicates removed preserving first-occurrence order"
        );
    }

    #[test]
    fn apply_advisor_routes_to_hmgi_when_modalities_set() {
        // With ≥ 2 modalities declared, the advisor selector should
        // either pick HMGI directly or fall back to HNSW with the
        // HMGI option present in the candidate set. Either way the
        // synthesized IndexConfig must NOT block on the modality
        // input (no panic, no failure). Pins that the
        // `parse_modalities → AnnAdvisorInput.modalities` plumbing
        // is wired end-to-end.
        let mut c = cfg(
            "c_multimodal",
            128,
            &["recall_target:0.85", "modalities:text,image"],
        );
        let advice = apply_advisor_to_indexes(&mut c, 0.85);
        assert!(
            !advice.is_empty(),
            "advisor must produce sizing when modalities + recall_target set"
        );
    }

    #[test]
    fn apply_advisor_to_indexes_respects_caller_pinned_ivf() {
        // IVF with pinned n_lists/n_probe → no-op.
        let mut c = cfg("c_pinned_ivf", 128, &["recall_target:0.70"]);
        c.index_configs
            .push(crate::proto::proximadb_v1::IndexConfig {
                index_name: "pinned".to_string(),
                algorithm: IndexingAlgorithm::Ivf as i32,
                ivf_config: Some(crate::proto::proximadb_v1::IvfConfig {
                    n_lists: Some(500),
                    n_probe: Some(25),
                    ..Default::default()
                }),
                ..Default::default()
            });
        let advice = apply_advisor_to_indexes(&mut c, 0.70);
        assert_eq!(advice.len(), 0, "pinned config must be untouched");
        let i = c.index_configs[0].ivf_config.as_ref().unwrap();
        assert_eq!(i.n_lists, Some(500)); // unchanged
        assert_eq!(i.n_probe, Some(25));
    }
}
