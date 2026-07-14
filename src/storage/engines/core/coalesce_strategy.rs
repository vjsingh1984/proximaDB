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

/// PAX cascade whole-vs-striped read decision (ADR-057 / TD-RDSTRAT-3 S2): `true` to
/// take the selective **striped** read (only codes + candidate-rerank + OID stripes,
/// many small GETs, few bytes), `false` for the **whole**-segment read (one GET, all
/// bytes). Same tier-aware cost model as [`choose_read_strategy`]
/// (`per_get · gets + per_mib · bytes`) — on cold object storage the per-GET fee is
/// real, so striped wins only when the byte saving outweighs the extra GETs; on hot
/// local/NVMe (`per_get ≈ 0`) it wins whenever it moves fewer bytes. Pure +
/// deterministic; the crossover falls out of the cost model, not a hardcoded
/// selectivity threshold.
pub fn choose_whole_vs_striped(
    is_cloud: bool,
    whole_bytes: u64,
    striped_gets: u64,
    striped_bytes: u64,
) -> bool {
    let (per_get, per_mib) = if is_cloud {
        (CLOUD_PER_GET, CLOUD_PER_MIB)
    } else {
        (LOCAL_PER_GET, LOCAL_PER_MIB)
    };
    let whole = score(per_get, per_mib, 1, whole_bytes);
    let striped = score(per_get, per_mib, striped_gets, striped_bytes);
    striped < whole
}

/// Whether `path` points at a cold object-store backend (vs hot local/NVMe/page-cache). Mirrors the
/// `is_cloud_file` prefix check used in `sst_io_layer.rs`, as a free fn shared by the chooser.
pub fn is_cloud_path(path: &str) -> bool {
    path.starts_with("s3://")
        || path.starts_with("gs://")
        || path.starts_with("azure://")
        || path.starts_with("http://")
        || path.starts_with("https://")
}

/// TD-RDSTRAT-1 slice 3: opt-in for the cost-driven chooser. Default OFF — the always-coalesce
/// baseline (slices 1+2) runs; set `PROXIMADB_READ_STRATEGY_CHOOSER=1` to engage (observe mode:
/// the chosen strategy + scores are logged before the ADR-052 observe→flip gate).
pub fn read_strategy_chooser_enabled() -> bool {
    match std::env::var("PROXIMADB_READ_STRATEGY_CHOOSER")
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

    #[test]
    fn whole_vs_striped_cloud_batched_selective_picks_striped() {
        // Cloud, big segment, GET-EFFICIENT (batched) striped read: ~4 coalesced
        // GETs move ~1 MiB vs the whole 100 MiB → striped wins (the byte saving
        // outweighs the small per-GET fee).
        assert!(choose_whole_vs_striped(
            true,
            100 * 1024 * 1024,
            4,
            1024 * 1024
        ));
    }

    #[test]
    fn whole_vs_striped_cloud_get_heavy_picks_whole() {
        // Cloud, big segment, but an UNBATCHED striped read (many small per-stripe
        // GETs) — the per-GET fee dominates and beats the byte saving → whole wins.
        // This is the co-design guard against GET-amplification (ADR-052): on cold
        // storage the striped read must BATCH (fs.read_ranges) its stripe reads to
        // win. (The current fs-native cascade path is unbatched, so the chooser
        // correctly declines it on cloud until batching lands — a follow-up.)
        assert!(!choose_whole_vs_striped(
            true,
            100 * 1024 * 1024,
            400,
            1024 * 1024
        ));
    }

    #[test]
    fn whole_vs_striped_cloud_tiny_segment_picks_whole() {
        // Cloud, tiny segment where the striped read's many GETs cost more than the
        // whole's 1 GET + its small byte total → whole wins.
        assert!(!choose_whole_vs_striped(true, 256 * 1024, 400, 200 * 1024));
    }

    #[test]
    fn whole_vs_striped_local_fewer_bytes_picks_striped() {
        // Local (per_get = 0): driven by bytes only → striped wins iff it moves
        // strictly fewer bytes, regardless of GET count.
        assert!(choose_whole_vs_striped(
            false,
            100 * 1024 * 1024,
            9999,
            1024 * 1024
        ));
        // Equal bytes ⇒ not strictly cheaper ⇒ whole.
        assert!(!choose_whole_vs_striped(false, 1024 * 1024, 8, 1024 * 1024));
    }
}
