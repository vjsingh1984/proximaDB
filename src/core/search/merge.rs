//! Phase 5 Slice 5.4 — Merge WAL/memtable delta with directory-routed
//! search results.
//!
//! The strong-consistency read path produces two candidate sets per
//! query:
//!
//! 1. `directory_results` — records selected by the
//!    [`VectorObjectEconomyDirectory`](crate::storage::engines::sst::object_economy_directory::VectorObjectEconomyDirectory)-routed
//!    search over committed-and-flushed SST blocks. These are visible
//!    up to the directory's `freshness_watermark_lsn`.
//! 2. `delta_results` — records returned by
//!    [`scan_wal_delta_if_needed`](crate::services::operations::vectors::VectorOperationsService::scan_wal_delta_if_needed)
//!    for the WAL/memtable region committed after the watermark.
//!    Tombstones in this set are marked with an empty `vector` plus a
//!    set `expires_at`.
//!
//! [`merge_delta_with_directory_results`] combines the two with these
//! rules:
//!
//! * **Delta wins on OID collision.** A record present in both sets
//!   means the WAL has a newer version (or a tombstone) of the same
//!   OID. The flushed copy is stale; the delta version is
//!   authoritative.
//! * **Tombstones suppress.** A tombstone in the delta removes that
//!   OID from the merged set entirely, regardless of whether the
//!   directory had a live copy.
//! * **Top-k truncation comes last.** After dedupe and tombstone
//!   handling, results are re-sorted by score (descending) and
//!   truncated to `top_k`.
//!
//! The function is a pure data transform: no I/O, no concurrency, no
//! filesystem. Fully unit-testable.

use std::collections::HashMap;

use crate::core::search::results::OptimizedSearchRecord;

/// True when this record is a WAL-emitted tombstone marker.
///
/// The WAL's `search_unflushed_vectors` returns tombstones with
/// `vector = None` and `expires_at = Some(t)` where `t` is the
/// expiration timestamp (already validated to be in the past at the
/// time of emission). Other empty-vector results (e.g. records
/// returned with `include_vectors=false`) carry no `expires_at`, so
/// the combined check is the right distinguishing signal for the
/// merge step.
fn is_wal_tombstone(record: &OptimizedSearchRecord) -> bool {
    // Canonical tombstone marker: valid_to_ns == Some(0). (The earlier
    // `vector.is_none() && expires_at.is_some()` heuristic broke under
    // `include_vectors=false`, where every delta record has vector == None,
    // and relied on the unit-muddled expires_at.) valid_to_ns is the
    // ns-accurate source of truth.
    record.valid_to_ns == Some(0)
}

