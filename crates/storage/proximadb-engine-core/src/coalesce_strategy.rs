//! Cost-driven, tier-aware read-coalescing strategy chooser (TD-RDSTRAT-1 slice 3).
//!
//! Picks *per-block* vs *coalesced* ranged reads from the predicted GET/byte cost and the storage
//! tier — ADR-050/052's "act" on the read path. Both strategies are byte-identical (verified in
//! TD-151 slice 1 + TD-RDSTRAT-1 slice 2), so this only affects cost/latency, never results.
//!
//! This is the chooser *logic* (slice 3a): pure, deterministic, unit-tested. The 2-site wiring
//! (slice 3b) replaces the always-coalesce `if/else` in `sst_io_layer::batch_read_with_filtering`
//! and `sst_query_engine::traditional_search` once TD-151 slice 1 (#780) + TD-RDSTRAT-1 slice 2
//! (#783) land on `develop` (this slice is based on `develop` and so precedes the wiring).

use proximadb_storage_filesystem_types::ObjectAccessTier;

/// The chosen ranged-read pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoalesceStrategy {
    /// One ranged read per block (2 GETs/block on the SST path: 4-byte size prefix + data). Wins on
    /// hot local/NVMe (no GET fee) or when coalescing would over-fetch a large gap on object store.
    PerBlock,
    /// Coalesce adjacent block ranges into fewer GETs. Wins on cold object storage where GET
    /// round-trips dominate cost (ADR-052 bytes-scanned/GET-cost thesis).
    Coalesced,
}

/// Storage-cost class used by the adaptive range-plan estimator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadCostClass {
    /// Local SSD/NVMe: requests are not billed, so avoid gap over-read.
    Local,
    /// Same-region standard/Hot object storage: request latency and transaction
    /// count dominate, while bytes still consume bandwidth and memory.
    HotCloud,
    /// Cool/Cold/Archive object storage: bytes can carry a retrieval charge in
    /// addition to request cost, so amplification is constrained tightly.
    RetrievalBilledCloud,
}

/// Exact cold-miss quantities predicted for one candidate range plan.
///
/// A DRAM or persistent-cache hit can eliminate an object request after this
/// physical plan is chosen, so `get_requests` is the canonical-store miss-path
/// count rather than a claim about the eventual cache outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangePlanEstimate {
    /// Physical requests the candidate will issue when every planned range
    /// misses the warm tiers.
    pub get_requests: u64,
    /// Physical bytes transferred, including bounded gap over-read.
    pub physical_bytes: u64,
}

/// A coalescing policy together with the exact plan it produces for the
/// current selected byte ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangePlanCandidate {
    /// Largest gap this policy may bridge.
    pub max_gap_bytes: u64,
    /// Largest merged physical range. A caller's indivisible logical range may
    /// exceed it; this field controls merging only.
    pub max_range_bytes: u64,
    /// Exact request/byte estimate produced by the real range planner.
    pub estimate: RangePlanEstimate,
}

/// Tier-aware objective and safety bounds for adaptive range selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadPlanCostProfile {
    /// Observable cost class, retained in decision telemetry.
    pub class: ReadCostClass,
    /// Neutral relative cost per physical request.
    pub per_get: f64,
    /// Neutral relative cost per MiB transferred.
    pub per_mib: f64,
    /// Maximum candidate bytes relative to the fixed-policy baseline, in basis
    /// points. `12_500` permits at most 25% amplification.
    pub max_bytes_ratio_bps: u32,
    /// Candidates within this many basis points of minimum score are treated as
    /// the same knee; the smaller range/gap wins to bound RSS and concurrency.
    pub score_tie_bps: u32,
}

impl ReadPlanCostProfile {
    /// Active same-region object-storage profile. The 25% byte guard contains
    /// the measured 1M +18% trade while rejecting unbounded request-only plans.
    pub const HOT_CLOUD: Self = Self {
        class: ReadCostClass::HotCloud,
        per_get: CLOUD_PER_GET,
        per_mib: CLOUD_PER_MIB,
        max_bytes_ratio_bps: 12_500,
        score_tie_bps: 50,
    };

    /// Retrieval-billed object tiers price bytes more heavily and admit only a
    /// small amplification over the conservative baseline.
    pub const RETRIEVAL_BILLED_CLOUD: Self = Self {
        class: ReadCostClass::RetrievalBilledCloud,
        per_get: CLOUD_PER_GET,
        per_mib: 20.0,
        max_bytes_ratio_bps: 11_000,
        score_tie_bps: 50,
    };

