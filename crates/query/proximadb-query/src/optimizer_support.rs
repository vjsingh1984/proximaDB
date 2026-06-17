//! Shared support structures for the unified query optimizer.

use parking_lot::RwLock;
use proximadb_multimodel_query::{DataModel, ModelOperation, MultiModelQuery, QueryComponent};
use proximadb_query_filter::FilterOperator;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::info;

/// Configuration for the query optimizer.
#[derive(Debug, Clone)]
pub struct OptimizerConfig {
    /// Enable selectivity-based reordering.
    pub enable_reordering: bool,
    /// Enable filter pushdown.
    pub enable_filter_pushdown: bool,
    /// Minimum selectivity to consider reordering.
    pub min_selectivity_threshold: f64,
    /// Maximum number of components to reorder.
    pub max_reorder_components: usize,
    /// Enable query plan caching.
    pub enable_plan_cache: bool,
    /// Plan cache TTL in seconds.
    pub plan_cache_ttl_secs: u64,
    /// Enable evolutionary optimizer for complex queries.
    pub enable_evolutionary_optimizer: bool,
    /// Population size for evolutionary optimizer.
    pub evolutionary_population_size: usize,
    /// Number of generations for evolutionary optimizer.
    pub evolutionary_generations: usize,
    /// Use measured wall-clock time as evolutionary fitness when available.
    pub enable_measured_fitness: bool,
    /// Soft cap on measured-fitness cache entries.
    pub measured_fitness_max_entries: usize,
    /// Configurable fallback heuristics used when catalog/statistics are absent.
    pub selectivity_policy: SelectivityPolicy,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            enable_reordering: true,
            enable_filter_pushdown: true,
            min_selectivity_threshold: 0.1,
            max_reorder_components: 10,
            enable_plan_cache: true,
            plan_cache_ttl_secs: 300,
            enable_evolutionary_optimizer: false,
            evolutionary_population_size: 20,
            evolutionary_generations: 5,
            enable_measured_fitness: false,
            measured_fitness_max_entries: 1024,
            selectivity_policy: SelectivityPolicy::default(),
        }
    }
}

impl OptimizerConfig {
    /// Validate optimizer configuration ranges before installing it in a runtime.
    pub fn validate(&self) -> Result<(), String> {
        validate_unit_interval("min_selectivity_threshold", self.min_selectivity_threshold)?;
        self.selectivity_policy.validate()
    }
}

/// Configurable selectivity heuristics for planner fallback paths.
///
/// These values are only used when catalog statistics or measured history are not
/// available. Keeping them in a policy object makes the production path explicit
/// and avoids hidden tuning constants in planner branches.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectivityPolicy {
    pub eq: f64,
    pub ne: f64,
    pub range: f64,
    pub in_list: f64,
    pub not_in: f64,
    pub contains: f64,
    pub starts_with: f64,
    pub ends_with: f64,
    pub exists: f64,
    pub type_match: f64,
    pub graph_label: f64,
    pub graph_filter: f64,
    pub graph_from_component: f64,
    pub graph_edge_type_multiplier: f64,
    pub graph_node_filter_multiplier: f64,
    pub log_severity_multiplier: f64,
    pub log_service_multiplier: f64,
    pub log_text_multiplier: f64,
    pub metric_name_multiplier: f64,
    pub metric_label_multiplier: f64,
}

impl Default for SelectivityPolicy {
    fn default() -> Self {
        Self {
            eq: 0.1,
            ne: 0.9,
            range: 0.5,
            in_list: 0.2,
            not_in: 0.8,
            contains: 0.3,
            starts_with: 0.2,
            ends_with: 0.3,
            exists: 0.8,
            type_match: 0.5,
            graph_label: 0.1,
            graph_filter: 0.05,
            graph_from_component: 0.01,
            graph_edge_type_multiplier: 0.3,
            graph_node_filter_multiplier: 0.5,
            log_severity_multiplier: 0.2,
            log_service_multiplier: 0.1,
            log_text_multiplier: 0.1,
            metric_name_multiplier: 0.01,
            metric_label_multiplier: 0.5,
        }
    }
}

impl SelectivityPolicy {
    /// Validate all selectivity factors as finite unit-interval probabilities.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("eq", self.eq),
            ("ne", self.ne),
            ("range", self.range),
            ("in_list", self.in_list),
            ("not_in", self.not_in),
            ("contains", self.contains),
            ("starts_with", self.starts_with),
            ("ends_with", self.ends_with),
            ("exists", self.exists),
            ("type_match", self.type_match),
            ("graph_label", self.graph_label),
            ("graph_filter", self.graph_filter),
            ("graph_from_component", self.graph_from_component),
            (
                "graph_edge_type_multiplier",
                self.graph_edge_type_multiplier,
            ),
            (
                "graph_node_filter_multiplier",
                self.graph_node_filter_multiplier,
            ),
            ("log_severity_multiplier", self.log_severity_multiplier),
            ("log_service_multiplier", self.log_service_multiplier),
            ("log_text_multiplier", self.log_text_multiplier),
            ("metric_name_multiplier", self.metric_name_multiplier),
            ("metric_label_multiplier", self.metric_label_multiplier),
        ] {
            validate_unit_interval(name, value)?;
        }

        Ok(())
    }
}

fn validate_unit_interval(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!(
            "{name} must be finite and between 0.0 and 1.0, got {value}"
        ));
    }
    Ok(())
}

/// Selectivity estimate for a query component.
#[derive(Debug, Clone)]
pub struct SelectivityEstimate {
    /// Estimated selectivity (0.0 - 1.0).
    pub selectivity: f64,
    /// Confidence in the estimate (0.0 - 1.0).
    pub confidence: f64,
    /// Estimated result count.
    pub estimated_rows: u64,
    /// Estimation method used.
    pub method: EstimationMethod,
}

/// Method used for selectivity estimation.
#[derive(Debug, Clone, PartialEq)]
pub enum EstimationMethod {
    /// Based on collected statistics.
    Statistics,
    /// Based on filter predicates.
    PredicateAnalysis,
    /// Based on historical query results.
    Historical,
    /// Default heuristic estimate.
    Heuristic,
}

