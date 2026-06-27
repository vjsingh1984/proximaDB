// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! A compact merging t-digest for numeric quantiles
//! (`FieldStatistics::quantiles`, `method: "tdigest"`).
//!
//! Buffers incoming points, then compresses by sort-merging adjacent centroids
//! under a bounded per-centroid weight. This is the "merging" t-digest with a
//! uniform weight cap (`total/compression`) — simpler than the scale-function
//! variant, monotone, and trivially mergeable for compaction. Quantiles are
//! interpolated from the centroid CDF and are approximate (labeled as such).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Centroid {
    mean: f64,
    weight: f64,
}

/// Mergeable approximate-quantile digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TDigest {
    compression: f64,
    centroids: Vec<Centroid>,
    buffer: Vec<f64>,
    count: f64,
    min: f64,
    max: f64,
}

impl Default for TDigest {
    fn default() -> Self {
        Self::new(100.0)
    }
}

impl TDigest {
    pub fn new(compression: f64) -> Self {
        Self {
            compression: compression.max(20.0),
            centroids: Vec::new(),
            buffer: Vec::new(),
            count: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Insert a finite value (NaN/inf are ignored — they have no quantile).
    pub fn insert(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        self.buffer.push(x);
        self.count += 1.0;
        if x < self.min {
            self.min = x;
        }
        if x > self.max {
            self.max = x;
        }
        if self.buffer.len() >= 256 {
            self.compress();
        }
    }

    /// Fold the buffer into the sorted centroid list, bounding each centroid's
    /// weight by `count/compression`.
    fn compress(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        // Seed the working set with existing centroids + buffered points.
        let mut points: Vec<Centroid> = self.centroids.clone();
        for &x in &self.buffer {
            points.push(Centroid {
                mean: x,
                weight: 1.0,
            });
        }
        self.buffer.clear();
        points.sort_by(|a, b| {
            a.mean
                .partial_cmp(&b.mean)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let max_weight = (self.count / self.compression).max(1.0);
        let mut merged: Vec<Centroid> = Vec::with_capacity(points.len());
        for c in points {
            match merged.last_mut() {
                Some(last) if last.weight + c.weight <= max_weight => {
                    let w = last.weight + c.weight;
                    last.mean = (last.mean * last.weight + c.mean * c.weight) / w;
                    last.weight = w;
                }
                _ => merged.push(c),
            }
        }
        self.centroids = merged;
    }

    /// Merge another digest in (compaction). Folds the other's *weighted*
    /// centroids into ours so the union's mass is preserved (replaying bare
    /// centroid means would drop their weights and skew quantiles toward the
    /// denser digest).
    pub fn merge(&mut self, other: &TDigest) {
        let mut other = other.clone();
        other.compress();
        self.count += other.count;
        if other.min < self.min {
            self.min = other.min;
        }
        if other.max > self.max {
            self.max = other.max;
        }
        // Append the other's weighted centroids; compress() re-sorts by mean and
        // re-merges under the union's weight cap.
        self.centroids.extend(other.centroids.iter().copied());
        self.compress();
    }

    /// Estimate the value at quantile `q` in `[0, 1]`. Returns `None` when empty.
    pub fn quantile(&mut self, q: f64) -> Option<f64> {
        self.compress();
        if self.centroids.is_empty() {
            return None;
        }
        let q = q.clamp(0.0, 1.0);
        let total: f64 = self.centroids.iter().map(|c| c.weight).sum();
        if total <= 0.0 {
            return Some(self.min);
        }
        let target = q * total;
        let mut cumulative = 0.0;
        for c in &self.centroids {
            let next = cumulative + c.weight;
            if target <= next {
                return Some(c.mean);
            }
            cumulative = next;
        }
        Some(self.max)
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0.0
    }

    pub fn min(&self) -> Option<f64> {
        if self.count == 0.0 {
            None
        } else {
            Some(self.min)
        }
    }

    pub fn max(&self) -> Option<f64> {
        if self.count == 0.0 {
            None
        } else {
            Some(self.max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_has_no_quantile() {
        let mut d = TDigest::new(100.0);
        assert!(d.quantile(0.5).is_none());
        assert!(d.is_empty());
    }

    #[test]
    fn uniform_median_is_near_middle() {
        let mut d = TDigest::new(100.0);
        for i in 1..=1000 {
            d.insert(i as f64);
        }
        let med = d.quantile(0.5).unwrap_or(0.0);
        assert!((med - 500.0).abs() < 50.0, "median ~500, got {med}");
        let p90 = d.quantile(0.9).unwrap_or(0.0);
        assert!((p90 - 900.0).abs() < 50.0, "p90 ~900, got {p90}");
    }

    #[test]
    fn tracks_min_max() {
        let mut d = TDigest::new(100.0);
        for i in 0..500 {
            d.insert((i % 100) as f64);
        }
        assert_eq!(d.min(), Some(0.0));
        assert_eq!(d.max(), Some(99.0));
    }

    #[test]
    fn merge_preserves_distribution() {
        let mut a = TDigest::new(100.0);
        let mut b = TDigest::new(100.0);
        for i in 1..=500 {
            a.insert(i as f64);
        }
        for i in 501..=1000 {
            b.insert(i as f64);
        }
        a.merge(&b);
        let med = a.quantile(0.5).unwrap_or(0.0);
        assert!((med - 500.0).abs() < 80.0, "merged median ~500, got {med}");
        assert_eq!(a.min(), Some(1.0));
        assert_eq!(a.max(), Some(1000.0));
    }

    #[test]
    fn ignores_non_finite() {
        let mut d = TDigest::new(100.0);
        d.insert(f64::NAN);
        d.insert(f64::INFINITY);
        assert!(d.is_empty());
    }
}