    /// Local storage has no request fee. Exact-adjacent reads therefore win
    /// unless coalescing is byte-neutral.
    pub const LOCAL: Self = Self {
        class: ReadCostClass::Local,
        per_get: LOCAL_PER_GET,
        per_mib: LOCAL_PER_MIB,
        max_bytes_ratio_bps: 10_000,
        score_tie_bps: 0,
    };

    /// Resolve a cost profile from the physical backend and optional canonical
    /// per-object access tier. An unset cloud tier means the active/default
    /// standard tier; callers that know a Cool/Cold/Archive placement pass it.
    pub fn for_path(path: &str, access_tier: Option<ObjectAccessTier>) -> Self {
        if !is_cloud_path(path) {
            return Self::LOCAL;
        }
        match access_tier {
            Some(ObjectAccessTier::Cool | ObjectAccessTier::Cold | ObjectAccessTier::Archive) => {
                Self::RETRIEVAL_BILLED_CLOUD
            }
            Some(ObjectAccessTier::Hot) | None => Self::HOT_CLOUD,
        }
    }

    fn score(self, estimate: RangePlanEstimate) -> f64 {
        score(
            self.per_get,
            self.per_mib,
            estimate.get_requests,
            estimate.physical_bytes,
        )
    }
}

/// Auditable outcome of choosing among exact range-plan candidates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangePlanDecision {
    /// Candidate index selected by the objective and knee rule.
    pub chosen_index: usize,
    /// Fixed-policy baseline used for the byte-amplification guard.
    pub baseline: RangePlanCandidate,
    /// Selected candidate.
    pub chosen: RangePlanCandidate,
    /// Number of candidates that survived the byte guard.
    pub admissible_candidates: usize,
    /// Neutral score of the fixed-policy baseline.
    pub baseline_score: f64,
    /// Neutral score of the selected plan.
    pub chosen_score: f64,
}

// Tier-aware cost weights. The cloud set mirrors `RouteCostModel`'s defaults
// (`src/query/route_cost_model.rs`: per_get=20.0, per_mib_read=5.0) — the cloud GET-economics
// encoding where per-GET cost dominates. Local/NVMe I/O has no per-request fee, so per_get ≈ 0
// and the choice is driven by bytes (no over-fetch ⇒ per-block).
const CLOUD_PER_GET: f64 = 20.0;
const CLOUD_PER_MIB: f64 = 5.0;
const LOCAL_PER_GET: f64 = 0.0;
const LOCAL_PER_MIB: f64 = 5.0;

/// Neutral cost score: `per_get · gets + per_mib · (bytes / MiB)` — the same shape as
/// `RouteCostModel::score` (restricted to the read terms).
fn score(per_get: f64, per_mib: f64, gets: u64, bytes: u64) -> f64 {
    let mib = (bytes as f64) / (1024.0 * 1024.0);
    per_get * (gets as f64) + per_mib * mib
}

/// Choose the cost knee among exact plans for the current selected ranges.
///
/// The caller supplies the conservative fixed-policy candidate and any larger
/// caps it can execute. Candidates that exceed the tier's byte-amplification
/// guard are rejected before scoring. Among plans within `score_tie_bps` of the
/// minimum, the smaller maximum range and gap win; this is the measured knee
/// rule that prevents a negligible GET improvement from inflating RSS.
pub fn choose_range_plan(
    candidates: &[RangePlanCandidate],
    baseline_index: usize,
    profile: ReadPlanCostProfile,
) -> Option<RangePlanDecision> {
    let baseline = *candidates.get(baseline_index)?;
    if !profile.per_get.is_finite()
        || !profile.per_mib.is_finite()
        || profile.per_get < 0.0
        || profile.per_mib < 0.0
        || profile.max_bytes_ratio_bps < 10_000
    {
        return None;
    }

    let within_byte_guard = |candidate: &RangePlanCandidate| {
        if baseline.estimate.physical_bytes == 0 {
            return candidate.estimate.physical_bytes == 0;
        }
        u128::from(candidate.estimate.physical_bytes) * 10_000
            <= u128::from(baseline.estimate.physical_bytes)
                * u128::from(profile.max_bytes_ratio_bps)
    };

    let mut admissible: Vec<(usize, RangePlanCandidate, f64)> = candidates
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, candidate)| *index == baseline_index || within_byte_guard(candidate))
        .map(|(index, candidate)| (index, candidate, profile.score(candidate.estimate)))
        .filter(|(_, _, candidate_score)| candidate_score.is_finite())
        .collect();
    if admissible.is_empty() {
        return None;
    }

    let minimum_score = admissible
        .iter()
        .map(|(_, _, candidate_score)| *candidate_score)
        .min_by(f64::total_cmp)?;
    let tie_ceiling = minimum_score * (1.0 + f64::from(profile.score_tie_bps) / 10_000.0);
    admissible.retain(|(_, _, candidate_score)| *candidate_score <= tie_ceiling);
    admissible.sort_by(|left, right| {
        left.1
            .max_range_bytes
            .cmp(&right.1.max_range_bytes)
            .then(left.1.max_gap_bytes.cmp(&right.1.max_gap_bytes))
            .then(
                left.1
                    .estimate
                    .physical_bytes
                    .cmp(&right.1.estimate.physical_bytes),
            )
            .then(
                left.1
                    .estimate
                    .get_requests
                    .cmp(&right.1.estimate.get_requests),
            )
            .then(left.0.cmp(&right.0))
    });
    let (chosen_index, chosen, chosen_score) = *admissible.first()?;

    Some(RangePlanDecision {
        chosen_index,
        baseline,
        chosen,
        admissible_candidates: candidates
            .iter()
            .enumerate()
            .filter(|(index, candidate)| *index == baseline_index || within_byte_guard(candidate))
            .count(),
        baseline_score: profile.score(baseline.estimate),
        chosen_score,
    })
}