/// Optimized query plan.
#[derive(Debug, Clone)]
pub struct OptimizedPlan {
    /// Reordered components.
    pub components: Vec<QueryComponent>,
    /// Component execution order.
    pub execution_order: Vec<usize>,
    /// Selectivity estimates per component.
    pub selectivity_estimates: Vec<SelectivityEstimate>,
    /// Pushed down filters per component.
    pub pushed_filters: Vec<Vec<PushedFilter>>,
    /// Estimated total cost.
    pub estimated_cost: f64,
    /// Optimization notes.
    pub notes: Vec<String>,
}

/// A filter that has been pushed down to an engine.
#[derive(Debug, Clone)]
pub struct PushedFilter {
    /// Field/path the filter applies to.
    pub field: String,
    /// Filter operator.
    pub operator: FilterOperator,
    /// Filter value as string.
    pub value: String,
    /// Target engine.
    pub target: DataModel,
}

/// Query statistics collector.
pub struct QueryStatistics {
    /// Collection statistics.
    collection_stats: RwLock<HashMap<String, OptimizerCollectionStats>>,
    /// Query execution history.
    query_history: RwLock<Vec<QueryHistoryEntry>>,
    /// Total queries executed.
    total_queries: AtomicU64,
    /// Total execution time.
    total_execution_time_us: AtomicU64,
}

/// Optimizer-local collection statistics with column-level detail.
#[derive(Debug, Clone)]
pub struct OptimizerCollectionStats {
    /// Collection name.
    pub name: String,
    /// Estimated row count.
    pub row_count: u64,
    /// Average row size in bytes.
    pub avg_row_size: u64,
    /// Distinct value counts for indexed columns.
    pub distinct_counts: HashMap<String, u64>,
    /// Last updated timestamp.
    pub last_updated: u64,
}

/// Historical query execution entry.
#[derive(Debug, Clone)]
pub struct QueryHistoryEntry {
    /// Query hash (for matching similar queries).
    pub query_hash: u64,
    /// Actual result count.
    pub result_count: u64,
    /// Execution time in microseconds.
    pub execution_time_us: u64,
    /// Timestamp.
    pub timestamp: u64,
}

impl QueryStatistics {
    /// Create new statistics collector.
    pub fn new() -> Self {
        Self {
            collection_stats: RwLock::new(HashMap::new()),
            query_history: RwLock::new(Vec::new()),
            total_queries: AtomicU64::new(0),
            total_execution_time_us: AtomicU64::new(0),
        }
    }

    /// Get collection statistics.
    pub fn get_collection_stats(&self, collection: &str) -> Option<OptimizerCollectionStats> {
        self.collection_stats.read().get(collection).cloned()
    }

    /// Update collection statistics.
    pub fn update_collection_stats(&self, stats: OptimizerCollectionStats) {
        let name = stats.name.clone();
        self.collection_stats.write().insert(name, stats);
    }

    /// Record query execution.
    pub fn record_query(&self, hash: u64, result_count: u64, execution_time_us: u64) {
        self.total_queries.fetch_add(1, Ordering::Relaxed);
        self.total_execution_time_us
            .fetch_add(execution_time_us, Ordering::Relaxed);

        let entry = QueryHistoryEntry {
            query_hash: hash,
            result_count,
            execution_time_us,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        let mut history = self.query_history.write();
        history.push(entry);
        if history.len() > 10000 {
            history.remove(0);
        }
    }

    /// Get average execution time for similar queries.
    pub fn get_avg_execution_time(&self, query_hash: u64) -> Option<u64> {
        let history = self.query_history.read();
        let matching: Vec<&QueryHistoryEntry> = history
            .iter()
            .filter(|e| e.query_hash == query_hash)
            .collect();

        if matching.is_empty() {
            return None;
        }

        let total: u64 = matching.iter().map(|e| e.execution_time_us).sum();
        Some(total / matching.len() as u64)
    }

    /// Get total query count.
    pub fn total_queries(&self) -> u64 {
        self.total_queries.load(Ordering::Relaxed)
    }

    /// Get average execution time across all queries.
    pub fn avg_execution_time_us(&self) -> u64 {
        let total = self.total_queries.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_execution_time_us.load(Ordering::Relaxed) / total
    }
}

impl Default for QueryStatistics {
    fn default() -> Self {
        Self::new()
    }
}

/// LRU cache for query plans with TTL expiration.
///
/// Thin wrapper over `proximadb_runtime_common::ThreadSafeLruCache` — the
/// horizontal-layer primitive that provides O(1) LRU (via doubly-linked
/// list, not the O(n) scan this used to do), TTL eviction, and atomic
/// hit/miss/eviction counters. Wrapping rather than re-exporting keeps the
/// existing public API (`get`/`insert`/`invalidate_all`/`invalidate_collection`/
/// `stats`) and the `PlanCacheStats` shape stable for downstream callers.
///
/// Previously this file shipped a hand-rolled LRU + TTL + stats (~150 lines)
/// that was strictly worse than the shared primitive — the LRU was an O(n)
/// scan per insert and the eviction loop walked the map twice.
pub struct PlanCache {
    inner: proximadb_runtime_common::ThreadSafeLruCache<u64, OptimizedPlan>,
    max_entries: usize,
}

impl PlanCache {
    /// Create a new plan cache.
    pub fn new(max_entries: usize, ttl_secs: u64) -> Self {
        Self {
            inner: proximadb_runtime_common::ThreadSafeLruCache::with_ttl(
                max_entries,
                Duration::from_secs(ttl_secs),
            ),
            max_entries,
        }
    }

    /// Get a cached plan if it exists and hasn't expired.
    pub fn get(&self, query_hash: u64) -> Option<OptimizedPlan> {
        self.inner.get(&query_hash)
    }

    /// Insert a plan into the cache.
    pub fn insert(&self, query_hash: u64, plan: OptimizedPlan) {
        self.inner.put(query_hash, plan);
    }

    /// Invalidate all cached plans.
    pub fn invalidate_all(&self) {
        let size_before = self.inner.len();
        self.inner.clear();
        info!("Invalidated {} cached query plans", size_before);
    }

