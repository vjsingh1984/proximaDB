// Graph-tunneling predicate gate — LLD §4, anchored on GateANN arXiv 2603.21466.
//
// In a filtered ANN search at low selectivity (typically ≤10%), the dominant
// cost is fetching full-record pages for nodes that ultimately fail the
// predicate. GateANN's insight: graph traversal only needs the neighbor list
// + an approximate distance estimate, not the full vector. If we keep both
// of those in memory we can evaluate the predicate **before** issuing the
// SSD read, and route *through* non-matching nodes via in-memory adjacency
// — they're useless as results but essential as routing states.
//
// The paper reports 10× fewer SSD reads and 7.6× higher throughput at 10%
// selectivity on BigANN-100M with this technique on an unmodified graph.
//
// This module exposes the pure-logic primitive: given a query, a predicate
// shape, and a candidate node, decide whether the node should be:
//
//   - `Fetch`    — read the full record from storage and consider it as
//                  a result candidate (the node both matches the predicate
//                  *and* is close enough to the query).
//   - `Tunnel`   — keep traversing through this node without fetching from
//                  storage. It fails the predicate but its neighbors might
//                  still match.
//   - `Skip`     — neither matches nor is worth traversing through (too far
//                  from the query relative to candidates already in flight).
//
// The gate also bounds traversal depth so we don't waste latency chasing
// long chains of non-matching nodes when the filter is highly repulsive.

use std::time::Duration;

/// Inputs the tunnel gate consumes per node decision.
#[derive(Debug, Clone)]
pub struct TunnelInputs<'a> {
    /// Approximate distance from query to candidate node, in the index's
    /// distance metric (lower = closer). The runtime supplies this from
    /// the quantized graph page so no SSD read happens here.
    pub approx_distance: f32,
    /// Whether the candidate node satisfies the query predicate.
    /// `None` means the predicate is unknown (the runtime hasn't classified
    /// this node yet); we conservatively tunnel.
    pub matches_predicate: Option<bool>,
    /// Distance of the current `top_k`th best candidate. Nodes farther than
    /// this can't displace the current top — used as a Skip threshold.
    pub worst_top_k_distance: f32,
    /// How many tunnel hops we've already taken in a row. The gate caps
    /// this to bound latency.
    pub current_tunnel_depth: u32,
    /// Configuration knobs.
    pub config: &'a TunnelConfig,
}

/// Tunable knobs. Defaults match the LLD §4 guidance.
#[derive(Debug, Clone, Copy)]
pub struct TunnelConfig {
    /// Tunneling engages only when planner-estimated selectivity is at or
    /// below this. Above the threshold, vanilla post-filter beats the
    /// in-memory adjacency walk.
    pub engage_selectivity: f64,
    /// Max consecutive tunnel hops before we stop chasing the chain. Beyond
    /// this we fall back to vanilla post-filter for the current branch.
    pub max_tunnel_depth: u32,
    /// Soft cap on total wall-time the runtime is willing to spend on
    /// tunnel hops per query. The runtime checks this; the gate just
    /// exposes the configured value so observability can track it.
    pub tunnel_time_budget: Duration,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            engage_selectivity: 0.10,
            max_tunnel_depth: 16,
            tunnel_time_budget: Duration::from_millis(20),
        }
    }
}

/// Decision the gate emits for a single candidate node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelDecision {
    /// Issue the SSD read; candidate is a possible result.
    Fetch,
    /// Skip the SSD read; traverse through in-memory adjacency only.
    Tunnel,
    /// Skip both — node is too far from query to matter and won't help
    /// reach better candidates.
    Skip,
}

/// Bookkeeping for the runtime to report into the LLD §10 trace.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TunnelStats {
    /// Nodes the gate decided to fetch (matched and close).
    pub fetched: u32,
    /// Nodes routed through in memory without a fetch.
    pub tunneled: u32,
    /// Nodes the gate skipped entirely.
    pub skipped: u32,
    /// Times the depth cap stopped a tunnel chain.
    pub depth_cap_hits: u32,
}

impl TunnelStats {
    /// Number of SSD reads avoided thanks to tunneling. Equivalent to the
    /// trace's `tunneled_nodes` counter.
    pub fn tunneled_nodes(&self) -> u32 {
        self.tunneled
    }

    /// Combine two stats (e.g. from concurrent shard partitions).
    pub fn merge(&mut self, other: TunnelStats) {
        self.fetched += other.fetched;
        self.tunneled += other.tunneled;
        self.skipped += other.skipped;
        self.depth_cap_hits += other.depth_cap_hits;
    }
}

/// Whether the planner-estimated selectivity is in the tunnel-engagement band.
pub fn should_engage(selectivity: f64, config: &TunnelConfig) -> bool {
    selectivity.is_finite() && selectivity > 0.0 && selectivity <= config.engage_selectivity
}

/// Evaluate a single candidate node. Returns the decision; the runtime is
/// responsible for advancing `current_tunnel_depth` and `worst_top_k_distance`
/// in its own state.
pub fn evaluate(inputs: TunnelInputs<'_>) -> TunnelDecision {
    // Step 1: nodes that already can't displace the current top_k are skipped.
    // The bound is strict — a candidate exactly at the top_k distance is
    // useful only if it matches the predicate and might displace a tie.
    if inputs.approx_distance > inputs.worst_top_k_distance {
        return TunnelDecision::Skip;
    }

    // Step 2: depth-capped tunneling. Beyond max_tunnel_depth we stop
    // chasing the non-matching chain — fall back to Fetch so the vanilla
    // post-filter path can take over for this branch.
    if inputs.current_tunnel_depth >= inputs.config.max_tunnel_depth {
        return TunnelDecision::Fetch;
    }

    // Step 3: predicate gate.
    match inputs.matches_predicate {
        Some(true) => TunnelDecision::Fetch,
        // Non-matching nodes are useless as results but essential as routing
        // states — tunnel through them without a fetch.
        Some(false) => TunnelDecision::Tunnel,
        // Unknown predicate (e.g. the runtime hasn't classified this node
        // yet). Conservatively tunnel — issuing a fetch here would be
        // wasteful if the node turns out to fail the predicate, and
        // tunneling preserves traversal connectivity.
        None => TunnelDecision::Tunnel,
    }
}

