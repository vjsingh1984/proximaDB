//! Plan execution cache for measured-time fitness in the query runtime.
//!
//! Records observed wall-clock execution time for `(query_shape, plan_order)`
//! tuples and exposes a rolling average. This is not a replacement for a full
//! optimized-plan cache; it memoizes measured runtime for plans the optimizer
//! has already tried.

use parking_lot::RwLock;
use proximadb_multimodel_query::QueryComponent;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Rolling-average measurement for one `(shape, order)` plan.
#[derive(Debug, Clone, Copy)]
struct PlanStats {
    /// Number of samples observed.
    samples: u64,
    /// Mean wall-clock time in microseconds.
    mean_us: f64,
    /// Sequence number for LRU eviction.
    last_touch: u64,
}

/// Bounded cache of measured plan execution times.
pub struct PlanExecutionCache {
    /// Stats keyed by `(shape_hash, plan_order_hash)`.
    inner: RwLock<HashMap<(u64, u64), PlanStats>>,
    /// Soft cap on entries before LRU eviction kicks in.
    max_entries: usize,
    /// Monotonic touch counter for LRU ordering.
    touch: AtomicU64,
    /// Total samples ever recorded.
    total_samples: AtomicU64,
    /// Total evictions.
    evictions: AtomicU64,
}

impl PlanExecutionCache {
    /// Create a new cache with the given soft entry cap.
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            max_entries,
            touch: AtomicU64::new(0),
            total_samples: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Record an observed wall-clock time for a `(shape, order)` plan.
    pub fn record(&self, shape: u64, order: &[usize], wall_time_us: u64) {
        let order_hash = hash_order(order);
        let key = (shape, order_hash);
        let touch_seq = self.touch.fetch_add(1, Ordering::Relaxed);
        let sample = wall_time_us as f64;

        let mut map = self.inner.write();

        if !map.contains_key(&key)
            && map.len() >= self.max_entries
            && let Some(lru_key) = map
                .iter()
                .min_by_key(|(_, stats)| stats.last_touch)
                .map(|(k, _)| *k)
        {
            map.remove(&lru_key);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }

        let entry = map.entry(key).or_insert(PlanStats {
            samples: 0,
            mean_us: 0.0,
            last_touch: touch_seq,
        });
        entry.samples += 1;
        entry.mean_us += (sample - entry.mean_us) / entry.samples as f64;
        entry.last_touch = touch_seq;

        self.total_samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Return the running mean wall-time for this `(shape, order)`.
    pub fn get_mean_us(&self, shape: u64, order: &[usize]) -> Option<f64> {
        let order_hash = hash_order(order);
        let key = (shape, order_hash);
        let map = self.inner.read();
        map.get(&key).filter(|s| s.samples > 0).map(|s| s.mean_us)
    }

    /// Number of distinct `(shape, order)` entries currently held.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Whether the cache holds any measurements.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Total samples recorded over the cache's lifetime.
    pub fn total_samples(&self) -> u64 {
        self.total_samples.load(Ordering::Relaxed)
    }

    /// Total LRU evictions over the cache's lifetime.
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// Drop all measurements.
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

/// Compute a stable shape hash for a sequence of query components.
pub fn shape_hash(components: &[QueryComponent]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (i, c) in components.iter().enumerate() {
        i.hash(&mut hasher);
        c.model.hash(&mut hasher);
        std::mem::discriminant(&c.operation).hash(&mut hasher);
        for dep in &c.dependencies {
            dep.component_index.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn hash_order(order: &[usize]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for idx in order {
        idx.hash(&mut hasher);
    }
    hasher.finish()
}

/// Convenience: lift a `PlanExecutionCache` into an `Arc` for sharing.
pub fn shared(max_entries: usize) -> Arc<PlanExecutionCache> {
    Arc::new(PlanExecutionCache::new(max_entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_multimodel_query::{
        ComponentDependency, DataModel, JoinType, ModelOperation, QueryComponent,
    };
    use proximadb_vector_query::{DistanceMetric, VectorSearchExpr, VectorSearchParams};

    fn vec_component(collection: &str, deps: Vec<usize>) -> QueryComponent {
        QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: collection.to_string(),
                query_vector: vec![],
                top_k: 10,
                threshold: None,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: deps
                .into_iter()
                .map(|d| ComponentDependency {
                    component_index: d,
                    join_field: "id".to_string(),
                    join_type: JoinType::Inner,
                })
                .collect(),
        }
    }

    #[test]
    fn shape_hash_ignores_collection_name() {
        let q1 = vec![vec_component("alpha", vec![])];
        let q2 = vec![vec_component("beta", vec![])];
        assert_eq!(shape_hash(&q1), shape_hash(&q2));
    }

    #[test]
    fn shape_hash_distinguishes_dependency_topology() {
        let q1 = vec![vec_component("a", vec![]), vec_component("b", vec![0])];
        let q2 = vec![vec_component("a", vec![]), vec_component("b", vec![])];
        assert_ne!(shape_hash(&q1), shape_hash(&q2));
    }

    #[test]
    fn record_and_retrieve_mean() {
        let cache = PlanExecutionCache::new(16);
        let shape = 42u64;
        let order = [0usize, 1, 2];

        assert!(cache.get_mean_us(shape, &order).is_none());
        cache.record(shape, &order, 1000);
        cache.record(shape, &order, 3000);
        cache.record(shape, &order, 2000);

        let mean = cache
            .get_mean_us(shape, &order)
            .expect("should have samples");
        assert_eq!(mean, 2000.0);
        assert_eq!(cache.total_samples(), 3);
    }

    #[test]
    fn lru_eviction_happens_at_capacity() {
        let cache = PlanExecutionCache::new(2);
        cache.record(1, &[0], 10);
        cache.record(2, &[0], 20);
        assert_eq!(cache.len(), 2);

        cache.record(3, &[0], 30);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.evictions(), 1);
    }
}