    /// Invalidate cached plans for a specific collection.
    ///
    /// Currently a coarse `clear()` — the primitive doesn't expose a
    /// predicate-based eviction, and the prior implementation did the same
    /// thing (this method was already a `clear` in disguise).
    pub fn invalidate_collection(&self, _collection: &str) {
        self.invalidate_all();
    }

    /// Get cache statistics.
    pub fn stats(&self) -> PlanCacheStats {
        match self.inner.stats() {
            Some(s) => PlanCacheStats {
                size: s.size,
                max_entries: self.max_entries,
                hits: s.hits,
                misses: s.misses,
                evictions: s.evictions + s.expirations,
            },
            None => PlanCacheStats {
                size: 0,
                max_entries: self.max_entries,
                hits: 0,
                misses: 0,
                evictions: 0,
            },
        }
    }
}

/// Statistics for the plan cache.
///
/// Part of the external API surface — appears in optimizer observability
/// (EXPLAIN output, query-planner metrics). Do NOT consolidate with
/// `proximadb_runtime_common::cache::CacheStats` without bumping the
/// public API version.
#[derive(Debug, Clone)]
pub struct PlanCacheStats {
    /// Current number of cached plans.
    pub size: usize,
    /// Maximum cache capacity.
    pub max_entries: usize,
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of evictions.
    pub evictions: u64,
}

impl PlanCacheStats {
    /// Calculate cache hit rate (0.0 - 1.0).
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }
}

/// Compute a hash for a query to use as a cache key.
pub fn compute_query_hash(query: &MultiModelQuery) -> u64 {
    let mut hasher = DefaultHasher::new();

    query.components.len().hash(&mut hasher);

    for component in &query.components {
        std::mem::discriminant(&component.model).hash(&mut hasher);

        match &component.operation {
            ModelOperation::VectorSearch(expr) => {
                "VectorSearch".hash(&mut hasher);
                expr.collection.hash(&mut hasher);
                expr.top_k.hash(&mut hasher);
                if let Some(t) = expr.threshold {
                    t.to_bits().hash(&mut hasher);
                }
                std::mem::discriminant(&expr.metric).hash(&mut hasher);
            }
            ModelOperation::DocumentQuery(expr) => {
                "DocumentQuery".hash(&mut hasher);
                expr.collection.hash(&mut hasher);
                expr.path_filters.len().hash(&mut hasher);
                for filter in &expr.path_filters {
                    filter.path.hash(&mut hasher);
                    std::mem::discriminant(&filter.operator).hash(&mut hasher);
                }
            }
            ModelOperation::GraphQuery(expr) => {
                "GraphQuery".hash(&mut hasher);
                expr.graph_name.hash(&mut hasher);
                expr.normalized_query.hash(&mut hasher);
                expr.max_depth.hash(&mut hasher);
                expr.uses_legacy_node_rows.hash(&mut hasher);
                expr.output_columns.len().hash(&mut hasher);
                for column in &expr.output_columns {
                    column.hash(&mut hasher);
                }
            }
            ModelOperation::GraphTraversal(expr) => {
                "GraphTraversal".hash(&mut hasher);
                expr.graph_name.hash(&mut hasher);
                expr.max_depth.hash(&mut hasher);
                expr.edge_types.len().hash(&mut hasher);
                for edge_type in &expr.edge_types {
                    edge_type.hash(&mut hasher);
                }
            }
            ModelOperation::LogQuery(expr) => {
                "LogQuery".hash(&mut hasher);
                expr.namespace.hash(&mut hasher);
                expr.start_time_ns.hash(&mut hasher);
                expr.end_time_ns.hash(&mut hasher);
                expr.services.len().hash(&mut hasher);
            }
            ModelOperation::MetricQuery(expr) => {
                "MetricQuery".hash(&mut hasher);
                expr.namespace.hash(&mut hasher);
                expr.metric_name.hash(&mut hasher);
                expr.start_time_ns.hash(&mut hasher);
                expr.end_time_ns.hash(&mut hasher);
            }
        }

        component.dependencies.len().hash(&mut hasher);
        for dep in &component.dependencies {
            dep.component_index.hash(&mut hasher);
            dep.join_field.hash(&mut hasher);
        }
    }

    std::mem::discriminant(&query.fusion_strategy).hash(&mut hasher);
    query.limit.hash(&mut hasher);
    query.offset.hash(&mut hasher);

    hasher.finish()
}

/// Fusion strategy for combining results from multi-model sub-queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusionStrategy {
    /// Reciprocal Rank Fusion.
    Rrf,
    /// Intersection.
    Intersection,
    /// Union.
    Union,
    /// Weighted sum.
    WeightedSum,
}

/// Select the optimal fusion strategy based on query characteristics.
pub fn select_fusion_strategy(
    has_vector_component: bool,
    has_graph_component: bool,
    filter_selectivity: f64,
) -> FusionStrategy {
    if has_vector_component && has_graph_component {
        FusionStrategy::Rrf
    } else if filter_selectivity < 0.1 {
        FusionStrategy::Intersection
    } else {
        FusionStrategy::Union
    }
}

// =========== Phase 4: Convergence access-path cost rules ===========
//
// These functions encode the four cost trade-offs from the convergence spec
// (Phase 4 §4.3): the planner calls them to decide which physical access path
// to use for graph, document, full-text, and variation-projection queries.
// Each function returns an `AccessPathCostEstimate` that the EXPLAIN output
// surfaces to the user.

/// Physical access path choices available to the convergence query planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessPath {
    /// Full canonical scan over `ProximaRecord` storage (always available).
    CanonicalScan,
    /// Relational adjacency table index lookup for graph traversal.
    AdjacencyTable,
    /// In-memory CSR projection for read-heavy graph traversal.
    CsrProjection,
    /// JSON-path index (`$.field` lookup) for document filtering.
    JsonPathIndex,
    /// Inverted full-text index for text search predicates.
    FullTextIndex,
    /// Columnar variation projection for field subset queries.
    ColumnarProjection,
}

