// ANN-Benchmarks runner for ProximaDB
//
// Entry point for running standardized ANN benchmarks.
// Usage:
//   cargo run --bin ann-benchmarks -- --dataset sift --algorithm hnsw --runs 100

use std::path::PathBuf;
use std::process;

use anyhow::Result;
use proximadb::bench::{ANNBenchmarksConfig, ANNBenchmarksRunner, BuildParams, SearchParams};

fn main() -> Result<()> {
    let args = parse_args();

    println!("ProximaDB ANN-Benchmarks Runner");
    println!("================================");
    println!("Dataset: {}", args.dataset);
    println!("Algorithm: {}", args.algorithm);
    println!("K: {}", args.k);
    println!("Runs: {}", args.runs);
    println!();

    let config = ANNBenchmarkConfig {
        dataset: args.dataset.clone(),
        algorithm: args.algorithm.clone(),
        dataset_path: args.dataset_path,
        k: args.k,
        runs: args.runs,
        build_params: args.build_params,
        search_params: args.search_params,
    };

    let runner = ANNBenchmarksRunner::new(config);

    println!("Building index...");
    let results = runner.run()?;

    println!("\n=== Benchmark Results ===");
    println!("Index build time: {:.2}s", results.build_time_secs);
    println!("Index size: {:.2} MB", results.index_size_bytes as f64 / 1024.0 / 1024.0);
    println!("Memory usage: {:.2} MB", results.memory_usage_bytes as f64 / 1024.0 / 1024.0);
    println!();

    println!("Query Performance:");
    println!("  Average QPS: {:.2}", results.query_stats.avg_qps);
    println!("  Median QPS: {:.2}", results.query_stats.median_qps);
    println!("  P95 QPS: {:.2}", results.query_stats.p95_qps);
    println!("  P99 QPS: {:.2}", results.query_stats.p99_qps);
    println!();

    println!("Accuracy:");
    println!("  Recall@{}: {:.2}%", results.query_stats.num_queries, results.query_stats.recall_at_k * 100.0);
    println!();

    println!("Latency:");
    println!("  Average: {:.2} ms", results.query_stats.avg_latency_ms);
    println!("  Median: {:.2} ms", results.query_stats.median_latency_ms);
    println!("  P95: {:.2} ms", results.query_stats.p95_latency_ms);
    println!("  P99: {:.2} ms", results.query_stats.p99_latency_ms);

    // Export results
    let csv = runner.export_csv(&results);
    let csv_path = format!("results_{}_{}.csv", args.dataset, args.algorithm);
    std::fs::write(&csv_path, csv)?;
    println!("\nResults exported to: {}", csv_path);

    let json = runner.export_json(&results)?;
    let json_path = format!("results_{}_{}.json", args.dataset, args.algorithm);
    std::fs::write(&json_path, json)?;
    println!("Results exported to: {}", json_path);

    Ok(())
}

/// Command-line arguments
struct Args {
    dataset: String,
    algorithm: String,
    dataset_path: PathBuf,
    k: usize,
    runs: usize,
    build_params: BuildParams,
    search_params: SearchParams,
}

/// Parse command-line arguments
fn parse_args() -> Args {
    let mut dataset = "sift".to_string();
    let mut algorithm = "hnsw".to_string();
    let mut dataset_path = PathBuf::from("/data/ann_benchmarks");
    let mut k = 10;
    let mut runs = 100;
    let mut m: Option<usize> = None;
    let mut ef_construction: Option<usize> = None;
    let mut ef_search: Option<usize> = None;
    let mut nlist: Option<usize> = None;
    let mut nprobe: Option<usize> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "--dataset" => {
                dataset = args[i + 1].clone();
                i += 2;
            }
            "--algorithm" => {
                algorithm = args[i + 1].clone();
                i += 2;
            }
            "--dataset-path" => {
                dataset_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--k" => {
                k = args[i + 1].parse().expect("Invalid k value");
                i += 2;
            }
            "--runs" => {
                runs = args[i + 1].parse().expect("Invalid runs value");
                i += 2;
            }
            "--m" => {
                m = Some(args[i + 1].parse().expect("Invalid m value"));
                i += 2;
            }
            "--ef-construction" => {
                ef_construction = Some(args[i + 1].parse().expect("Invalid ef-construction value"));
                i += 2;
            }
            "--ef-search" => {
                ef_search = Some(args[i + 1].parse().expect("Invalid ef-search value"));
                i += 2;
            }
            "--nlist" => {
                nlist = Some(args[i + 1].parse().expect("Invalid nlist value"));
                i += 2;
            }
            "--nprobe" => {
                nprobe = Some(args[i + 1].parse().expect("Invalid nprobe value"));
                i += 2;
            }
            "--help" => {
                print_help();
                process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                print_help();
                process::exit(1);
            }
        }
    }

    Args {
        dataset,
        algorithm,
        dataset_path,
        k,
        runs,
        build_params: BuildParams {
            m,
            ef_construction,
            nlist,
            ..Default::default()
        },
        search_params: SearchParams {
            ef_search,
            nprobe,
            ..Default::default()
        },
    }
}

/// Print help message
fn print_help() {
    println!("ProximaDB ANN-Benchmarks Runner");
    println!();
    println!("USAGE:");
    println!("  ann-benchmarks [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  --dataset <NAME>         Dataset name (sift, gist, mnist, deep1b) [default: sift]");
    println!("  --algorithm <NAME>       Algorithm name (hnsw, ivf, annoy, flat, lsh, pq, diskann) [default: hnsw]");
    println!("  --dataset-path <PATH>    Path to dataset files [default: /data/ann_benchmarks]");
    println!("  --k <N>                  Number of neighbors [default: 10]");
    println!("  --runs <N>               Number of query runs [default: 100]");
    println!();
    println!("  --m <N>                  HNSW M parameter (graph degree)");
    println!("  --ef-construction <N>    HNSW ef-construction parameter");
    println!("  --ef-search <N>          HNSW ef-search parameter");
    println!("  --nlist <N>              IVF number of centroids");
    println!("  --nprobe <N>             IVF number of centroids to probe");
    println!();
    println!("  --help                   Print this help message");
    println!();
    println!("EXAMPLES:");
    println!("  ann-benchmarks --dataset sift --algorithm hnsw");
    println!("  ann-benchmarks --dataset gist --algorithm ivf --nlist 100 --nprobe 10");
    println!("  ann-benchmarks --dataset mnist --algorithm flat --runs 1000");
}
