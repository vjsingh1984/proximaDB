//! Plan execution cache for measured-time fitness in the evolutionary optimizer.
//!
//! Records observed wall-clock execution time for `(query_shape, plan_order)`
//! tuples and exposes a rolling average. The evolutionary optimizer (TD-047
//! sub A) uses this average as its fitness function when available, falling
//! back to the estimated cost when a plan has never been observed.
//!
//! This is *not* a replacement for the [`super::PlanCache`] LRU of full
//! optimized plans -- that cache memoizes the optimizer's *output*. This
//! cache memoizes the *measured runtime* of plans the optimizer has tried,
//! so subsequent runs converge toward what was empirically fastest rather
//! than what the cost model said would be fastest.
//!
//! ## Shape hashing
//!
//! The shape hash collapses a query into a fingerprint that is stable across
//! invocations with different concrete values: the sequence of
//! `(DataModel, ModelOperation discriminant)` per component plus the
//! dependency graph topology. Two queries that differ only in a literal
//! filter value will share a shape; two queries that differ in which models
//! are joined will not. This is the granularity at which it makes sense to
//! reuse runtime measurements.
//!
//! ## Recording
//!
//! Callers feed measurements via [`PlanExecutionCache::record`] after the
//! query executor finishes. The wiring of that call site is left to the
//! executor (TD-047 sub A wiring follow-up); shipping the cache and
//! optimizer integration first keeps the change reviewable.

use super::ast::{ModelOperation, QueryComponent};
use parking_lot::RwLock;
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
    /// Mean wall-clock time in microseconds (Welford-style update).
    mean_us: f64,
    /// Sequence number for LRU eviction (monotonically increasing on access).
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
    /// Total samples ever recorded (observability).
    total_samples: AtomicU64,
    /// Total evictions (observability).
    evictions: AtomicU64,
}

impl PlanExecutionCache {
    /// Create a new cache with the given soft entry cap.
    ///
    /// The cap is *soft*: the cache may briefly exceed it before LRU eviction
    /// reclaims space on the next write.
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
    ///
    /// Updates the running mean using Welford's algorithm so memory usage is
    /// O(1) per entry regardless of sample count.
    pub fn record(&self, shape: u64, order: &[usize], wall_time_us: u64) {
        let order_hash = hash_order(order);
        let key = (shape, order_hash);
        let touch_seq = self.touch.fetch_add(1, Ordering::Relaxed);
        let sample = wall_time_us as f64;

        let mut map = self.inner.write();

        // Soft eviction: if we'd push above the cap with a brand-new key,
        // drop the least-recently-touched entry first.
        if !map.contains_key(&key) && map.len() >= self.max_entries {
            if let Some(lru_key) = map
                .iter()
                .min_by_key(|(_, stats)| stats.last_touch)
                .map(|(k, _)| *k)
            {
                map.remove(&lru_key);
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        let entry = map.entry(key).or_insert(PlanStats {
            samples: 0,
            mean_us: 0.0,
            last_touch: touch_seq,
        });
        entry.samples += 1;
        // Welford update: mean += (x - mean) / n
        entry.mean_us += (sample - entry.mean_us) / entry.samples as f64;
        entry.last_touch = touch_seq;

        self.total_samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Return the running mean wall-time (microseconds) for this `(shape, order)`,
    /// or `None` if no samples have been observed yet.
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

    /// Drop all measurements. Useful in tests and after schema changes.
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

/// Compute a stable shape hash for a sequence of query components.
///
/// The hash captures the *structure* of the query (which models are involved,
/// which operation kind on each, and the dependency topology) but *not* the
/// concrete values (collection names, query vectors, filter literals). Two
/// invocations of "vector_search(collection=A, vec=v1) JOIN graph_traversal"
/// produce the same hash regardless of which collection or vector was used.
pub fn shape_hash(components: &[QueryComponent]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (i, c) in components.iter().enumerate() {
        i.hash(&mut hasher);
        // Hash the model discriminant (DataModel is hashable as it's an enum
        // re-export of StoreType).
        c.model.hash(&mut hasher);
        // Hash the operation discriminant. We deliberately do NOT hash the
        // operation payload (collection names, vectors, filters) -- those are
        // the per-query values that must NOT affect shape.
        std::mem::discriminant(&c.operation).hash(&mut hasher);
        // Hash the dependency graph topology: which earlier component each
        // dependency points at. We don't include the join field name because
        // join field is a property of the schema, not a runtime tuning knob
        // -- two queries on the same schema joining on the same models will
        // share their join field.
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

/// Convenience: lift a `PlanExecutionCache` into an `Arc` for sharing across
/// the optimizer + executor.
pub fn shared(max_entries: usize) -> Arc<PlanExecutionCache> {
    Arc::new(PlanExecutionCache::new(max_entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::unified::ast::{
        ComponentDependency, DataModel, DistanceMetric, JoinType, ModelOperation, QueryComponent,
        VectorSearchExpr, VectorSearchParams,
    };

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
        assert_eq!(
            shape_hash(&q1),
            shape_hash(&q2),
            "shape hash must collapse over collection literal"
        );
    }

    #[test]
    fn shape_hash_distinguishes_dependency_topology() {
        // Same models, but different dependency wiring should hash differently.
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
        // Welford should average to 2000.
        assert!((mean - 2000.0).abs() < 1e-6, "mean was {}", mean);
        assert_eq!(cache.total_samples(), 3);
    }

    #[test]
    fn different_orders_have_distinct_means() {
        let cache = PlanExecutionCache::new(16);
        let shape = 7u64;
        cache.record(shape, &[0, 1], 100);
        cache.record(shape, &[1, 0], 5000);

        assert_eq!(cache.get_mean_us(shape, &[0, 1]), Some(100.0));
        assert_eq!(cache.get_mean_us(shape, &[1, 0]), Some(5000.0));
    }

    #[test]
    fn lru_eviction_respects_cap() {
        let cache = PlanExecutionCache::new(2);

        // Fill to cap.
        cache.record(1, &[0], 100);
        cache.record(2, &[0], 200);

        // Touch shape 1 so shape 2 becomes the LRU candidate.
        let _ = cache.get_mean_us(1, &[0]);
        // get_mean_us is a read; LRU is by record-order in this impl, so
        // re-record shape 1 to refresh its touch position.
        cache.record(1, &[0], 100);

        // Push a third entry; soft cap forces eviction of the LRU (shape 2).
        cache.record(3, &[0], 300);

        assert!(cache.get_mean_us(3, &[0]).is_some(), "newest survives");
        assert!(
            cache.get_mean_us(1, &[0]).is_some(),
            "recently-touched survives"
        );
        assert!(cache.get_mean_us(2, &[0]).is_none(), "LRU evicted");
        assert_eq!(cache.evictions(), 1);
    }

    #[test]
    fn clear_drops_all_state() {
        let cache = PlanExecutionCache::new(8);
        cache.record(1, &[0], 100);
        cache.record(2, &[0], 200);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }
}