/// Cost estimate for a chosen access path, returned by the convergence cost rules.
#[derive(Debug, Clone)]
pub struct AccessPathCostEstimate {
    /// Chosen access path.
    pub path: AccessPath,
    /// Relative cost unit (lower is better; comparable only within the same rule).
    pub cost: f64,
    /// Estimated output selectivity (0.0 – 1.0).
    pub selectivity: f64,
    /// Human-readable reason for the choice — surfaces in EXPLAIN output.
    pub reason: String,
}

/// Cost rule 1: Adjacency table vs CSR projection for graph traversal.
///
/// Choose `CsrProjection` when the graph is read-heavy and the CSR epoch is
/// fresh enough to be trusted. Fall back to `AdjacencyTable` (canonical row
/// lookup) when the workload has high write frequency or the CSR is stale.
///
/// Parameters:
/// - `csr_epoch_age_secs`: seconds since the CSR was last rebuilt.
/// - `write_ops_per_min`: recent write rate on this graph's edge set.
/// - `expected_hop_fan_out`: average out-degree (neighbours per node).
pub fn graph_traversal_access_path(
    csr_epoch_age_secs: u64,
    write_ops_per_min: u64,
    expected_hop_fan_out: f64,
) -> AccessPathCostEstimate {
    // Heuristic thresholds (tunable via statistics later).
    const MAX_STALE_SECS: u64 = 300; // 5-minute freshness window
    const MAX_WRITE_RATE: u64 = 10; // writes/min above which CSR is unreliable

    let csr_is_fresh = csr_epoch_age_secs < MAX_STALE_SECS;
    let workload_is_read_heavy = write_ops_per_min < MAX_WRITE_RATE;

    if csr_is_fresh && workload_is_read_heavy {
        // CSR O(1) neighbour fetch; significantly cheaper than row scan for fan-out > 5.
        let cost = 1.0 + expected_hop_fan_out * 0.1;
        AccessPathCostEstimate {
            path: AccessPath::CsrProjection,
            cost,
            selectivity: 1.0,
            reason: format!(
                "CSR projection chosen: epoch age {csr_epoch_age_secs}s < {MAX_STALE_SECS}s, \
                 write rate {write_ops_per_min}/min < {MAX_WRITE_RATE}/min"
            ),
        }
    } else {
        // Adjacency table row lookup: O(degree) index scan, but always consistent.
        let cost = 2.0 + expected_hop_fan_out * 0.5;
        let reason = if !csr_is_fresh {
            format!("Adjacency table chosen: CSR stale ({csr_epoch_age_secs}s ≥ {MAX_STALE_SECS}s)")
        } else {
            format!(
                "Adjacency table chosen: write-heavy workload ({write_ops_per_min}/min ≥ {MAX_WRITE_RATE}/min)"
            )
        };
        AccessPathCostEstimate {
            path: AccessPath::AdjacencyTable,
            cost,
            selectivity: 1.0,
            reason,
        }
    }
}

/// Cost rule 2: JSON path index vs full row scan for document field filtering.
///
/// Choose `JsonPathIndex` when a path index exists and the predicate is
/// selective enough to make the index cheaper than a full scan. Fall back to
/// `CanonicalScan` (full row scan with inline JSON parsing) otherwise.
///
/// Parameters:
/// - `has_path_index`: whether a `$.field` path index exists for this predicate.
/// - `predicate_selectivity`: fraction of rows estimated to match (0.0 – 1.0).
/// - `collection_size`: number of records in the collection.
pub fn document_filter_access_path(
    has_path_index: bool,
    predicate_selectivity: f64,
    collection_size: u64,
) -> AccessPathCostEstimate {
    const INDEX_OVERHEAD: f64 = 10.0; // fixed cost to use the index (B-tree traversal)
    const JSON_PARSE_COST: f64 = 5.0; // per-row JSON parsing cost relative to row read

    let selectivity = predicate_selectivity.clamp(0.0, 1.0);

    if has_path_index {
        let index_cost = INDEX_OVERHEAD + selectivity * collection_size as f64;
        let scan_cost = collection_size as f64 * (1.0 + JSON_PARSE_COST * 0.1);

        if index_cost < scan_cost {
            return AccessPathCostEstimate {
                path: AccessPath::JsonPathIndex,
                cost: index_cost,
                selectivity,
                reason: format!(
                    "JSON path index chosen: index_cost={index_cost:.1} < scan_cost={scan_cost:.1} \
                     (selectivity={selectivity:.3}, n={collection_size})"
                ),
            };
        }
    }

    let scan_cost = collection_size as f64 * (1.0 + JSON_PARSE_COST * selectivity);
    AccessPathCostEstimate {
        path: AccessPath::CanonicalScan,
        cost: scan_cost,
        selectivity,
        reason: if has_path_index {
            format!(
                "Canonical scan chosen: index not selective enough \
                 (selectivity={selectivity:.3} → scan cheaper)"
            )
        } else {
            "Canonical scan chosen: no JSON path index available".to_string()
        },
    }
}

/// Cost rule 3: Full-text index vs row scan for text search predicates.
///
/// Choose `FullTextIndex` when an inverted index exists and the term is
/// selective (uncommon). Fall back to `CanonicalScan` with LIKE for high-
/// frequency terms or when no full-text index is available.
///
/// Parameters:
/// - `has_fulltext_index`: whether an inverted text index is available.
/// - `term_selectivity`: fraction of documents containing the search term (0.0 – 1.0).
/// - `collection_size`: total number of documents.
pub fn fulltext_access_path(
    has_fulltext_index: bool,
    term_selectivity: f64,
    collection_size: u64,
) -> AccessPathCostEstimate {
    const FTS_OVERHEAD: f64 = 20.0; // posting-list decode + merge overhead
    const LIKE_SCAN_FACTOR: f64 = 8.0; // per-row regex/pattern match cost
    // Above this selectivity, the posting list is so large that merging it is
    // more expensive than a sequential LIKE scan.
    const MAX_FTS_SELECTIVITY: f64 = 0.5;

    let selectivity = term_selectivity.clamp(0.0, 1.0);

    if has_fulltext_index && selectivity < MAX_FTS_SELECTIVITY {
        let fts_cost = FTS_OVERHEAD + selectivity * collection_size as f64;
        let scan_cost = collection_size as f64 * LIKE_SCAN_FACTOR;

        if fts_cost < scan_cost {
            return AccessPathCostEstimate {
                path: AccessPath::FullTextIndex,
                cost: fts_cost,
                selectivity,
                reason: format!(
                    "Full-text index chosen: fts_cost={fts_cost:.1} < scan_cost={scan_cost:.1} \
                     (term_selectivity={selectivity:.3} < {MAX_FTS_SELECTIVITY})"
                ),
            };
        }
    }

    let scan_cost = collection_size as f64 * LIKE_SCAN_FACTOR;
    AccessPathCostEstimate {
        path: AccessPath::CanonicalScan,
        cost: scan_cost,
        selectivity,
        reason: if has_fulltext_index && selectivity >= MAX_FTS_SELECTIVITY {
            format!(
                "Canonical scan chosen: high-frequency term (selectivity={selectivity:.2} ≥ {MAX_FTS_SELECTIVITY}), \
                 posting list merge more expensive than sequential scan"
            )
        } else if has_fulltext_index {
            "Canonical scan chosen: small collection, FTS overhead not amortised".to_string()
        } else {
            "Canonical scan chosen: no full-text index available".to_string()
        },
    }
}

