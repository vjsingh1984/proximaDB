// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! Heuristic-based storage engine selection for newly-created collections.
//!
//! # Why
//!
//! ProximaDB ships multiple storage engines with different recall/latency
//! trade-offs (see `src/storage/engines/` and the per-engine docstrings):
//!
//! * **SST** — write-optimized LSM. Blocks are oid-sorted (lexicographic),
//!   so flat-mode centroid pruning collapses on diffuse vector data
//!   (~5% recall at 100K — see TD-096). SST's *intended* vector workflow is
//!   **with an AXIS index** (HNSW/IVF/PQ) attached, or **with progressive
//!   quantization** (fp8/int8 candidate scan + fp32 rerank). The index /
//!   quantization handles ANN; SST handles the OLTP `oid → record` lookup.
//!
//! * **HELIX** — locality-optimized engine that sorts blocks by
//!   PCA-projected Hilbert curve at flush time
//!   (`src/storage/engines/helix/mod.rs:1279` "SORTED FLUSH OPTIMIZATION").
//!   Block centroids reflect real spatial clusters, so the built-in
//!   zone-map pruning achieves ANN-quality recall *without* a separate
//!   index. The right choice for a pure-vector collection that doesn't
//!   want the build/memory cost of an AXIS HNSW.
//!
//! The original mental model — "SST is default for everything" — produces
//! pathological behaviour on vector collections that don't opt into an
//! index. The heuristic in this module routes such collections to HELIX
//! instead.
//!
//! # Heuristic
//!
//! Given a `CollectionConfig` at create-time, infer the engine via:
//!
//! 1. If `storage_engine` is explicitly set (not Unspecified/None), the
//!    caller wins. The heuristic only fires when the engine is unset.
//! 2. If `auto_index_selection == false`, defer to the SST default and
//!    leave engine choice to the caller (operator opted out of auto-routing).
//! 3. If a `recall_target:<float>` tag is set on the collection, choose
//!    **SST**. The recall-target adaptive stack (advisor, drift
//!    detector, hot-swap, /recluster) lives exclusively on SST+HNSW;
//!    HELIX's Hilbert-sorted block pruning has no ef/m tuning surface,
//!    so a recall_target on HELIX would be a no-op promise.
//! 4. If the collection has `index_configs` populated OR
//!    `quantization.enabled = true`, choose **SST**. Indexes + progressive
//!    quantization compensate for SST's block-pruning limitation.
//! 5. Otherwise (vector collection with no index and no quantization
//!    and no recall_target) choose **HELIX**. Its Hilbert-sorted
//!    blocks deliver ANN-quality recall without an external index.
//!
//! # Observability
//!
//! Each call returns the chosen engine **and a short reason string** so
//! the caller can log/emit-metric for operator visibility:
//!
//! ```ignore
//! let (engine, reason) = infer_storage_engine(&config);
//! tracing::info!(
//!     target: "collection.engine_selector",
//!     collection = %config.name,
//!     chosen_engine = ?engine,
//!     reason = reason,
//!     "auto-selected storage engine"
//! );
//! ```
//!
//! # Future
//!
//! This is the static heuristic baseline. The RL planner will eventually
//! observe per-collection latency / recall outcomes and learn which
//! (engine, index, quantization) tuple actually performs best for a given
//! workload shape — at that point the heuristic becomes a fallback for
//! cold-start before the RL model has enough signal.

use crate::proto::proximadb_v1::{CollectionConfig, StorageEngine};

/// Static reason codes returned alongside the engine choice. Kept as
/// short literals so they're cheap to log and easy to filter in
/// Prometheus / SIEM pipelines.
pub mod reasons {
    pub const EXPLICIT_OVERRIDE: &str = "explicit_caller_override";
    pub const AUTO_SELECT_DISABLED: &str = "auto_select_disabled_default_sst";
    pub const HAS_INDEX: &str = "has_index_config_route_sst";
    pub const HAS_QUANTIZATION: &str = "has_quantization_route_sst";
    pub const HAS_BOTH: &str = "has_index_and_quantization_route_sst";
    pub const RECALL_TARGET_SET: &str = "recall_target_route_sst";
    pub const VECTOR_NO_INDEX: &str = "vector_no_index_route_helix";
    pub const NON_VECTOR_DEFAULT_SST: &str = "non_vector_default_sst";
}

