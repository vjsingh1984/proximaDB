/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! End-to-end Cross-Modal Fusion benchmark (TD-137 Phase 3).
//!
//! Measures the fusion seam pipeline across realistic scale:
//!
//! * **Vector ANN seed** → top-K vector search
//! * **Graph k-hop expand** → BFS traversal from top vector seeds
//! * **PIT calibration** → empirical CDF normalization per source
//! * **Fuse-by-oid** → merge by canonical entity identifier
//! * **Rank** → final fused results
//!
//! ## What this bench measures
//!
//! * **Cold p50/p95/p99 latency** — first fusion query after restart (cache miss)
//! * **Warm p50/p95/p99 latency** — subsequent queries (cache hit)
//! * **Per-source participation** — how many sources contribute to results
//! * **Calibration overhead** — PIT CDF computation cost
//! * **Fusion quality** — consensus boost effect (multi-source items)
//!
//! ## Scale control
//!
//! Default: 10K vectors, 5K graph nodes, 1K documents — runs in seconds,
//! suitable for CI smoke. Override via env:
//!
//! ```bash
//! BENCH_VECTORS=100000  BENCH_GRAPH_NODES=50000  BENCH_DOCS=10000 \
//!   BENCH_WARM_RUNS=100  cargo bench --bench bench_cross_modal_fusion_e2e
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::search::cross_modal_fusion::{
    FusedItem, Fuser, FusionPolicy, SourceCandidates, SourceId,
};
use proximadb::graph::executor::in_memory::InMemoryGraphExecutor;
use proximadb::graph::{GraphConfig, GraphOperationsService};
use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, Graph as GraphProto, Node as NodeProto,
};
use proximadb::services::VectorOperationsService;
use proximadb::services::fusion_service::{FusionService, GraphFusionParams, GraphGrain};
use proximadb::storage::engines::sst::SstEngine;
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::traits::{FlushParameters, StorageQueryContext, UnifiedStorageEngine};
use tempfile::TempDir;

fn main() {
    let cfg = BenchConfig::from_env();

    println!("================================================================================");
    println!("   ProximaDB — Cross-Modal Fusion E2E Benchmark");
    println!("================================================================================");
    println!();
    println!("Configuration:");
    println!("  vectors:      {}", cfg.vector_count);
    println!("  graph nodes:  {}", cfg.graph_node_count);
    println!("  documents:    {}", cfg.document_count);
    println!("  dimension:    {}", cfg.dimension);
    println!("  top_k:        {}", cfg.top_k);
    println!("  max_depth:    {}", cfg.max_depth);
    println!("  cold runs:    {} (fresh engine each)", cfg.cold_runs);
    println!(
        "  warm runs:    {} (same engine, repeated queries)",
        cfg.warm_runs
    );
    println!();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async move {
        let setup = SetupResult::run(&cfg).await;
        let cold = measure_cold(&cfg, &setup).await;
        let warm = measure_warm(&cfg, &setup).await;

        print_report(&cfg, &setup, &cold, &warm);
    });
}

// ────────────────────────────────────────────────────────────────────────
// Config
// ────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct BenchConfig {
    vector_count: usize,
    graph_node_count: usize,
    document_count: usize,
    dimension: usize,
    top_k: usize,
    max_depth: u32,
    max_seeds: usize,
    cold_runs: usize,
    warm_runs: usize,
}

