/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # ProximaDB String Interner
//!
//! [`StringInterner`] — a concurrent string deduplication cache keyed by XxHash64,
//! returning canonical `Arc<str>` handles so many metadata entries can share one
//! allocation. Extracted from the root crate's `storage::cache::orchestrator`
//! during the root-crate decomposition track (it was a recurring root-internal
//! dependency of the metadata layer).

use dashmap::DashMap;
use proximadb_kernel::hash::XxHash64;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Concurrent string interner reducing memory usage by storing each unique
/// string only once.
///
/// Reduces memory usage by storing each unique string only once.
/// Multiple metadata entries can reference the same string via Arc.
///
/// ## Performance:
/// - Lookup: O(1) average with XxHash64
/// - Insert: O(1) amortized
/// - Memory savings: 50-80% for typical metadata
#[derive(Clone)]
pub struct StringInterner {
    /// Map from string hash to Arc<str> for fast deduplication
    /// Using XxHash64 for faster hashing than default hasher
    strings: Arc<DashMap<u64, Arc<str>, BuildHasherDefault<XxHash64>>>,

    /// Statistics for monitoring effectiveness
    stats: Arc<InternerStats>,
}

#[derive(Default)]
struct InternerStats {
    total_lookups: AtomicU64,
    cache_hits: AtomicU64,
    unique_strings: AtomicU64,
    bytes_saved: AtomicU64,
}

impl StringInterner {
    pub fn new() -> Self {
        Self {
            strings: Arc::new(DashMap::with_hasher(
                BuildHasherDefault::<XxHash64>::default(),
            )),
            stats: Arc::new(InternerStats::default()),
        }
    }

    /// Intern a string, returning Arc<str> to the canonical version
    pub async fn intern(&self, s: &str) -> Arc<str> {
        // Compute hash using XxHash64
        let mut hasher = XxHash64::default();
        hasher.write(s.as_bytes());
        let hash = hasher.finish();

        // Check if already interned
        if let Some(entry) = self.strings.get(&hash) {
            self.stats.total_lookups.fetch_add(1, Ordering::Relaxed);
            self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            self.stats
                .bytes_saved
                .fetch_add(s.len() as u64, Ordering::Relaxed);
            return entry.clone();
        }

        // Add new string
        let arc_str: Arc<str> = Arc::from(s);
        self.strings.insert(hash, arc_str.clone());

        self.stats.total_lookups.fetch_add(1, Ordering::Relaxed);
        self.stats.unique_strings.fetch_add(1, Ordering::Relaxed);

        arc_str
    }

    /// Get interning statistics
    pub async fn stats(&self) -> (u64, u64, f64) {
        let total_lookups = self.stats.total_lookups.load(Ordering::Relaxed);
        let cache_hits = self.stats.cache_hits.load(Ordering::Relaxed);
        let unique_strings = self.stats.unique_strings.load(Ordering::Relaxed);
        let bytes_saved = self.stats.bytes_saved.load(Ordering::Relaxed);
        let hit_rate = if total_lookups > 0 {
            cache_hits as f64 / total_lookups as f64
        } else {
            0.0
        };
        (unique_strings, bytes_saved, hit_rate)
    }

    /// Clear the interner (useful for memory pressure)
    pub fn clear(&self) {
        self.strings.clear();
    }
}

impl Default for StringInterner {
    fn default() -> Self {
        Self::new()
    }
}