/// Choose the cheaper strategy from the predicted per-strategy GET/byte cost and the storage tier.
///
/// - `n_blocks` / `total_block_bytes` describe the **per-block** path (2 GETs/block: a 4-byte size
///   prefix + the data each).
/// - `coalesced_gets` / `coalesced_bytes` describe the **coalesced** path (from the range
///   coalescer's plan estimate — includes gap over-fetch).
///
/// Pure + deterministic (unit-testable). Not a hardcoded selectivity threshold — the crossover
/// falls out of the cost model + geometry per tier. (TD-RDSTRAT-1 slice 3.)
pub fn choose_read_strategy(
    is_cloud: bool,
    n_blocks: u64,
    total_block_bytes: u64,
    coalesced_gets: u64,
    coalesced_bytes: u64,
) -> CoalesceStrategy {
    let (per_get, per_mib) = if is_cloud {
        (CLOUD_PER_GET, CLOUD_PER_MIB)
    } else {
        (LOCAL_PER_GET, LOCAL_PER_MIB)
    };
    // Per-block: 2 GETs/block (4-byte size prefix + data); bytes = data + the 4-byte prefix each.
    let per_block_gets = 2u64.saturating_mul(n_blocks);
    let per_block_bytes = total_block_bytes.saturating_add(4u64.saturating_mul(n_blocks));
    let score_per_block = score(per_get, per_mib, per_block_gets, per_block_bytes);
    let score_coalesced = score(per_get, per_mib, coalesced_gets, coalesced_bytes);
    if score_coalesced <= score_per_block {
        CoalesceStrategy::Coalesced
    } else {
        CoalesceStrategy::PerBlock
    }
}

/// Whether `path` points at a cold object-store backend (vs hot local/NVMe/page-cache). Mirrors the
/// `is_cloud_file` prefix check used in `sst_io_layer.rs`, as a free fn shared by the chooser.
pub fn is_cloud_path(path: &str) -> bool {
    matches!(
        path.split_once("://")
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .as_deref(),
        Some("s3" | "gs" | "gcs" | "az" | "azure" | "adls" | "abfs" | "http" | "https")
    )
}

