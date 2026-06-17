// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Segment registry: shared, in-process registry of PAX segment metadata.
//!
//! `SegmentRegistry` is the bridge between the write path (gRPC v2, Arrow Flight)
//! and the Iceberg REST catalog server.  After a `PaxSegmentWriter::finish()` call
//! the writer registers the resulting `SegmentMeta` here; the Iceberg service reads
//! aggregated stats when generating synthetic snapshots.
//!
//! The registry is `Arc`-cloned into:
//! - `SharedServices::segment_registry` — write path (gRPC v2)
//! - `AppState::segment_registry` — read path (Iceberg REST, REST catalog API)
//!
//! Both get the **same** `Arc` so stats flow through a single source of truth.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use proximadb_storage_common::pax_block::SegmentMeta;

/// Aggregated per-collection stats derived from registered PAX segments.
#[derive(Debug, Clone, Default)]
pub struct CollectionSegmentStats {
    /// Total rows across all registered segments.
    pub row_count: u64,
    /// Total bytes across all registered segments.
    pub size_bytes: u64,
    /// Number of segments (maps to Iceberg `data_files` count).
    pub segment_count: u32,
    /// Earliest `min_timestamp_ns` seen, or 0 if no segments.
    pub min_timestamp_ns: i64,
    /// Latest `max_timestamp_ns` seen, or 0 if no segments.
    pub max_timestamp_ns: i64,
}

struct Inner {
    /// Segments keyed by collection_id.  Multiple segments accumulate per collection.
    segments: HashMap<String, Vec<SegmentMeta>>,
}

/// Shared, lock-protected PAX segment registry.
///
/// Clone-cheap: cloning an `Arc<SegmentRegistry>` shares the same map.
#[derive(Clone)]
pub struct SegmentRegistry(Arc<RwLock<Inner>>);

impl Default for SegmentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentRegistry {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(Inner {
            segments: HashMap::new(),
        })))
    }

    /// Register a PAX segment produced by `PaxSegmentWriter::finish()`.
    pub fn register(&self, collection_id: impl Into<String>, meta: SegmentMeta) {
        self.0
            .write()
            .segments
            .entry(collection_id.into())
            .or_default()
            .push(meta);
    }

    /// Return aggregated stats for a collection, or `None` if no segments registered.
    pub fn stats(&self, collection_id: &str) -> Option<CollectionSegmentStats> {
        let guard = self.0.read();
        let segs = guard.segments.get(collection_id)?;
        if segs.is_empty() {
            return None;
        }

        let mut agg = CollectionSegmentStats {
            min_timestamp_ns: i64::MAX,
            max_timestamp_ns: i64::MIN,
            ..Default::default()
        };

        for seg in segs {
            agg.row_count += seg.row_count;
            agg.size_bytes += seg.size_bytes;
            agg.segment_count += 1;

            for block in &seg.block_stats {
                if block.min_timestamp_ns != 0 && block.min_timestamp_ns < agg.min_timestamp_ns {
                    agg.min_timestamp_ns = block.min_timestamp_ns;
                }
                if block.max_timestamp_ns != 0 && block.max_timestamp_ns > agg.max_timestamp_ns {
                    agg.max_timestamp_ns = block.max_timestamp_ns;
                }
            }
        }

        // Normalise sentinel values
        if agg.min_timestamp_ns == i64::MAX {
            agg.min_timestamp_ns = 0;
        }
        if agg.max_timestamp_ns == i64::MIN {
            agg.max_timestamp_ns = 0;
        }

        Some(agg)
    }

    /// Clear all segments for a collection (e.g., after compaction).
    pub fn clear(&self, collection_id: &str) {
        self.0.write().segments.remove(collection_id);
    }

    /// List all collection IDs that have registered segments.
    pub fn collections(&self) -> Vec<String> {
        self.0.read().segments.keys().cloned().collect()
    }
}