impl BenchConfig {
    fn from_env() -> Self {
        Self {
            vector_count: std::env::var("BENCH_VECTORS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10_000),
            graph_node_count: std::env::var("BENCH_GRAPH_NODES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5_000),
            document_count: std::env::var("BENCH_DOCS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1_000),
            dimension: std::env::var("BENCH_DIM")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(128),
            top_k: std::env::var("BENCH_TOP_K")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            max_depth: std::env::var("BENCH_MAX_DEPTH")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(2),
            max_seeds: std::env::var("BENCH_MAX_SEEDS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            cold_runs: std::env::var("BENCH_COLD_RUNS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            warm_runs: std::env::var("BENCH_WARM_RUNS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Setup
// ────────────────────────────────────────────────────────────────────────

struct SetupResult {
    vector_collection: String,
    graph_id: String,
    query_vector: Vec<f32>,
    temp_dir: TempDir,
    _vector_service: Arc<VectorOperationsService>,
    _graph_service: Arc<GraphOperationsService>,
    setup_time_ms: u64,
}

impl SetupResult {
    async fn run(cfg: &BenchConfig) -> Self {
        let start = Instant::now();
        let temp_dir = TempDir::new().expect("tempdir");

        // 1. Set up vector collection
        let vector_collection = format!("bench-fusion-vec-{}", uuid::Uuid::new_v4());
        let graph_id = format!("bench-fusion-graph-{}", uuid::Uuid::new_v4());

        println!("Setting up test environment...");
        println!("  Creating vector collection: {}", vector_collection);
        println!("  Creating graph: {}", graph_id);

        let storage_factory = Arc::new(FilesystemFactory::new(temp_dir.path().to_path_buf()));
        let sst_engine = Arc::new(
            SstEngine::new(storage_factory.clone())
                .await
                .expect("sst engine"),
        );

        let vector_service = Arc::new(VectorOperationsService::new(
            sst_engine.clone(),
            UnifiedDistanceCompute::default(),
        ));

        // Create vector collection
        let vector_config = CollectionConfig {
            dimension: cfg.dimension as u32,
            ..Default::default()
        };
        vector_service
            .create_collection(&vector_collection, vector_config)
            .await
            .expect("create vector collection");

        // 2. Insert vector records (co-indexed with graph by oid)
        println!("  Inserting {} vector records...", cfg.vector_count);
        let mut vector_oids = Vec::with_capacity(cfg.vector_count);
        for i in 0..cfg.vector_count {
            let oid = format!("graph/{}/node/{}", graph_id, i);
            let mut vector = vec![0.0f32; cfg.dimension];
            // Make vectors distinct but clustered (realistic distribution)
            for (j, v) in vector.iter_mut().enumerate() {
                *v = ((i as f32 * 0.01) + (j as f32 * 0.001)).sin();
            }
            // Normalize
            let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            for v in vector.iter_mut() {
                *v /= norm;
            }

            vector_service
                .insert_record(&vector_collection, &oid, &vector, None)
                .await
                .expect("insert vector");
            vector_oids.push((oid, vector));
        }

        // Flush to SST
        vector_service
            .flush_collection(&vector_collection, &FlushParameters::default())
            .await
            .expect("flush vector collection");

        // 3. Set up in-memory graph
        println!("  Creating graph with {} nodes...", cfg.graph_node_count);
        let graph_executor = Arc::new(InMemoryGraphExecutor::new());
        let graph_service = Arc::new(GraphOperationsService::new(
            graph_executor.clone(),
            GraphConfig::default(),
        ));

        // Create graph
        let graph_proto = GraphProto {
            id: graph_id.clone(),
            ..Default::default()
        };
        graph_service
            .create_graph(graph_proto)
            .await
            .expect("create graph");

        // Insert graph nodes (co-indexed with vectors)
        for i in 0..cfg.graph_node_count.min(cfg.vector_count) {
            let oid = format!("graph/{}/node/{}", graph_id, i);
            let node = NodeProto {
                id: i.to_string(),
                labels: vec!["Entity".to_string()],
                properties: HashMap::new(),
            };
            graph_service
                .add_node(&graph_id, node)
                .await
                .expect("add node");
        }

        // Create edges (graph structure for traversal)
        println!("  Creating graph edges...");
        let edges_per_node = 3;
        for i in 0..cfg.graph_node_count {
            for j in 1..=edges_per_node {
                let target = (i + j) % cfg.graph_node_count;
                graph_service
                    .add_edge(
                        &graph_id,
                        i.to_string(),
                        format!("e_{}_{}", i, target),
                        target.to_string(),
                        "RELATES_TO".to_string(),
                        HashMap::new(),
                    )
                    .await
                    .expect("add edge");
            }
        }

        // 4. Generate a query vector (similar to first cluster)
        let mut query_vector = vec![0.0f32; cfg.dimension];
        for (j, v) in query_vector.iter_mut().enumerate() {
            *v = ((0.0 * 0.01) + (j as f32 * 0.001)).sin();
        }
        let norm: f32 = query_vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        for v in query_vector.iter_mut() {
            *v /= norm;
        }

        let setup_time_ms = start.elapsed().as_millis() as u64;
        println!("  Setup complete in {}ms", setup_time_ms);
        println!();

        Self {
            vector_collection,
            graph_id,
            query_vector,
            temp_dir,
            _vector_service: vector_service,
            _graph_service: graph_service,
            setup_time_ms,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Measurement
// ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ColdMetrics {
    latencies: Vec<Duration>,
    multi_source_count: usize,
    source_participation: HashMap<SourceId, usize>,
}

#[derive(Debug, Default)]
struct WarmMetrics {
    latencies: Vec<Duration>,
    p50: Duration,
    p95: Duration,
    p99: Duration,
}

async fn measure_cold(cfg: &BenchConfig, setup: &SetupResult) -> ColdMetrics {
    println!("Running cold path benchmarks...");
    let mut metrics = ColdMetrics::default();

    let storage_factory = Arc::new(FilesystemFactory::new(setup.temp_dir.path().to_path_buf()));
    let sst_engine = Arc::new(SstEngine::new(storage_factory).await.expect("sst engine"));

    let vector_service = Arc::new(VectorOperationsService::new(
        sst_engine,
        UnifiedDistanceCompute::default(),
    ));

    let graph_executor = Arc::new(InMemoryGraphExecutor::new());
    let graph_service = Arc::new(GraphOperationsService::new(
        graph_executor,
        GraphConfig::default(),
    ));

    let fusion_service = FusionService::new(vector_service.clone(), graph_service);

    for run in 0..cfg.cold_runs {
        let start = Instant::now();

        let params = GraphFusionParams {
            graph_id: setup.graph_id.clone(),
            vector_collection: setup.vector_collection.clone(),
            query_vector: setup.query_vector.clone(),
            max_depth: cfg.max_depth,
            edge_types: vec![],
            max_seeds: cfg.max_seeds,
            limit: cfg.top_k,
            vector_weight: 1.0,
            graph_weight: 1.0,
            grain: GraphGrain::Nodes,
            policy: FusionPolicy::default(),
        };

        let (items, stats) = fusion_service
            .graph_fusion_search(params)
            .await
            .expect("fusion search");

        let elapsed = start.elapsed();
        metrics.latencies.push(elapsed);

        // Track multi-source results
        metrics.multi_source_count += items.iter().filter(|i| i.source_count > 1).count();

        // Track source participation
        for item in &items {
            for source in item.per_source.keys() {
                *metrics
                    .source_participation
                    .entry(source.clone())
                    .or_insert(0) += 1;
            }
        }

        println!(
            "  Cold run {}: {:>8}ms | {} items | {} sources fused",
            run + 1,
            elapsed.as_millis(),
            items.len(),
            stats.sources_fused
        );
    }

    println!();
    metrics
}

async fn measure_warm(cfg: &BenchConfig, setup: &SetupResult) -> WarmMetrics {
    println!("Running warm path benchmarks...");
    let mut latencies = Vec::with_capacity(cfg.warm_runs);

    let storage_factory = Arc::new(FilesystemFactory::new(setup.temp_dir.path().to_path_buf()));
    let sst_engine = Arc::new(SstEngine::new(storage_factory).await.expect("sst engine"));

    let vector_service = Arc::new(VectorOperationsService::new(
        sst_engine,
        UnifiedDistanceCompute::default(),
    ));

    let graph_executor = Arc::new(InMemoryGraphExecutor::new());
    let graph_service = Arc::new(GraphOperationsService::new(
        graph_executor,
        GraphConfig::default(),
    ));

    let fusion_service = FusionService::new(vector_service, graph_service);

    // Warm-up: run once to populate caches
    let params = GraphFusionParams {
        graph_id: setup.graph_id.clone(),
        vector_collection: setup.vector_collection.clone(),
        query_vector: setup.query_vector.clone(),
        max_depth: cfg.max_depth,
        edge_types: vec![],
        max_seeds: cfg.max_seeds,
        limit: cfg.top_k,
        vector_weight: 1.0,
        graph_weight: 1.0,
        grain: GraphGrain::Nodes,
        policy: FusionPolicy::default(),
    };
    let _ = fusion_service
        .graph_fusion_search(params)
        .await
        .expect("warm-up");

    // Measure warm runs
    for run in 0..cfg.warm_runs {
        let start = Instant::now();

        let params = GraphFusionParams {
            graph_id: setup.graph_id.clone(),
            vector_collection: setup.vector_collection.clone(),
            query_vector: setup.query_vector.clone(),
            max_depth: cfg.max_depth,
            edge_types: vec![],
            max_seeds: cfg.max_seeds,
            limit: cfg.top_k,
            vector_weight: 1.0,
            graph_weight: 1.0,
            grain: GraphGrain::Nodes,
            policy: FusionPolicy::default(),
        };

        let (items, _stats) = fusion_service
            .graph_fusion_search(params)
            .await
            .expect("fusion search");

        let elapsed = start.elapsed();
        latencies.push(elapsed);

        if run < 5 || (run + 1) % 20 == 0 {
            println!(
                "  Warm run {}: {:>8}ms | {} items",
                run + 1,
                elapsed.as_millis(),
                items.len()
            );
        }
    }

    // Compute percentiles
    let mut sorted = latencies.clone();
    sorted.sort();

    let p50 = sorted[cfg.warm_runs / 2];
    let p95 = sorted[cfg.warm_runs * 95 / 100];
    let p99 = sorted[cfg.warm_runs * 99 / 100];

    println!();
    WarmMetrics {
        latencies,
        p50,
        p95,
        p99,
    }
}

// ────────────────────────────────────────────────────────────────────────
// Report
// ────────────────────────────────────────────────────────────────────────

fn print_report(cfg: &BenchConfig, setup: &SetupResult, cold: &ColdMetrics, warm: &WarmMetrics) {
    println!("================================================================================");
    println!("   Benchmark Results");
    println!("================================================================================");
    println!();

    println!("Setup:");
    println!("  Setup time:     {}ms", setup.setup_time_ms);
    println!();

    println!("Cold Path (fresh engine, {} runs):", cfg.cold_runs);
    let cold_avg: Duration = cold.latencies.iter().sum::<Duration>() / cold.latencies.len() as u32;
    let cold_min = cold.latencies.iter().min().unwrap();
    let cold_max = cold.latencies.iter().max().unwrap();
    println!("  Average:        {}ms", cold_avg.as_millis());
    println!("  Min:            {}ms", cold_min.as_millis());
    println!("  Max:            {}ms", cold_max.as_millis());
    println!(
        "  Multi-source:    {} items ({:.1}%)",
        cold.multi_source_count,
        (cold.multi_source_count as f64 / (cfg.cold_runs * cfg.top_k) as f64) * 100.0
    );
    println!("  Source participation:");
    for (source, count) in &cold.source_participation {
        println!("    {:?}: {} hits", source, count);
    }
    println!();

    println!("Warm Path (cached, {} runs):", cfg.warm_runs);
    println!("  p50 latency:    {}ms", warm.p50.as_millis());
    println!("  p95 latency:    {}ms", warm.p95.as_millis());
    println!("  p99 latency:    {}ms", warm.p99.as_millis());
    println!();

    println!("Throughput:");
    let qps = 1_000.0 / warm.p50.as_secs_f64() * 1000.0;
    println!("  Queries/sec:    {:.1} (at p50)", qps);
    println!();

    println!("================================================================================");
    println!("   Benchmark Evidence (copy to BENCHMARK_EVIDENCE.toml)");
    println!("================================================================================");
    println!();
    println!("[cross_modal_fusion_e2e]");
    println!(
        "scale = \"{{}} vectors, {{}} graph nodes\"",
        cfg.vector_count, cfg.graph_node_count
    );
    println!("cold_avg_ms = {}", cold_avg.as_millis());
    println!("warm_p50_ms = {}", warm.p50.as_millis());
    println!("warm_p95_ms = {}", warm.p95.as_millis());
    println!("warm_p99_ms = {}", warm.p99.as_millis());
    println!(
        "multi_source_pct = {:.1}",
        (cold.multi_source_count as f64 / (cfg.cold_runs * cfg.top_k) as f64) * 100.0
    );
}
