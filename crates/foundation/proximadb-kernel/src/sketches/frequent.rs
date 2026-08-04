// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Space-Saving heavy-hitters sketch (ADR-037 Decision 1 — the count-min /
//! heavy-hitter role).
//!
//! Count-min estimates the frequency of a *known* key but cannot tell you *which*
//! keys are heavy; for the envelope we want the heavy keys **and** their counts
//! (`DocumentStatistics::top_terms`, frequent field values). Space-Saving
//! (Metwally et al.) tracks the top-`capacity` items in bounded state with a
//! monotone over-estimate, and is mergeable for compaction.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One tracked heavy hitter: the item, its estimated count, and the maximum
/// over-estimation error (count is in `[count - error, count]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrequentItem {
    pub item: String,
    pub count: u64,
    pub error: u64,
}

/// Bounded top-K frequency tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrequentItems {
    capacity: usize,
    counts: HashMap<String, (u64, u64)>, // item -> (count, error)
}

impl FrequentItems {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            counts: HashMap::new(),
        }
    }

    /// Observe one occurrence of `item`.
    pub fn insert(&mut self, item: &str) {
        self.add(item, 1);
    }

    /// Observe `weight` occurrences of `item` (used by merge).
    pub fn add(&mut self, item: &str, weight: u64) {
        if weight == 0 {
            return;
        }
        if let Some(entry) = self.counts.get_mut(item) {
            entry.0 = entry.0.saturating_add(weight);
            return;
        }
        if self.counts.len() < self.capacity {
            self.counts.insert(item.to_string(), (weight, 0));
            return;
        }
        // Full: evict the current minimum, inheriting its count as this item's
        // over-estimation error (the Space-Saving guarantee).
        if let Some((min_key, min_count)) = self
            .counts
            .iter()
            .min_by_key(|(_, (c, _))| *c)
            .map(|(k, (c, _))| (k.clone(), *c))
        {
            self.counts.remove(&min_key);
            self.counts.insert(
                item.to_string(),
                (min_count.saturating_add(weight), min_count),
            );
        }
    }

    /// Merge another sketch in (compaction folds segment heavy-hitters into the
    /// collection's). Approximate: we replay the other's tracked items by weight,
    /// preserving the monotone over-estimate.
    pub fn merge(&mut self, other: &FrequentItems) {
        // Replay heaviest-first so the surviving top-K is the most accurate.
        let mut items: Vec<(&String, u64)> =
            other.counts.iter().map(|(k, (c, _))| (k, *c)).collect();
        items.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
        for (item, count) in items {
            self.add(item, count);
        }
    }

    /// Top-`n` items by estimated count, descending. Ties broken by item for
    /// determinism.
    pub fn top(&self, n: usize) -> Vec<FrequentItem> {
        let mut v: Vec<FrequentItem> = self
            .counts
            .iter()
            .map(|(item, (count, error))| FrequentItem {
                item: item.clone(),
                count: *count,
                error: *error,
            })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.item.cmp(&b.item)));
        v.truncate(n);
        v
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_clear_heavy_hitters() {
        let mut f = FrequentItems::new(4);
        for _ in 0..100 {
            f.insert("timeout");
        }
        for _ in 0..50 {
            f.insert("payment");
        }
        for i in 0..20 {
            f.insert(&format!("rare-{i}")); // many distinct rares
        }
        let top = f.top(2);
        assert_eq!(top[0].item, "timeout");
        assert_eq!(top[0].count, 100);
        assert_eq!(top[1].item, "payment");
        // Heavy hitters are exact (never evicted), so error is 0.
        assert_eq!(top[0].error, 0);
    }

    #[test]
    fn capacity_is_bounded() {
        let mut f = FrequentItems::new(8);
        for i in 0..1000 {
            f.insert(&format!("item-{i}"));
        }
        assert!(f.top(1000).len() <= 8);
    }

    #[test]
    fn merge_combines_counts() {
        let mut a = FrequentItems::new(4);
        let mut b = FrequentItems::new(4);
        for _ in 0..30 {
            a.insert("x");
        }
        for _ in 0..40 {
            b.insert("x");
        }
        a.merge(&b);
        let top = a.top(1);
        assert_eq!(top[0].item, "x");
        assert_eq!(top[0].count, 70);
    }

    #[test]
    fn empty_is_empty() {
        assert!(FrequentItems::new(4).is_empty());
        assert!(FrequentItems::new(4).top(10).is_empty());
    }
}
