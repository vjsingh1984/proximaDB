//! Trajectory-analysis pass (Phase 8 F1) — analysis-only refinement.
//!
//! Analyzes provenance + temporal structure over the pinned snapshot (the
//! "trajectory" of how records entered the collection): distinct actors and
//! origins, coverage, and the time span. Reports them as `quality_metrics`. No
//! external model is needed. Records are unchanged (`refined == input`,
//! `removed == 0`); the executor republishes the pinned snapshot. The signals
//! help characterize agent/ingest trajectories an operator — or the F1 trigger
//! arm — can act on.

use std::collections::HashSet;

use anyhow::Result;

use super::PassContext;
use crate::services::discovery::DiscoveryJobResult;

/// Run the trajectory-analysis pass against `ctx.collection_id`. Identity pass
/// (no-op) if the canonical read path is not wired.
pub async fn run(ctx: &PassContext) -> Result<DiscoveryJobResult> {
    let Some(vector_ops) = ctx.vector_ops.as_ref() else {
        return Ok(DiscoveryJobResult::default());
    };
    let collection_id = vector_ops
        .resolve_collection_id(ctx.collection_id.as_str())
        .await;

    let records = vector_ops
        .list_all_records_with_tenant_context(collection_id.as_str(), None)
        .await?;
    let input = records.len() as u64;

    let stats = compute_trajectory(
        records
            .iter()
            .map(|r| (r.actor.as_deref(), r.origin.as_deref(), r.created_at_ns)),
    );

    // Trajectory analysis never removes records: refined == input, removed == 0.
    let mut result = DiscoveryJobResult {
        input_record_count: input,
        refined_record_count: input,
        removed_count: 0,
        ..Default::default()
    };
    let m = &mut result.quality_metrics;
    m.insert("trajectory_input".to_string(), input as f64);
    m.insert("trajectory_with_actor".to_string(), stats.with_actor as f64);
    m.insert(
        "trajectory_distinct_actors".to_string(),
        stats.distinct_actors as f64,
    );
    m.insert(
        "trajectory_with_origin".to_string(),
        stats.with_origin as f64,
    );
    m.insert(
        "trajectory_distinct_origins".to_string(),
        stats.distinct_origins as f64,
    );
    m.insert(
        "trajectory_time_span_ms".to_string(),
        stats.time_span_ns as f64 / 1.0e6,
    );
    Ok(result)
}

#[derive(Debug, Default, PartialEq)]
struct TrajectoryStats {
    with_actor: u64,
    distinct_actors: u64,
    with_origin: u64,
    distinct_origins: u64,
    time_span_ns: i64,
}

/// Provenance + temporal stats over `(actor, origin, created_at_ns)` triples.
/// `time_span_ns` is `max - min` of the timestamps (0 for fewer than 2 records).
fn compute_trajectory<'a>(
    items: impl Iterator<Item = (Option<&'a str>, Option<&'a str>, i64)>,
) -> TrajectoryStats {
    let mut stats = TrajectoryStats::default();
    let mut actors: HashSet<&str> = HashSet::new();
    let mut origins: HashSet<&str> = HashSet::new();
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    let mut count = 0i64;

    for (actor, origin, created_at_ns) in items {
        count += 1;
        if let Some(a) = actor.filter(|s| !s.is_empty()) {
            stats.with_actor += 1;
            actors.insert(a);
        }
        if let Some(o) = origin.filter(|s| !s.is_empty()) {
            stats.with_origin += 1;
            origins.insert(o);
        }
        min_ts = min_ts.min(created_at_ns);
        max_ts = max_ts.max(created_at_ns);
    }

    stats.distinct_actors = actors.len() as u64;
    stats.distinct_origins = origins.len() as u64;
    stats.time_span_ns = if count >= 2 { max_ts - min_ts } else { 0 };
    stats
}

#[cfg(test)]
mod tests {
    use super::compute_trajectory;

    #[test]
    fn empty_is_default() {
        let s = compute_trajectory(std::iter::empty());
        assert_eq!(s.distinct_actors, 0);
        assert_eq!(s.time_span_ns, 0);
    }

    #[test]
    fn counts_distinct_actors_origins_and_span() {
        let items = vec![
            (Some("agent-a"), Some("api"), 1_000i64),
            (Some("agent-a"), Some("cdc"), 3_000i64),
            (Some("agent-b"), None, 2_000i64),
            (None, Some("api"), 5_000i64),
            (Some(""), Some(""), 4_000i64), // empties ignored
        ];
        let s = compute_trajectory(items.into_iter());
        assert_eq!(s.with_actor, 3);
        assert_eq!(s.distinct_actors, 2); // agent-a, agent-b
        assert_eq!(s.with_origin, 3);
        assert_eq!(s.distinct_origins, 2); // api, cdc
        assert_eq!(s.time_span_ns, 4_000); // 5000 - 1000
    }
}
