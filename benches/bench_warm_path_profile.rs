/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 */

//! Warm-path profile bench — phase-by-phase breakdown of a single
//! search query.
//!
//! The e2e bench (`bench_vector_object_economy_e2e.rs`) measures
//! end-to-end warm latency but doesn't say WHERE the time goes. This
//! bench captures per-phase timings via the
//! `target: "sst_warm_phase"` tracing events emitted by
//! `SstEngine::fallback_to_direct_search` and reports per-phase
//! percentile distributions.
//!
//! Phases:
//!
//! * `discovery` — listing SSTable files (+ optional centroid prune)
//! * `per_file_scan` — sum across all files of the actual ANN scan
//! * `topk_merge` — bounded priority queue merge across file results
//! * `result_filter` — final include-vectors/include-metadata pass
//!
//! Run:
//!
//! ```bash
//! BENCH_VECTORS=100000 BENCH_DIM=768 BENCH_QUERIES=50 \
//!   cargo bench --bench bench_warm_path_profile
//! ```

use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::search::{SearchMode, SearchParams};
use proximadb::proto::proximadb_v1::{Collection, CollectionConfig};
use proximadb::storage::engines::sst::SstEngine;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::traits::{FlushParameters, StorageQueryContext, UnifiedStorageEngine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tempfile::TempDir;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;

// ────────────────────────────────────────────────────────────────────────
// Custom layer: capture `sst_warm_phase` events into a sample store
// ────────────────────────────────────────────────────────────────────────

/// Per-phase samples in microseconds. Keyed by the `phase` field on
/// the tracing event (e.g. "discovery", "per_file_scan").
#[derive(Default)]
struct PhaseStore {
    samples: HashMap<String, Vec<u64>>,
}

impl PhaseStore {
    fn record(&mut self, phase: String, elapsed_us: u64) {
        self.samples.entry(phase).or_default().push(elapsed_us);
    }

    fn drain_and_collect(&mut self) -> HashMap<String, Vec<u64>> {
        std::mem::take(&mut self.samples)
    }
}

/// Tracing layer that captures events emitted under the
/// `sst_warm_phase` target. Extracts the `phase` (str) and
/// `elapsed_us` (u64) fields and stores them.
struct WarmPhaseLayer {
    store: Arc<Mutex<PhaseStore>>,
}

/// Visitor that pulls `phase` + `elapsed_us` out of an event.
struct PhaseVisitor {
    phase: Option<String>,
    elapsed_us: Option<u64>,
}

impl tracing::field::Visit for PhaseVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "phase" {
            self.phase = Some(value.to_string());
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "elapsed_us" {
            self.elapsed_us = Some(value);
        }
    }

    // The remaining types are required by the trait but unused for
    // this bench. tracing's `info!(phase = "x")` literals become
    // `record_str`; integer literals become `record_u64` (or i64).
    fn record_debug(&mut self, _f: &tracing::field::Field, _v: &dyn std::fmt::Debug) {}
    fn record_i64(&mut self, _f: &tracing::field::Field, _v: i64) {}
    fn record_bool(&mut self, _f: &tracing::field::Field, _v: bool) {}
}