/// Helper that updates `TunnelStats` from a sequence of decisions. The
/// runtime calls this per candidate; the gate has no internal state so the
/// helper lives next to it for cohesion.
pub fn record_decision(stats: &mut TunnelStats, decision: TunnelDecision, hit_depth_cap: bool) {
    match decision {
        TunnelDecision::Fetch => stats.fetched += 1,
        TunnelDecision::Tunnel => stats.tunneled += 1,
        TunnelDecision::Skip => stats.skipped += 1,
    }
    if hit_depth_cap {
        stats.depth_cap_hits += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TunnelConfig {
        TunnelConfig::default()
    }

    fn inputs<'a>(
        approx_distance: f32,
        matches: Option<bool>,
        worst_top_k: f32,
        depth: u32,
        cfg: &'a TunnelConfig,
    ) -> TunnelInputs<'a> {
        TunnelInputs {
            approx_distance,
            matches_predicate: matches,
            worst_top_k_distance: worst_top_k,
            current_tunnel_depth: depth,
            config: cfg,
        }
    }

    #[test]
    fn engagement_band_anchors_on_ten_percent() {
        let cfg = config();
        assert!(should_engage(0.05, &cfg));
        assert!(should_engage(0.10, &cfg));
        assert!(!should_engage(0.11, &cfg));
        assert!(!should_engage(0.0, &cfg));
        assert!(!should_engage(f64::NAN, &cfg));
    }

    #[test]
    fn matching_close_node_is_fetched() {
        let cfg = config();
        let d = evaluate(inputs(0.3, Some(true), 0.5, 0, &cfg));
        assert_eq!(d, TunnelDecision::Fetch);
    }

    #[test]
    fn non_matching_close_node_is_tunneled() {
        let cfg = config();
        let d = evaluate(inputs(0.3, Some(false), 0.5, 0, &cfg));
        assert_eq!(d, TunnelDecision::Tunnel);
    }

    #[test]
    fn far_node_is_skipped_regardless_of_predicate() {
        let cfg = config();
        let d = evaluate(inputs(0.9, Some(true), 0.5, 0, &cfg));
        assert_eq!(d, TunnelDecision::Skip);
        // Same skip for non-matching far nodes.
        let d = evaluate(inputs(0.9, Some(false), 0.5, 0, &cfg));
        assert_eq!(d, TunnelDecision::Skip);
    }

    #[test]
    fn depth_cap_forces_fetch_to_break_long_chains() {
        let cfg = config();
        // We've already tunneled max_tunnel_depth times — gate must Fetch
        // so the vanilla post-filter path takes over.
        let d = evaluate(inputs(0.3, Some(false), 0.5, cfg.max_tunnel_depth, &cfg));
        assert_eq!(d, TunnelDecision::Fetch);
    }

    #[test]
    fn unknown_predicate_conservatively_tunnels() {
        let cfg = config();
        let d = evaluate(inputs(0.3, None, 0.5, 0, &cfg));
        assert_eq!(d, TunnelDecision::Tunnel);
    }

    #[test]
    fn stats_record_correctly() {
        let mut s = TunnelStats::default();
        record_decision(&mut s, TunnelDecision::Fetch, false);
        record_decision(&mut s, TunnelDecision::Tunnel, false);
        record_decision(&mut s, TunnelDecision::Tunnel, false);
        record_decision(&mut s, TunnelDecision::Skip, false);
        record_decision(&mut s, TunnelDecision::Fetch, true);
        assert_eq!(s.fetched, 2);
        assert_eq!(s.tunneled, 2);
        assert_eq!(s.skipped, 1);
        assert_eq!(s.depth_cap_hits, 1);
        assert_eq!(s.tunneled_nodes(), 2);
    }

    #[test]
    fn stats_merge_is_additive() {
        let mut a = TunnelStats {
            fetched: 1,
            tunneled: 2,
            skipped: 3,
            depth_cap_hits: 4,
        };
        let b = TunnelStats {
            fetched: 5,
            tunneled: 6,
            skipped: 7,
            depth_cap_hits: 8,
        };
        a.merge(b);
        assert_eq!(a.fetched, 6);
        assert_eq!(a.tunneled, 8);
        assert_eq!(a.skipped, 10);
        assert_eq!(a.depth_cap_hits, 12);
    }

    #[test]
    fn skip_takes_precedence_over_depth_cap() {
        // Even at max tunnel depth, a far node still gets skipped — the
        // depth cap shouldn't pull a useless candidate back into the
        // fetch path.
        let cfg = config();
        let d = evaluate(inputs(0.9, Some(false), 0.5, cfg.max_tunnel_depth, &cfg));
        assert_eq!(d, TunnelDecision::Skip);
    }

    #[test]
    fn boundary_distance_equal_to_top_k_is_eligible() {
        // Exactly at the top_k distance — not strictly farther, so still
        // eligible for fetch (might displace a tie).
        let cfg = config();
        let d = evaluate(inputs(0.5, Some(true), 0.5, 0, &cfg));
        assert_eq!(d, TunnelDecision::Fetch);
    }
}