/// Decide which storage engine to use for a newly-created collection.
///
/// Returns `(engine, reason)`. The reason is one of the constants in
/// [`reasons`] so callers can attach it to structured logs / metrics
/// without doing string parsing.
///
/// See the module docs for the full decision tree.
pub fn infer_storage_engine(config: &CollectionConfig) -> (StorageEngine, &'static str) {
    // (1) Caller-explicit choice always wins.
    let explicit = config
        .storage_engine
        .and_then(|v| StorageEngine::try_from(v).ok())
        .filter(|e| !matches!(e, StorageEngine::Unspecified));
    if let Some(engine) = explicit {
        return (engine, reasons::EXPLICIT_OVERRIDE);
    }

    // (2) Operator opted out of auto-selection — keep SST as the
    // historical default. The proto default is `auto_index_selection = true`
    // so a None here means "the field was unset", which we treat as
    // "auto-select is on" to preserve the heuristic's reach.
    if let Some(false) = config.auto_index_selection {
        return (StorageEngine::Sst, reasons::AUTO_SELECT_DISABLED);
    }

    // (3a) Vector collections that committed to a recall_target tag
    // are opting into the adaptive HNSW path — that path lives
    // exclusively on SST (HELIX's Hilbert-sorted block pruning has
    // no ef/m tuning surface). Route them to SST even when the
    // caller didn't explicitly attach an HNSW IndexConfig — the
    // recall_target wiring downstream synthesizes one from the
    // advisor's recommendation.
    if super::recall_target::parse_recall_target(config).is_some() {
        return (StorageEngine::Sst, reasons::RECALL_TARGET_SET);
    }

    // (3b) Vector collections that opted into an index or quantization
    // get SST. The index/quantization handles ANN; SST handles
    // oid-keyed point lookups efficiently via bloom + binary search.
    let has_index = !config.index_configs.is_empty();
    let has_quant = config
        .quantization
        .as_ref()
        .and_then(|q| q.enabled)
        .unwrap_or(false);
    match (has_index, has_quant) {
        (true, true) => return (StorageEngine::Sst, reasons::HAS_BOTH),
        (true, false) => return (StorageEngine::Sst, reasons::HAS_INDEX),
        (false, true) => return (StorageEngine::Sst, reasons::HAS_QUANTIZATION),
        (false, false) => {}
    }

    // (4) No index, no quantization. Two sub-cases:
    //   * Vector collection (dimension > 0): route to HELIX so the
    //     Hilbert-sorted block layout delivers usable recall without an
    //     external index.
    //   * Non-vector collection (dimension = 0, e.g. metadata-only
    //     storage): SST is the right home.
    if config.dimension > 0 {
        (StorageEngine::Helix, reasons::VECTOR_NO_INDEX)
    } else {
        (StorageEngine::Sst, reasons::NON_VECTOR_DEFAULT_SST)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{IndexConfig, QuantizationConfig};

    fn vec_config(name: &str) -> CollectionConfig {
        CollectionConfig {
            name: name.to_string(),
            dimension: 128,
            ..Default::default()
        }
    }

    #[test]
    fn explicit_engine_wins_over_heuristic() {
        let mut cfg = vec_config("t1");
        cfg.storage_engine = Some(StorageEngine::Viper as i32);
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Viper);
        assert_eq!(reason, reasons::EXPLICIT_OVERRIDE);
    }

    #[test]
    fn unspecified_engine_treated_as_unset() {
        let mut cfg = vec_config("t2");
        cfg.storage_engine = Some(StorageEngine::Unspecified as i32);
        let (engine, reason) = infer_storage_engine(&cfg);
        // Falls through to heuristic — vector collection, no index → HELIX.
        assert_eq!(engine, StorageEngine::Helix);
        assert_eq!(reason, reasons::VECTOR_NO_INDEX);
    }

    #[test]
    fn auto_select_false_picks_sst() {
        let mut cfg = vec_config("t3");
        cfg.auto_index_selection = Some(false);
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Sst);
        assert_eq!(reason, reasons::AUTO_SELECT_DISABLED);
    }

    #[test]
    fn vector_with_index_picks_sst() {
        let mut cfg = vec_config("t4");
        cfg.index_configs.push(IndexConfig::default());
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Sst);
        assert_eq!(reason, reasons::HAS_INDEX);
    }

    #[test]
    fn vector_with_quantization_picks_sst() {
        let mut cfg = vec_config("t5");
        cfg.quantization = Some(QuantizationConfig {
            enabled: Some(true),
            ..Default::default()
        });
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Sst);
        assert_eq!(reason, reasons::HAS_QUANTIZATION);
    }

    #[test]
    fn vector_with_index_and_quantization_picks_sst() {
        let mut cfg = vec_config("t6");
        cfg.index_configs.push(IndexConfig::default());
        cfg.quantization = Some(QuantizationConfig {
            enabled: Some(true),
            ..Default::default()
        });
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Sst);
        assert_eq!(reason, reasons::HAS_BOTH);
    }

    #[test]
    fn vector_no_index_no_quantization_picks_helix() {
        let cfg = vec_config("t7");
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Helix);
        assert_eq!(reason, reasons::VECTOR_NO_INDEX);
    }

    #[test]
    fn recall_target_tag_routes_to_sst_without_explicit_index() {
        // A recall_target tag alone, no index_configs, no
        // quantization, vector collection: the previous default
        // would have been HELIX (vector-no-index rule), but the
        // recall-target adaptive stack only works on SST+HNSW,
        // so the override fires.
        let mut cfg = vec_config("t10");
        cfg.tags = vec!["recall_target:0.95".to_string()];
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Sst);
        assert_eq!(reason, reasons::RECALL_TARGET_SET);
    }

    #[test]
    fn recall_target_tag_still_loses_to_explicit_engine() {
        // The explicit-override rule from step (1) takes precedence
        // — operators who pin storage_engine know what they want.
        let mut cfg = vec_config("t11");
        cfg.tags = vec!["recall_target:0.95".to_string()];
        cfg.storage_engine = Some(StorageEngine::Helix as i32);
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Helix);
        assert_eq!(reason, reasons::EXPLICIT_OVERRIDE);
    }

    #[test]
    fn quantization_disabled_does_not_route_to_sst() {
        let mut cfg = vec_config("t8");
        cfg.quantization = Some(QuantizationConfig {
            enabled: Some(false),
            ..Default::default()
        });
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Helix);
        assert_eq!(reason, reasons::VECTOR_NO_INDEX);
    }

    #[test]
    fn non_vector_collection_picks_sst() {
        let mut cfg = vec_config("t9");
        cfg.dimension = 0;
        let (engine, reason) = infer_storage_engine(&cfg);
        assert_eq!(engine, StorageEngine::Sst);
        assert_eq!(reason, reasons::NON_VECTOR_DEFAULT_SST);
    }
}
