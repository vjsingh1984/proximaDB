/*
 * Copyright 2025 Vijaykumar Singh
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

//! # TD-042 Cache Architecture Consolidation - Performance Benchmarks
//!
//! This benchmark suite measures the performance improvements from TD-042 implementation:
//! - Unified cache interface overhead
//! - String interner effectiveness
//! - Coordinated eviction performance
//! - Cross-cache invalidation efficiency
//!
//! ## Expected Performance Improvements
//!
//! 1. **String Interner**: 50-70% reduction in string memory usage
//! 2. **Unified Interface**: < 5% overhead compared to direct access
//! 3. **Coordinated Eviction**: 20-40% faster eviction decisions
//! 4. **Cross-Cache Invalidation**: 10-30% faster cascade invalidation

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::storage::cache::{
    unified_cache::{CacheId, UnifiedCache, UnifiedCacheCoordinator},
    unified_eviction::{EvictionConfig, UnifiedEvictionPolicy},
};
use std::sync::Arc;
use std::time::Duration;

/// Mock cache implementation for benchmarking
struct MockCache {
    cache_id: CacheId,
    data: Arc<parking_lot::Mutex<std::collections::HashMap<String, serde_json::Value>>>,
    size_bytes: Arc<std::sync::atomic::AtomicU64>,
}

impl MockCache {
    fn new(cache_id: CacheId) -> Self {
        Self {
            cache_id,
            data: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            size_bytes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn insert(&self, key: String, value: serde_json::Value, size: u64) {
        let mut data = self.data.lock();
        data.insert(key, value);
        self.size_bytes
            .fetch_add(size, std::sync::atomic::Ordering::Relaxed);
    }

    fn get_memory_usage(&self) -> u64 {
        self.size_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Benchmark string interner effectiveness
fn bench_string_interner(c: &mut Criterion) {
    let mut group = c.benchmark_group("td042_string_interner");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    let coordinator = Arc::new(UnifiedCacheCoordinator::new());
    let interner = coordinator.string_interner();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Benchmark string interning with various duplication levels
    for (dedup_ratio, unique_count, total_count) in [
        (10, 100, 1000),   // 10% unique
        (50, 100, 5000),   // 2% unique
        (100, 100, 10000), // 1% unique
    ] {
        group.throughput(Throughput::Elements(total_count as u64));
        group.bench_with_input(
            BenchmarkId::new("interner", dedup_ratio),
            &dedup_ratio,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        // Create strings with high duplication
                        for i in 0..total_count {
                            let unique_id = i % unique_count;
                            let string_to_intern =
                                format!("cache_key_{}_very_long_string_{}", unique_id, i);
                            black_box(interner.intern(black_box(&string_to_intern)).await);
                        }
                    });
                });
            },
        );
    }

    // Baseline: String creation without interning
    group.bench_function("no_interner_baseline", |b| {
        b.iter(|| {
            // Create same number of strings without interning
            for i in 0..1000 {
                let string = format!("cache_key_{}_very_long_string_{}", i % 100, i);
                black_box(string);
            }
        });
    });

    group.finish();
}

/// Benchmark unified cache interface overhead
fn bench_unified_interface_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("td042_interface_overhead");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    let coordinator = Arc::new(UnifiedCacheCoordinator::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Create mock caches for different cache types
    let vector_cache = MockCache::new(CacheId::VectorData);
    let metadata_cache = MockCache::new(CacheId::Metadata);
    let query_cache = MockCache::new(CacheId::QueryResult);

    // Pre-populate caches with data
    for i in 0..1000 {
        vector_cache.insert(format!("vec_{}", i), serde_json::json!(i), 100);
        metadata_cache.insert(format!("meta_{}", i), serde_json::json!(i), 50);
        query_cache.insert(format!("query_{}", i), serde_json::json!(i), 75);
    }

    // Benchmark coordinator statistics retrieval
    group.bench_function("coordinator_stats", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(coordinator.get_all_stats().await);
            });
        });
    });

    // Benchmark memory pressure detection
    let config = EvictionConfig::default();
    let policy = UnifiedEvictionPolicy::new(coordinator, config);

    group.bench_function("pressure_detection", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(policy.get_pressure_status().await);
            });
        });
    });

    group.finish();
}

/// Benchmark coordinated eviction performance
fn bench_coordinated_eviction(c: &mut Criterion) {
    let mut group = c.benchmark_group("td042_coordinated_eviction");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);

    let coordinator = Arc::new(UnifiedCacheCoordinator::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Create eviction policy with various cache configurations
    for cache_count in [3, 5, 7] {
        let mut config = EvictionConfig::default();
        config.total_memory_budget = (cache_count * 100_000_000) as u64; // 100MB per cache

        let policy = UnifiedEvictionPolicy::new(coordinator.clone(), config);

        group.bench_with_input(
            BenchmarkId::new("eviction_check", cache_count),
            &cache_count,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        black_box(policy.check_memory_pressure(black_box(false)).await);
                    });
                });
            },
        );
    }

    // Benchmark forced eviction
    let config = EvictionConfig::default();
    let policy = UnifiedEvictionPolicy::new(coordinator, config);

    group.bench_function("forced_eviction", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(policy.check_memory_pressure(black_box(true)).await);
            });
        });
    });

    group.finish();
}

/// Benchmark cache priority system
fn bench_cache_priority_system(c: &mut Criterion) {
    let mut group = c.benchmark_group("td042_priority_system");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(100);

    let coordinator = Arc::new(UnifiedCacheCoordinator::new());
    let config = EvictionConfig::default();
    let policy = UnifiedEvictionPolicy::new(coordinator, config);

    // Benchmark priority lookups for different cache types
    for cache_id in [
        CacheId::VectorData,
        CacheId::Metadata,
        CacheId::QueryResult,
        CacheId::BitmapFilter,
        CacheId::IndexNode,
    ] {
        group.bench_with_input(
            BenchmarkId::new("priority_lookup", format!("{:?}", cache_id)),
            &cache_id,
            |b, cache_id| {
                b.iter(|| {
                    black_box(policy.get_cache_priority(black_box(*cache_id)));
                });
            },
        );
    }

    group.finish();
}

/// Benchmark memory pressure handling
fn bench_memory_pressure_handling(c: &mut Criterion) {
    let mut group = c.benchmark_group("td042_memory_pressure");
    group.measurement_time(Duration::from_secs(6));
    group.sample_size(50);

    let coordinator = Arc::new(UnifiedCacheCoordinator::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Create eviction policies with different pressure thresholds
    for threshold in [0.7, 0.8, 0.9, 0.95] {
        let mut config = EvictionConfig::default();
        config.pressure_threshold = threshold;

        let policy = UnifiedEvictionPolicy::new(coordinator.clone(), config);

        group.bench_with_input(
            BenchmarkId::new("pressure_check", threshold),
            &threshold,
            |b, _| {
                b.iter(|| {
                    rt.block_on(async {
                        black_box(policy.get_pressure_status().await);
                    });
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_string_interner,
    bench_unified_interface_overhead,
    bench_coordinated_eviction,
    bench_cache_priority_system,
    bench_memory_pressure_handling
);
criterion_main!(benches);