/// Merge WAL/memtable delta candidates with directory-routed candidates
/// into a single top-k result list with MVCC semantics.
///
/// See module-level docs for the rules. The function does not mutate
/// either input vector beyond consuming them.
pub fn merge_delta_with_directory_results(
    delta_results: Vec<OptimizedSearchRecord>,
    directory_results: Vec<OptimizedSearchRecord>,
    top_k: usize,
) -> Vec<OptimizedSearchRecord> {
    if top_k == 0 {
        return Vec::new();
    }

    // Pass 1: track tombstoned OIDs. These suppress matching directory
    // records and are themselves dropped from the output.
    let mut tombstone_ids: HashMap<String, ()> = HashMap::new();
    for record in &delta_results {
        if is_wal_tombstone(record) {
            tombstone_ids.insert(record.id.clone(), ());
        }
    }

    // Pass 2: build the merged map. Insert delta records first so the
    // "delta wins" rule is enforced by the subsequent directory pass
    // skipping any OIDs already seen.
    //
    // A tombstone in the delta suppresses *every* copy of that OID — not
    // just the flushed/directory copy but also the live copy that may sit
    // beside it in the same unflushed WAL (insert-then-delete before any
    // flush). Without this guard the live WAL record survives into the
    // output because it is itself not a tombstone, so `is_wal_tombstone`
    // returns false and it gets inserted here while the tombstone is dropped.
    let mut by_oid: HashMap<String, OptimizedSearchRecord> = HashMap::new();
    for record in delta_results {
        if is_wal_tombstone(&record) {
            // Tombstones never appear in the output set.
            continue;
        }
        if tombstone_ids.contains_key(&record.id) {
            // A tombstone for this OID exists in the same delta — the
            // delete supersedes this live copy.
            continue;
        }
        by_oid.insert(record.id.clone(), record);
    }
    for record in directory_results {
        if tombstone_ids.contains_key(&record.id) {
            continue;
        }
        // Delta-wins: don't overwrite a delta entry with a stale flushed copy.
        by_oid.entry(record.id.clone()).or_insert(record);
    }

    // Pass 3: collect, sort by score desc (ties broken by id for
    // determinism), truncate to top_k.
    let mut merged: Vec<OptimizedSearchRecord> = by_oid.into_values().collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    merged.truncate(top_k);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::results::OptimizedSearchRecord;

    fn live(id: &str, score: f32) -> OptimizedSearchRecord {
        OptimizedSearchRecord {
            id: id.to_string(),
            vector_id: Some(id.to_string()),
            score,
            similarity: Some(score),
            ..Default::default()
        }
    }

    fn tombstone(id: &str) -> OptimizedSearchRecord {
        OptimizedSearchRecord {
            id: id.to_string(),
            vector_id: Some(id.to_string()),
            score: 0.0,
            similarity: Some(0.0),
            vector: None,
            // Canonical tombstone marker (valid_to_ns == Some(0)); expires_at
            // is display-only.
            valid_to_ns: Some(0),
            expires_at: Some(0),
            ..Default::default()
        }
    }

    #[test]
    fn empty_delta_returns_directory_results_unchanged_after_truncation() {
        let directory = vec![live("a", 0.9), live("b", 0.8), live("c", 0.7)];
        let merged = merge_delta_with_directory_results(vec![], directory, 10);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].id, "a");
        assert_eq!(merged[1].id, "b");
        assert_eq!(merged[2].id, "c");
    }

    #[test]
    fn delta_only_oid_is_included_in_merged_results() {
        let directory = vec![live("a", 0.9), live("b", 0.8)];
        let delta = vec![live("z", 0.95)];
        let merged = merge_delta_with_directory_results(delta, directory, 10);

        assert_eq!(merged.len(), 3);
        assert_eq!(
            merged[0].id, "z",
            "delta record with highest score wins position 0"
        );
        assert_eq!(merged[1].id, "a");
        assert_eq!(merged[2].id, "b");
    }

    #[test]
    fn same_oid_in_both_sets_delta_version_wins() {
        // Directory has a stale "a" with score 0.5; delta has the same
        // OID with a refreshed score 0.95. Delta must win.
        let directory = vec![live("a", 0.5), live("b", 0.8)];
        let delta = vec![live("a", 0.95)];
        let merged = merge_delta_with_directory_results(delta, directory, 10);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a");
        assert_eq!(merged[0].score, 0.95, "delta-wins: refreshed score visible");
        assert_eq!(merged[1].id, "b");
    }

    #[test]
    fn tombstone_in_delta_suppresses_directory_result() {
        let directory = vec![live("a", 0.9), live("b", 0.8)];
        let delta = vec![tombstone("a")];
        let merged = merge_delta_with_directory_results(delta, directory, 10);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "b", "tombstoned OID removed from output");
    }

    #[test]
    fn tombstone_in_delta_suppresses_live_copy_in_same_delta() {
        // Insert-then-delete before any flush: both the live record and its
        // tombstone are unflushed, so both surface in the WAL delta. The
        // tombstone must suppress the live copy sitting beside it in the
        // same delta — not just the flushed/directory copy.
        let directory: Vec<OptimizedSearchRecord> = vec![];
        let delta = vec![
            live("a", 0.9), // live copy, inserted
            tombstone("a"), // tombstone, same OID, newer
            live("b", 0.8),
        ];
        let merged = merge_delta_with_directory_results(delta, directory, 10);

        assert_eq!(
            merged.len(),
            1,
            "live + tombstone for the same OID in the delta collapses to nothing"
        );
        assert_eq!(merged[0].id, "b");
    }

    #[test]
    fn mixed_directory_delta_tombstone_yields_expected_count() {
        // 3 directory + 2 delta (one live override + one new OID) + 1
        // tombstone for an unrelated directory OID. Expected output:
        //   - "a" suppressed by tombstone
        //   - "b" overridden by delta
        //   - "c" passes through from directory
        //   - "d" new from delta
        // => 3 final results.
        let directory = vec![live("a", 0.9), live("b", 0.5), live("c", 0.7)];
        let delta = vec![live("b", 0.95), live("d", 0.85), tombstone("a")];
        let merged = merge_delta_with_directory_results(delta, directory, 10);

        assert_eq!(merged.len(), 3);
        let ids: Vec<&str> = merged.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&"a"), "tombstoned a suppressed");
        assert!(ids.contains(&"b"));
        assert!(ids.contains(&"c"));
        assert!(ids.contains(&"d"));
        // Score order: b(0.95) > d(0.85) > c(0.7)
        assert_eq!(merged[0].id, "b");
        assert_eq!(merged[1].id, "d");
        assert_eq!(merged[2].id, "c");
    }

    #[test]
    fn top_k_truncates_after_merge_and_sort() {
        let directory = vec![live("a", 0.9), live("b", 0.8), live("c", 0.7)];
        let delta = vec![live("z", 0.95)];
        let merged = merge_delta_with_directory_results(delta, directory, 2);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "z");
        assert_eq!(merged[1].id, "a");
    }

    #[test]
    fn top_k_zero_returns_empty() {
        let directory = vec![live("a", 0.9)];
        let delta = vec![live("z", 0.95)];
        let merged = merge_delta_with_directory_results(delta, directory, 0);
        assert!(merged.is_empty());
    }

    #[test]
    fn ties_break_by_id_for_determinism() {
        let directory = vec![live("a", 0.5), live("b", 0.5)];
        let merged_run1 = merge_delta_with_directory_results(vec![], directory.clone(), 10);
        let merged_run2 = merge_delta_with_directory_results(vec![], directory, 10);

        assert_eq!(merged_run1[0].id, "a");
        assert_eq!(merged_run1[1].id, "b");
        // Two independent runs must produce the same order.
        assert_eq!(
            merged_run1.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            merged_run2.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
        );
    }
}