/// TD-RDSTRAT-1 slice 3: opt-in for the cost-driven chooser. Default OFF — the always-coalesce
/// baseline (slices 1+2) runs; set `PROXIMADB_STORAGE_READ_STRATEGY_CHOOSER=1` to engage (observe mode:
/// the chosen strategy + scores are logged before the ADR-052 observe→flip gate).
pub fn read_strategy_chooser_enabled() -> bool {
    match std::env::var("PROXIMADB_STORAGE_READ_STRATEGY_CHOOSER")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mib(value: u64) -> u64 {
        value * 1024 * 1024
    }

    fn candidate(cap_mib: u64, gets: u64, bytes_mib: u64) -> RangePlanCandidate {
        RangePlanCandidate {
            max_gap_bytes: mib(1),
            max_range_bytes: mib(cap_mib),
            estimate: RangePlanEstimate {
                get_requests: gets,
                physical_bytes: mib(bytes_mib),
            },
        }
    }

    #[test]
    fn hot_cloud_chooses_measured_1m_knee_not_largest_cap() {
        // Rounded forms of the exact-current BIGANN 1M points. 16 and 24 MiB
        // have effectively equal cost; the knee rule must prefer 16 MiB.
        let candidates = [
            candidate(4, 10_077, 28_561),
            candidate(8, 8_177, 33_111),
            candidate(12, 7_796, 33_691),
            candidate(16, 7_657, 33_713),
            candidate(24, 7_647, 33_713),
        ];
        let decision = choose_range_plan(
            &candidates,
            0,
            ReadPlanCostProfile::for_path("az://container/segment.pax", None),
        )
        .expect("non-empty candidate set");

        assert_eq!(decision.chosen_index, 3);
        assert_eq!(decision.chosen.max_range_bytes, mib(16));
    }

    #[test]
    fn hot_cloud_accepts_k90_request_win_with_bounded_bytes() {
        // Exact-current 768-d k_c=90 top-10 shape, scaled to whole units.
        let candidates = [
            candidate(4, 47_514, 132_386),
            candidate(24, 26_790, 137_653),
        ];
        let decision = choose_range_plan(
            &candidates,
            0,
            ReadPlanCostProfile::for_path("azure://container/segment.pax", None),
        )
        .expect("non-empty candidate set");

        assert_eq!(decision.chosen_index, 1);
    }

    #[test]
    fn byte_amplification_guard_rejects_unbounded_hot_merge() {
        let candidates = [candidate(4, 10, 32), candidate(24, 1, 64)];
        let decision = choose_range_plan(
            &candidates,
            0,
            ReadPlanCostProfile::for_path("az://container/segment.pax", None),
        )
        .expect("non-empty candidate set");

        assert_eq!(decision.chosen_index, 0);
        assert_eq!(decision.admissible_candidates, 1);
    }

    #[test]
    fn retrieval_billed_tier_prices_bytes_more_strictly() {
        let candidates = [candidate(4, 10, 32), candidate(16, 8, 36)];
        let hot = choose_range_plan(
            &candidates,
            0,
            ReadPlanCostProfile::for_path("az://container/segment.pax", None),
        )
        .expect("hot decision");
        let cold = choose_range_plan(
            &candidates,
            0,
            ReadPlanCostProfile::for_path(
                "az://container/segment.pax",
                Some(proximadb_storage_filesystem_types::ObjectAccessTier::Cold),
            ),
        )
        .expect("cold decision");

        assert_eq!(hot.chosen_index, 1);
        assert_eq!(cold.chosen_index, 0);
    }

    #[test]
    fn local_disk_does_not_overread_to_save_unpriced_requests() {
        let candidates = [candidate(1, 10, 32), candidate(8, 1, 33)];
        let decision = choose_range_plan(
            &candidates,
            0,
            ReadPlanCostProfile::for_path("file:///data/segment.pax", None),
        )
        .expect("local decision");

        assert_eq!(decision.chosen_index, 0);
    }

    #[test]
    fn every_azure_alias_uses_the_same_cloud_cost_class() {
        for path in [
            "az://container/key",
            "azure://container/key",
            "adls://container/key",
            "abfs://container/key",
        ] {
            assert!(is_cloud_path(path), "{path}");
            assert_eq!(
                ReadPlanCostProfile::for_path(path, None),
                ReadPlanCostProfile::HOT_CLOUD
            );
        }
    }

    #[test]
    fn cloud_coalesceable_blocks_choose_coalesced() {
        // 10 contiguous 64 KiB blocks coalesce to ~1 range. Cloud: per_get dominates ⇒ coalesced
        // (1 GET) crushes per-block (20 GETs).
        let s = choose_read_strategy(true, 10, 10 * 65536, 1, 10 * 65536);
        assert_eq!(s, CoalesceStrategy::Coalesced);
    }

    #[test]
    fn cloud_huge_gap_overfetch_chooses_per_block() {
        // Cloud, but coalescing would over-fetch a 100 MiB gap: per-block (4 GETs, no over-fetch)
        // beats coalesced (1 GET + 100 MiB @ 5/MiB = 500 vs 4·20 = 80).
        let gap = 100 * 1024 * 1024u64;
        let s = choose_read_strategy(true, 2, 2 * 65536, 1, 2 * 65536 + gap);
        assert_eq!(s, CoalesceStrategy::PerBlock);
    }

    #[test]
    fn local_overfetch_chooses_per_block() {
        // Local: per_get = 0 ⇒ driven by bytes. Coalesced over-fetches a 1 MiB gap ⇒ per-block
        // (fewer bytes) wins.
        let s = choose_read_strategy(false, 2, 2 * 65536, 1, 2 * 65536 + 1024 * 1024);
        assert_eq!(s, CoalesceStrategy::PerBlock);
    }

    #[test]
    fn local_contiguous_tie_chooses_coalesced() {
        // Local, no over-fetch (coalesced bytes == per-block bytes): cost tie ⇒ coalesced (fewer
        // syscalls).
        let s = choose_read_strategy(false, 4, 4 * 65536, 1, 4 * 65536);
        assert_eq!(s, CoalesceStrategy::Coalesced);
    }
}
