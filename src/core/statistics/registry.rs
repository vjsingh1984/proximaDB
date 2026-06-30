// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Process-global resident statistics registry (ADR-037 TD-174).
//!
//! The [`StatisticsSummary`] for each collection lives here so the two ends of
//! the write boundary can reach it without threading it through every engine
//! constructor: the **SST flush/compaction path** updates it (sibling of the
//! KSU meter), and the **REST introspection surface** reads it to project the
//! envelope. This mirrors the existing `get_sst_axis_manager()` global-singleton
//! seam already used to bridge the storage engine and the REST layer.
//!
//! Resident and in-memory only (v1): a restart drops summaries until the next
//! flush re-populates them — honest, since freshness then reflects the last
//! observed flush. The lock is never force-unwrapped: a poisoned lock degrades
//! to "no statistics", never a panic (panic-policy mandate).

use super::{StatisticsEnvelope, StatisticsSummary};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// Per-collection resident statistics, keyed by collection id.
#[derive(Default)]
pub struct StatisticsRegistry {
    inner: RwLock<HashMap<String, StatisticsSummary>>,
}

static GLOBAL: OnceLock<StatisticsRegistry> = OnceLock::new();

/// The process-wide registry.
pub fn global() -> &'static StatisticsRegistry {
    GLOBAL.get_or_init(StatisticsRegistry::default)
}

impl StatisticsRegistry {
    /// Apply `f` to the collection's summary, creating it if absent. Used by the
    /// flush/compaction hook to fold in observations and stamp freshness.
    pub fn update<F>(&self, collection_id: &str, f: F)
    where
        F: FnOnce(&mut StatisticsSummary),
    {
        let Ok(mut map) = self.inner.write() else {
            return; // poisoned lock → skip; statistics are best-effort
        };
        let entry = map
            .entry(collection_id.to_string())
            .or_insert_with(|| StatisticsSummary::new(collection_id));
        f(entry);
    }

    /// Replace a collection's summary wholesale (e.g. compaction recomputed it).
    pub fn put(&self, summary: StatisticsSummary) {
        if let Ok(mut map) = self.inner.write() {
            map.insert(summary.collection_id().to_string(), summary);
        }
    }

    /// Project the envelope for a collection, if a summary exists.
    pub fn envelope(&self, collection_id: &str) -> Option<StatisticsEnvelope> {
        let map = self.inner.read().ok()?;
        map.get(collection_id).map(|s| s.to_envelope())
    }

    /// Equality selectivity for `field = ?` (feeds ADR-004), if known.
    pub fn equality_selectivity(&self, collection_id: &str, field: &str) -> Option<f64> {
        let map = self.inner.read().ok()?;
        map.get(collection_id)?.equality_selectivity(field)
    }

    /// True if a resident summary exists for the collection.
    pub fn contains(&self, collection_id: &str) -> bool {
        self.inner
            .read()
            .map(|m| m.contains_key(collection_id))
            .unwrap_or(false)
    }

    /// Drop a collection's summary (collection deleted/dropped).
    pub fn remove(&self, collection_id: &str) {
        if let Ok(mut map) = self.inner.write() {
            map.remove(collection_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_then_read_envelope() {
        let reg = StatisticsRegistry::default();
        reg.update("c1", |s| {
            s.set_record_count(42);
            s.set_sizes(4096, Some(512));
            s.set_freshness("2026-06-26T00:00:00Z", "flush", Some(7));
        });
        let env = reg.envelope("c1").expect("summary exists");
        assert_eq!(env.record_count, 42);
        assert_eq!(env.freshness.segment_watermark, Some(7));
        assert!(reg.contains("c1"));
        assert!(reg.envelope("missing").is_none());
    }

    #[test]
    fn remove_drops_summary() {
        let reg = StatisticsRegistry::default();
        reg.update("c1", |s| s.set_record_count(1));
        reg.remove("c1");
        assert!(!reg.contains("c1"));
    }
}