impl<S> Layer<S> for WarmPhaseLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sst_warm_phase" {
            return;
        }
        let mut visitor = PhaseVisitor {
            phase: None,
            elapsed_us: None,
        };
        event.record(&mut visitor);
        if let (Some(phase), Some(elapsed_us)) = (visitor.phase, visitor.elapsed_us)
            && let Ok(mut store) = self.store.lock()
        {
            store.record(phase, elapsed_us);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Bench config (mirrors the e2e bench)
// ────────────────────────────────────────────────────────────────────────

struct BenchConfig {
    vector_count: usize,
    dimension: usize,
    top_k: usize,
    warm_runs: usize,
    pre_warm_runs: usize,
    /// When true, use `SearchMode::Approximate { nprobe: None }`
    /// (auto-calc sqrt). Default false (Exact) preserves the
    /// baseline behaviour the e2e bench measures.
    approx_mode: bool,
    /// When true, sets `SearchParams::enable_vectorized_execution`,
    /// routing the SST reader through its SIMD vectorized path
    /// (TD-041). Default false uses the scalar path.
    vectorized: bool,
    /// When true, construct an `AxisManager` with default HNSW
    /// config, register it globally so the SstEngine picks it up,
    /// and insert all records into AXIS after flush. This routes
    /// queries through `execute_orchestrated_search` →
    /// `axis_manager.query(...)` (HNSW) instead of the linear
    /// fallback. Setup time grows by the HNSW build cost
    /// (~O(n log n) NEON distance computes for build).
    axis: bool,
}

impl BenchConfig {
    fn from_env() -> Self {
        Self {
            vector_count: env_usize("BENCH_VECTORS", 100_000),
            dimension: env_usize("BENCH_DIM", 768),
            top_k: env_usize("BENCH_TOPK", 10),
            warm_runs: env_usize("BENCH_QUERIES", 50),
            // One pre-warm pass to populate caches the same way the
            // e2e bench does — turbopuffer-style "warm" is reached
            // after the first query.
            pre_warm_runs: 1,
            approx_mode: env_bool("BENCH_APPROX", false),
            vectorized: env_bool("BENCH_VECTORIZED", false),
            axis: env_bool("BENCH_AXIS", false),
        }
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ────────────────────────────────────────────────────────────────────────
// Setup
// ────────────────────────────────────────────────────────────────────────

struct SetupResult {
    warm_engine: Arc<SstEngine>,
    warm_collection: Arc<Collection>,
    insert_ms: u128,
    flush_ms: u128,
    /// Time spent building the AXIS HNSW index post-flush, or 0
    /// when `BENCH_AXIS` is not set.
    axis_build_ms: u128,
    _temp_dir: TempDir,
}

impl SetupResult {
    async fn run(cfg: &BenchConfig) -> Self {
        let temp_dir = TempDir::new().expect("tempdir");
        let collection = make_collection(&temp_dir, cfg.dimension);
        let vectors = synthetic_records(&collection.id, cfg.vector_count, cfg.dimension);

        // Slice B: when BENCH_AXIS is on, build the AxisManager and
        // register it as the SstEngine's global *before* engine
        // construction so `get_sst_axis_manager()` picks it up
        // during `SstEngine::new_with_config`. AXIS construction is
        // expensive enough (background tasks) that we want it once
        // per bench run, not per-iteration.
        if cfg.axis {
            // OnceLock — safe to call once per process. If a prior
            // bench iteration set this already, the second set is a
            // no-op and we live with whichever instance won.
            let axis_manager = std::sync::Arc::new(
                proximadb::index::AxisManager::new(
                    proximadb::index::AxisConfig::default(),
                )
                .await
                .expect("axis manager"),
            );
            proximadb::storage::engines::sst::core::set_sst_axis_manager(axis_manager);
        }

        let insert_start = Instant::now();
        let fs = Arc::new(
            FilesystemFactory::create_default()
                .await
                .expect("filesystem factory"),
        );
        let dist = Arc::new(UnifiedDistanceCompute::default());
        let engine = SstEngine::new_with_config(Default::default(), fs.clone(), dist.clone())
            .await
            .expect("sst engine");
        let flush_start = Instant::now();
        let insert_ms = (flush_start - insert_start).as_millis();

        let params = FlushParameters {
            collection_id: Some(collection.id.clone()),
            collection_config: Some(collection.clone()),
            vector_records: vectors.clone(),
            force: true,
            synchronous: true,
            ..Default::default()
        };
        engine.flush(params).await.expect("flush");
        let flush_ms = flush_start.elapsed().as_millis();

        // Slice B: build the HNSW after flush so the AXIS index
        // mirrors the on-disk data. The engine's
        // `execute_orchestrated_search` will route warm queries
        // through `axis_manager.query(...)` once records are
        // indexed.
        let axis_build_ms = if cfg.axis {
            let build_start = Instant::now();
            if let Some(axis_manager) =
                proximadb::storage::engines::sst::core::get_sst_axis_manager()
            {
                for record in &vectors {
                    axis_manager
                        .insert_record(&collection.id, record)
                        .await
                        .expect("axis insert");
                }
            }
            build_start.elapsed().as_millis()
        } else {
            0
        };

        Self {
            warm_engine: Arc::new(engine),
            warm_collection: Arc::new(collection),
            insert_ms,
            flush_ms,
            axis_build_ms,
            _temp_dir: temp_dir,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Main
// ────────────────────────────────────────────────────────────────────────

fn main() {
    let cfg = BenchConfig::from_env();

    // Install the warm-phase capturing layer BEFORE setup so any
    // events emitted during setup (we don't expect any, but doesn't
    // hurt) are still in scope. We reset the store after setup to
    // ensure only timed queries contribute to the report.
    let store = Arc::new(Mutex::new(PhaseStore::default()));
    let layer = WarmPhaseLayer {
        store: store.clone(),
    };
    Registry::default().with(layer).init();

    println!("================================================================================");
    println!("   ProximaDB — Warm-Path Profile (per-phase breakdown)");
    println!("================================================================================");
    println!();
    println!("Configuration:");
    println!("  vectors:    {}", cfg.vector_count);
    println!("  dimension:  {}", cfg.dimension);
    println!("  top_k:      {}", cfg.top_k);
    println!("  warm_runs:  {}", cfg.warm_runs);
    println!(
        "  mode:       {}",
        if cfg.approx_mode {
            "Approximate (nprobe=auto)"
        } else {
            "Exact (default)"
        }
    );
    println!(
        "  vectorized: {} (enable_vectorized_execution)",
        cfg.vectorized
    );
    println!("  axis_hnsw:  {} (BENCH_AXIS — orchestrated path)", cfg.axis);
    println!();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async move {
        let setup = SetupResult::run(&cfg).await;
        if cfg.axis {
            println!(
                "Setup: insert {} ms, flush {} ms, axis_build {} ms",
                setup.insert_ms, setup.flush_ms, setup.axis_build_ms
            );
        } else {
            println!(
                "Setup: insert {} ms, flush {} ms",
                setup.insert_ms, setup.flush_ms
            );
        }

        // Pre-warm + reset
        for _ in 0..cfg.pre_warm_runs {
            run_one_query(&setup, &cfg).await;
        }
        if let Ok(mut s) = store.lock() {
            s.drain_and_collect();
        }

        println!();
        println!("📊 Capturing per-phase timings across {} warm queries", cfg.warm_runs);

        let mut total_us = Vec::with_capacity(cfg.warm_runs);
        for _ in 0..cfg.warm_runs {
            let t = run_one_query(&setup, &cfg).await;
            total_us.push(t);
        }

        let captured = store.lock().unwrap().drain_and_collect();
        print_report(&cfg, &total_us, &captured);
    });
}

async fn run_one_query(setup: &SetupResult, cfg: &BenchConfig) -> u64 {
    let query_vector = synthetic_query(cfg.dimension);
    let search_mode = if cfg.approx_mode {
        SearchMode::Approximate { nprobe: None }
    } else {
        SearchMode::Exact
    };
    let search_params = Arc::new(SearchParams {
        vector: Some(query_vector),
        top_k: Some(cfg.top_k),
        search_mode,
        enable_vectorized_execution: if cfg.vectorized { Some(true) } else { None },
        ..Default::default()
    });
    let ctx = StorageQueryContext::new(search_params, Arc::clone(&setup.warm_collection));
    let start = Instant::now();
    let _ = setup
        .warm_engine
        .search_vectors_unified(&ctx)
        .await
        .expect("search");
    start.elapsed().as_micros() as u64
}

// ────────────────────────────────────────────────────────────────────────
// Report
// ────────────────────────────────────────────────────────────────────────

fn percentile(samples: &[u64], p: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u64> = samples.to_vec();
    sorted.sort_unstable();
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn mean(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    (samples.iter().sum::<u64>() / samples.len() as u64) as u64
}

fn print_report(cfg: &BenchConfig, total_us: &[u64], captured: &HashMap<String, Vec<u64>>) {
    println!();
    println!("================================================================================");
    println!("   Results");
    println!("================================================================================");
    println!();
    println!(
        "Total query latency (warm, includes all phases + outer dispatch):"
    );
    print_dist("  total", total_us);
    println!();

    // Print captured phases in a consistent order matching the
    // search pipeline. Unknown phases (added later by source code)
    // appear at the end.
    let canonical = ["discovery", "per_file_scan", "topk_merge", "result_filter"];
    println!("Per-phase breakdown (microseconds, {} samples / phase):", cfg.warm_runs);
    println!(
        "  {:<18} {:>10} {:>10} {:>10} {:>10} {:>8}",
        "phase", "mean", "p50", "p99", "max", "% of mean total"
    );
    let total_mean = mean(total_us);

    for name in canonical.iter() {
        if let Some(samples) = captured.get(*name) {
            let m = mean(samples);
            let p50 = percentile(samples, 50.0);
            let p99 = percentile(samples, 99.0);
            let max = *samples.iter().max().unwrap_or(&0);
            let pct = if total_mean > 0 {
                (m as f64 / total_mean as f64) * 100.0
            } else {
                0.0
            };
            println!(
                "  {:<18} {:>10} {:>10} {:>10} {:>10} {:>7.1}%",
                name, m, p50, p99, max, pct
            );
        } else {
            println!("  {:<18} {:>10}", name, "(no samples)");
        }
    }
    // Any extra phases not in the canonical list
    for (name, samples) in captured.iter() {
        if canonical.iter().any(|c| *c == name.as_str()) {
            continue;
        }
        let m = mean(samples);
        let p50 = percentile(samples, 50.0);
        let p99 = percentile(samples, 99.0);
        let max = *samples.iter().max().unwrap_or(&0);
        let pct = if total_mean > 0 {
            (m as f64 / total_mean as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  {:<18} {:>10} {:>10} {:>10} {:>10} {:>7.1}%",
            name, m, p50, p99, max, pct
        );
    }

    // Sum of phase means vs total mean — the gap is overhead
    // outside instrumented phases (collection lookup, query-vector
    // validation, dispatch, etc.).
    let phase_total_mean: u64 = canonical
        .iter()
        .filter_map(|n| captured.get(*n).map(|s| mean(s)))
        .sum();
    let gap = total_mean.saturating_sub(phase_total_mean);
    println!();
    println!(
        "Outside-instrumented overhead:  {} us  ({:.1}% of mean total)",
        gap,
        if total_mean > 0 {
            (gap as f64 / total_mean as f64) * 100.0
        } else {
            0.0
        }
    );
}

fn print_dist(label: &str, samples: &[u64]) {
    println!(
        "  {:<18}  mean={} us  p50={} us  p99={} us  max={} us",
        label,
        mean(samples),
        percentile(samples, 50.0),
        percentile(samples, 99.0),
        samples.iter().max().copied().unwrap_or(0),
    );
}

// ────────────────────────────────────────────────────────────────────────
// Synthetic data (mirrors e2e bench)
// ────────────────────────────────────────────────────────────────────────

fn make_collection(temp_dir: &TempDir, dim: usize) -> Collection {
    Collection {
        id: "bench_warm_profile".to_string(),
        config: Some(CollectionConfig {
            dimension: dim as u32,
            distance_metric: Some(1), // Cosine
            ..Default::default()
        }),
        storage_assignment: Some(proximadb::proto::proximadb_v1::StorageAssignment {
            base_location: temp_dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn synthetic_records(
    collection_id: &str,
    count: usize,
    dim: usize,
) -> Vec<proximadb_records::ProximaRecord> {
    (0..count)
        .map(|i| proximadb_records::ProximaRecord {
            oid: format!("{}_{:08}", collection_id, i),
            embeddings: vec![proximadb_records::EmbeddingCell {
                model_id: "default".to_string(),
                modality: "vector".to_string(),
                dim: dim as u32,
                values: proximadb_records::EmbeddingValues::Fp32(
                    (0..dim).map(|j| ((i + j) as f32 * 0.001).sin()).collect(),
                ),
                ..Default::default()
            }],
            props: proximadb_records::ProximaTree::new(),
            record_version: 1,
            created_at_ns: 0,
            updated_at_ns: 0,
            ..Default::default()
        })
        .collect()
}

fn synthetic_query(dim: usize) -> Vec<f32> {
    (0..dim).map(|j| (j as f32 * 0.001).sin() + 0.01).collect()
}