/// Cost rule 4: Columnar variation projection vs JSON payload scan.
///
/// Choose `ColumnarProjection` when a columnar physical layout exists for the
/// requested fields and the output is a small fraction of the document size.
/// Fall back to `CanonicalScan` (deserialize full JSON, project in memory)
/// when fewer fields are stored columnarly or the document is small enough
/// that the deserialization overhead is negligible.
///
/// Parameters:
/// - `has_columnar`: whether columnar physical layout is available for these fields.
/// - `projected_field_count`: number of fields to project.
/// - `total_field_count`: total fields in the document schema.
/// - `avg_doc_size_bytes`: average serialized document size.
pub fn variation_projection_access_path(
    has_columnar: bool,
    projected_field_count: usize,
    total_field_count: usize,
    avg_doc_size_bytes: usize,
) -> AccessPathCostEstimate {
    const COLUMNAR_OVERHEAD: f64 = 5.0; // per-query columnar reader setup
    const JSON_DESER_PER_BYTE: f64 = 0.001; // relative JSON deserialization cost

    let projection_ratio = if total_field_count > 0 {
        projected_field_count as f64 / total_field_count as f64
    } else {
        1.0
    };

    if has_columnar && projection_ratio < 0.5 {
        let columnar_cost = COLUMNAR_OVERHEAD + projected_field_count as f64 * 1.0;
        AccessPathCostEstimate {
            path: AccessPath::ColumnarProjection,
            cost: columnar_cost,
            selectivity: projection_ratio,
            reason: format!(
                "Columnar projection chosen: projecting {projected_field_count}/{total_field_count} fields \
                 (ratio={projection_ratio:.2} < 0.5)"
            ),
        }
    } else {
        let json_cost =
            avg_doc_size_bytes as f64 * JSON_DESER_PER_BYTE + projected_field_count as f64 * 0.5;
        AccessPathCostEstimate {
            path: AccessPath::CanonicalScan,
            cost: json_cost,
            selectivity: projection_ratio,
            reason: if has_columnar {
                format!(
                    "Canonical scan chosen: projecting {projected_field_count}/{total_field_count} fields \
                     (ratio={projection_ratio:.2} ≥ 0.5, JSON deserialization cheaper than columnar overhead)"
                )
            } else {
                "Canonical scan chosen: no columnar physical layout available".to_string()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// CSR auto-materialization threshold function (Phase 6)
// ---------------------------------------------------------------------------

/// Inputs used by the CSR auto-materialization decision function.
#[derive(Debug, Clone)]
pub struct CsrMaterializationInput {
    /// Number of nodes in the graph.
    pub graph_size_nodes: u64,
    /// Recent write rate on this graph's edge set (writes/min).
    pub write_ops_per_min: u64,
    /// Average out-degree (edges per source node).
    pub avg_out_degree: f64,
    /// How many times per minute the graph is traversed by read queries.
    pub query_repetitions_per_min: u64,
    /// Seconds since the CSR was last fully materialised.
    pub csr_epoch_age_secs: u64,
}

/// Why the auto-materialization decision was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsrMaterializeTrigger {
    /// Graph is large enough that CSR sequential access pays off.
    GraphSizeThreshold,
    /// Queries repeat frequently — build cost amortized across many reads.
    HighQueryRepetition,
    /// Write rate is low enough that CSR stays fresh for its full TTL.
    LowWriteRate,
    /// CSR is already fresh — skip rebuild.
    AlreadyFresh,
    /// Write rate too high; CSR would go stale before being useful.
    HighWriteRatePreventsUse,
    /// Graph is small enough that the adjacency table is cheaper to scan.
    GraphTooSmall,
    /// Not enough query repetition to amortize the materialization cost.
    InsufficientQueryLoad,
}

/// Decision produced by `csr_auto_materialize_decision`.
#[derive(Debug, Clone)]
pub struct CsrMaterializationDecision {
    /// Whether to (re)materialise the CSR projection.
    pub should_materialize: bool,
    /// Primary factor that drove the decision.
    pub trigger: CsrMaterializeTrigger,
    /// Estimated time (ms) to rebuild the CSR from the adjacency table.
    pub estimated_build_cost_ms: f64,
    /// Estimated factor by which CSR reduces per-hop traversal latency vs
    /// adjacency table row lookups.
    pub estimated_traversal_speedup: f64,
    /// Human-readable explanation (surfaced in EXPLAIN / observability output).
    pub reason: String,
}

/// Decide whether to (re)materialise the CSR projection for a graph.
///
/// Thresholds are informed by Phase 6 benchmark findings:
/// - CSR is worthwhile for graphs ≥ 100 k nodes with read-heavy workloads.
/// - At write rates > 10/min the CSR epoch expires before amortizing build cost.
/// - Traversal speedup scales with out-degree; high-fan-out graphs benefit most.
/// - Materialization is always skipped when the epoch is still within the
///   freshness window (`csr_epoch_age_secs < MAX_STALE_SECS`).
pub fn csr_auto_materialize_decision(
    input: &CsrMaterializationInput,
) -> CsrMaterializationDecision {
    // Threshold constants. All durations in seconds; rates in ops/min.
    const SMALL_GRAPH_THRESHOLD: u64 = 10_000; // ≤ 10 k nodes → adjacency table fast enough
    const LARGE_GRAPH_THRESHOLD: u64 = 100_000; // ≥ 100 k nodes → CSR speedup justifies build
    const MAX_WRITE_RATE_FOR_CSR: u64 = 10; // writes/min above which CSR staleness outweighs gain
    const MIN_QUERY_REPETITIONS: u64 = 5; // reads/min needed to amortize build cost
    const HIGH_DEGREE_THRESHOLD: f64 = 20.0; // avg out-degree above which sequential CSR wins
    const CSR_FRESHNESS_WINDOW_SECS: u64 = 300; // 5-minute freshness window (aligns with graph_traversal_access_path)

    // Estimated build cost: ~0.01 ms per node (dominated by CSR index sort).
    let estimated_build_cost_ms = input.graph_size_nodes as f64 * 0.01;

    // Per-hop speedup: sequential CSR access vs. random adjacency table lookup.
    // Empirically ~2× for low-degree, ~8× for high-degree (cache-line effects).
    let estimated_traversal_speedup = if input.avg_out_degree >= HIGH_DEGREE_THRESHOLD {
        8.0
    } else if input.avg_out_degree >= 5.0 {
        3.5
    } else {
        1.5
    };

    // Fast path: CSR already fresh — skip rebuild.
    if input.csr_epoch_age_secs < CSR_FRESHNESS_WINDOW_SECS {
        return CsrMaterializationDecision {
            should_materialize: false,
            trigger: CsrMaterializeTrigger::AlreadyFresh,
            estimated_build_cost_ms,
            estimated_traversal_speedup,
            reason: format!(
                "CSR epoch is {age}s old (< {window}s freshness window); no rebuild needed",
                age = input.csr_epoch_age_secs,
                window = CSR_FRESHNESS_WINDOW_SECS,
            ),
        };
    }

    // High write rate: CSR would go stale too fast.
    if input.write_ops_per_min > MAX_WRITE_RATE_FOR_CSR {
        return CsrMaterializationDecision {
            should_materialize: false,
            trigger: CsrMaterializeTrigger::HighWriteRatePreventsUse,
            estimated_build_cost_ms,
            estimated_traversal_speedup,
            reason: format!(
                "Write rate {}/min exceeds threshold ({}/min); CSR would stale before amortizing build cost {:.1}ms",
                input.write_ops_per_min, MAX_WRITE_RATE_FOR_CSR, estimated_build_cost_ms,
            ),
        };
    }

    // Small graph: adjacency table scan is sufficient.
    if input.graph_size_nodes < SMALL_GRAPH_THRESHOLD {
        return CsrMaterializationDecision {
            should_materialize: false,
            trigger: CsrMaterializeTrigger::GraphTooSmall,
            estimated_build_cost_ms,
            estimated_traversal_speedup,
            reason: format!(
                "Graph has {} nodes (< {} threshold); adjacency table random access \
                 comparable to sequential CSR at this scale",
                input.graph_size_nodes, SMALL_GRAPH_THRESHOLD,
            ),
        };
    }

    // Large graph: strongly prefer CSR regardless of query load.
    if input.graph_size_nodes >= LARGE_GRAPH_THRESHOLD {
        return CsrMaterializationDecision {
            should_materialize: true,
            trigger: CsrMaterializeTrigger::GraphSizeThreshold,
            estimated_build_cost_ms,
            estimated_traversal_speedup,
            reason: format!(
                "Graph has {} nodes (≥ {} threshold); CSR sequential access gives \
                 {:.1}× speedup at avg degree {:.1}, build cost {:.1}ms",
                input.graph_size_nodes,
                LARGE_GRAPH_THRESHOLD,
                estimated_traversal_speedup,
                input.avg_out_degree,
                estimated_build_cost_ms,
            ),
        };
    }

    // Mid-size graph: materialize only when query load justifies it.
    if input.query_repetitions_per_min >= MIN_QUERY_REPETITIONS {
        return CsrMaterializationDecision {
            should_materialize: true,
            trigger: CsrMaterializeTrigger::HighQueryRepetition,
            estimated_build_cost_ms,
            estimated_traversal_speedup,
            reason: format!(
                "{} queries/min (≥ {} threshold) amortizes {:.1}ms build cost; \
                 CSR yields {:.1}× speedup at avg degree {:.1}",
                input.query_repetitions_per_min,
                MIN_QUERY_REPETITIONS,
                estimated_build_cost_ms,
                estimated_traversal_speedup,
                input.avg_out_degree,
            ),
        };
    }

    // Mid-size, low query load: skip CSR.
    CsrMaterializationDecision {
        should_materialize: false,
        trigger: CsrMaterializeTrigger::InsufficientQueryLoad,
        estimated_build_cost_ms,
        estimated_traversal_speedup,
        reason: format!(
            "{} queries/min (< {} threshold) insufficient to amortize {:.1}ms build cost \
             for a {} node graph; use adjacency table",
            input.query_repetitions_per_min,
            MIN_QUERY_REPETITIONS,
            estimated_build_cost_ms,
            input.graph_size_nodes,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_document_query::PathFilter;
    use proximadb_multimodel_query::{ComponentDependency, JoinType, QueryComponent};
    use proximadb_query_filter::{FilterOperator, FilterValue};
    use proximadb_vector_query::{DistanceMetric, VectorSearchExpr, VectorSearchParams};

    fn make_vector_search_component(threshold: Option<f32>, top_k: u32) -> QueryComponent {
        QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "embeddings".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k,
                threshold,
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        }
    }

    #[test]
    fn query_statistics_records_history() {
        let stats = QueryStatistics::new();
        stats.record_query(42, 10, 1_000);
        stats.record_query(42, 20, 3_000);
        assert_eq!(stats.total_queries(), 2);
        assert_eq!(stats.avg_execution_time_us(), 2_000);
        assert_eq!(stats.get_avg_execution_time(42), Some(2_000));
    }

    #[test]
    fn plan_cache_stores_and_reports_stats() {
        let cache = PlanCache::new(100, 300);
        let plan = OptimizedPlan {
            components: vec![make_vector_search_component(None, 10)],
            execution_order: vec![0],
            selectivity_estimates: vec![SelectivityEstimate {
                selectivity: 0.1,
                confidence: 0.5,
                estimated_rows: 100,
                method: EstimationMethod::PredicateAnalysis,
            }],
            pushed_filters: vec![vec![]],
            estimated_cost: 1.0,
            notes: vec!["test".to_string()],
        };

        cache.insert(123, plan.clone());
        assert!(cache.get(123).is_some());
        let stats = cache.stats();
        assert_eq!(stats.size, 1);
        assert_eq!(stats.hits, 1);
    }

    #[test]
    fn compute_query_hash_is_stable_for_same_query_shape() {
        let query = MultiModelQuery {
            components: vec![make_vector_search_component(Some(0.7), 5)],
            fusion_strategy: proximadb_query_fusion::FusionStrategy::Intersection,
            limit: Some(10),
            offset: None,
            projection: vec![],
            order_by: None,
        };

        assert_eq!(compute_query_hash(&query), compute_query_hash(&query));
    }

    #[test]
    fn select_fusion_strategy_prefers_rrf_for_vector_and_graph() {
        assert_eq!(select_fusion_strategy(true, true, 0.5), FusionStrategy::Rrf);
        assert_eq!(
            select_fusion_strategy(true, false, 0.05),
            FusionStrategy::Intersection
        );
        assert_eq!(
            select_fusion_strategy(false, false, 0.5),
            FusionStrategy::Union
        );
    }

    #[test]
    fn path_filter_hashing_inputs_compile() {
        let _filter = PathFilter {
            path: "category".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("electronics".to_string()),
        };
        let _dep = ComponentDependency {
            component_index: 0,
            join_field: "id".to_string(),
            join_type: JoinType::Inner,
        };
    }

    // ── Phase 4: convergence cost rule tests ─────────────────────────────────

    #[test]
    fn graph_traversal_prefers_csr_when_fresh_and_read_heavy() {
        let est = graph_traversal_access_path(60, 2, 5.0);
        assert_eq!(est.path, AccessPath::CsrProjection);
        assert!(est.reason.contains("CSR projection chosen"));
    }

    #[test]
    fn graph_traversal_falls_back_to_adjacency_when_stale() {
        let est = graph_traversal_access_path(600, 2, 5.0);
        assert_eq!(est.path, AccessPath::AdjacencyTable);
        assert!(est.reason.contains("stale"));
    }

    #[test]
    fn graph_traversal_falls_back_to_adjacency_when_write_heavy() {
        let est = graph_traversal_access_path(60, 50, 5.0);
        assert_eq!(est.path, AccessPath::AdjacencyTable);
        assert!(est.reason.contains("write-heavy"));
    }

    #[test]
    fn document_filter_prefers_path_index_when_selective() {
        // 0.1% selectivity on 1M rows → index is cheaper
        let est = document_filter_access_path(true, 0.001, 1_000_000);
        assert_eq!(est.path, AccessPath::JsonPathIndex);
        assert!(est.reason.contains("JSON path index chosen"));
    }

    #[test]
    fn document_filter_falls_back_to_scan_when_no_index() {
        let est = document_filter_access_path(false, 0.001, 100_000);
        assert_eq!(est.path, AccessPath::CanonicalScan);
        assert!(est.reason.contains("no JSON path index"));
    }

    #[test]
    fn document_filter_falls_back_to_scan_when_not_selective() {
        // 80% selectivity → scan is cheaper than index overhead
        let est = document_filter_access_path(true, 0.8, 10);
        assert_eq!(est.path, AccessPath::CanonicalScan);
        assert!(est.reason.contains("not selective enough"));
    }

    #[test]
    fn fulltext_prefers_index_for_rare_terms() {
        // 0.1% term frequency on 1M docs → FTS is cheaper
        let est = fulltext_access_path(true, 0.001, 1_000_000);
        assert_eq!(est.path, AccessPath::FullTextIndex);
        assert!(est.reason.contains("Full-text index chosen"));
    }

    #[test]
    fn fulltext_falls_back_to_scan_for_common_terms() {
        // 90% term frequency → posting list is nearly the whole collection, scan wins
        let est = fulltext_access_path(true, 0.9, 1_000_000);
        assert_eq!(est.path, AccessPath::CanonicalScan);
        assert!(est.reason.contains("high-frequency term"));
    }

    #[test]
    fn fulltext_falls_back_to_scan_without_index() {
        let est = fulltext_access_path(false, 0.001, 1_000_000);
        assert_eq!(est.path, AccessPath::CanonicalScan);
        assert!(est.reason.contains("no full-text index"));
    }

    #[test]
    fn variation_prefers_columnar_for_sparse_projection() {
        // Projecting 2 of 20 fields (10%) with columnar available
        let est = variation_projection_access_path(true, 2, 20, 4096);
        assert_eq!(est.path, AccessPath::ColumnarProjection);
        assert!(est.reason.contains("Columnar projection chosen"));
    }

    #[test]
    fn variation_falls_back_to_scan_for_dense_projection() {
        // Projecting 15 of 20 fields (75%) → JSON deserialization cheaper
        let est = variation_projection_access_path(true, 15, 20, 512);
        assert_eq!(est.path, AccessPath::CanonicalScan);
        assert!(est.reason.contains("ratio"));
    }

    #[test]
    fn variation_falls_back_to_scan_without_columnar() {
        let est = variation_projection_access_path(false, 2, 20, 4096);
        assert_eq!(est.path, AccessPath::CanonicalScan);
        assert!(est.reason.contains("no columnar physical layout"));
    }

    #[test]
    fn cost_rule_reasons_are_non_empty() {
        let rules = [
            graph_traversal_access_path(60, 2, 5.0),
            graph_traversal_access_path(600, 50, 5.0),
            document_filter_access_path(true, 0.001, 1_000_000),
            document_filter_access_path(false, 0.5, 100),
            fulltext_access_path(true, 0.001, 1_000_000),
            fulltext_access_path(false, 0.5, 100),
            variation_projection_access_path(true, 2, 20, 4096),
            variation_projection_access_path(false, 5, 10, 512),
        ];
        for est in &rules {
            assert!(
                !est.reason.is_empty(),
                "reason must not be empty: {:?}",
                est.path
            );
            assert!(est.cost > 0.0, "cost must be positive: {:?}", est.path);
        }
    }

    // ── CSR auto-materialization decision ─────────────────────────────────

    fn fresh_input() -> CsrMaterializationInput {
        CsrMaterializationInput {
            graph_size_nodes: 500_000,
            write_ops_per_min: 2,
            avg_out_degree: 25.0,
            query_repetitions_per_min: 20,
            csr_epoch_age_secs: 600,
        }
    }

    #[test]
    fn csr_already_fresh_skips_rebuild() {
        let mut input = fresh_input();
        input.csr_epoch_age_secs = 60; // well within 5-min window
        let d = csr_auto_materialize_decision(&input);
        assert!(!d.should_materialize);
        assert_eq!(d.trigger, CsrMaterializeTrigger::AlreadyFresh);
    }

    #[test]
    fn csr_high_write_rate_prevents_materialization() {
        let mut input = fresh_input();
        input.write_ops_per_min = 50; // above 10/min threshold
        let d = csr_auto_materialize_decision(&input);
        assert!(!d.should_materialize);
        assert_eq!(d.trigger, CsrMaterializeTrigger::HighWriteRatePreventsUse);
    }

    #[test]
    fn csr_small_graph_uses_adjacency_table() {
        let mut input = fresh_input();
        input.graph_size_nodes = 5_000; // below 10 k threshold
        let d = csr_auto_materialize_decision(&input);
        assert!(!d.should_materialize);
        assert_eq!(d.trigger, CsrMaterializeTrigger::GraphTooSmall);
    }

    #[test]
    fn csr_large_graph_always_materializes() {
        let mut input = fresh_input();
        input.graph_size_nodes = 2_000_000; // above 100 k threshold
        input.query_repetitions_per_min = 0; // even with no queries
        let d = csr_auto_materialize_decision(&input);
        assert!(d.should_materialize);
        assert_eq!(d.trigger, CsrMaterializeTrigger::GraphSizeThreshold);
    }

    #[test]
    fn csr_mid_graph_high_query_load_materializes() {
        let input = CsrMaterializationInput {
            graph_size_nodes: 50_000, // mid-range
            write_ops_per_min: 3,
            avg_out_degree: 10.0,
            query_repetitions_per_min: 15, // above min threshold
            csr_epoch_age_secs: 600,
        };
        let d = csr_auto_materialize_decision(&input);
        assert!(d.should_materialize);
        assert_eq!(d.trigger, CsrMaterializeTrigger::HighQueryRepetition);
    }

    #[test]
    fn csr_mid_graph_low_query_load_skips() {
        let input = CsrMaterializationInput {
            graph_size_nodes: 50_000,
            write_ops_per_min: 3,
            avg_out_degree: 10.0,
            query_repetitions_per_min: 1, // below 5/min threshold
            csr_epoch_age_secs: 600,
        };
        let d = csr_auto_materialize_decision(&input);
        assert!(!d.should_materialize);
        assert_eq!(d.trigger, CsrMaterializeTrigger::InsufficientQueryLoad);
    }

    #[test]
    fn csr_high_degree_graph_gets_higher_speedup_estimate() {
        let low_degree = csr_auto_materialize_decision(&CsrMaterializationInput {
            graph_size_nodes: 200_000,
            write_ops_per_min: 1,
            avg_out_degree: 2.0,
            query_repetitions_per_min: 10,
            csr_epoch_age_secs: 600,
        });
        let high_degree = csr_auto_materialize_decision(&CsrMaterializationInput {
            graph_size_nodes: 200_000,
            write_ops_per_min: 1,
            avg_out_degree: 50.0,
            query_repetitions_per_min: 10,
            csr_epoch_age_secs: 600,
        });
        assert!(high_degree.estimated_traversal_speedup > low_degree.estimated_traversal_speedup);
    }

    #[test]
    fn csr_decision_reason_is_non_empty_for_all_triggers() {
        let cases = vec![
            CsrMaterializationInput {
                csr_epoch_age_secs: 60,
                graph_size_nodes: 200_000,
                write_ops_per_min: 2,
                avg_out_degree: 5.0,
                query_repetitions_per_min: 10,
            },
            CsrMaterializationInput {
                csr_epoch_age_secs: 600,
                graph_size_nodes: 200_000,
                write_ops_per_min: 50,
                avg_out_degree: 5.0,
                query_repetitions_per_min: 10,
            },
            CsrMaterializationInput {
                csr_epoch_age_secs: 600,
                graph_size_nodes: 500,
                write_ops_per_min: 2,
                avg_out_degree: 5.0,
                query_repetitions_per_min: 10,
            },
            CsrMaterializationInput {
                csr_epoch_age_secs: 600,
                graph_size_nodes: 2_000_000,
                write_ops_per_min: 2,
                avg_out_degree: 5.0,
                query_repetitions_per_min: 0,
            },
            CsrMaterializationInput {
                csr_epoch_age_secs: 600,
                graph_size_nodes: 50_000,
                write_ops_per_min: 2,
                avg_out_degree: 5.0,
                query_repetitions_per_min: 15,
            },
            CsrMaterializationInput {
                csr_epoch_age_secs: 600,
                graph_size_nodes: 50_000,
                write_ops_per_min: 2,
                avg_out_degree: 5.0,
                query_repetitions_per_min: 1,
            },
        ];
        for input in &cases {
            let d = csr_auto_materialize_decision(input);
            assert!(
                !d.reason.is_empty(),
                "reason empty for trigger {:?}",
                d.trigger
            );
            assert!(d.estimated_build_cost_ms >= 0.0);
            assert!(d.estimated_traversal_speedup >= 1.0);
        }
    }
}
