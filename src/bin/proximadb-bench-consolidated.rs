//! ProximaDB Unified Benchmark Suite
//!
//! A comprehensive benchmark suite for ProximaDB that tests:
//! - Distance computation performance  
//! - Vector operations
//! - Index operations (HNSW, LSH)
//! - Future: Storage engines and concurrent operations

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
// use std::sync::Arc; // Unused - removing to fix warning
use std::time::Instant;
use tracing::warn;

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::index::axis::indexes::{
    hnsw_index::{AxisHnswConfig, create_hnsw_index},
    lsh_index::{AxisLshConfig, AxisLshIndex},
};

#[derive(Parser)]
#[command(name = "proximadb-bench")]
#[command(about = "ProximaDB Unified Benchmark Suite", long_about = None)]
struct Cli {
    /// Verbosity level (0-3)
    #[arg(short, long, default_value_t = 1)]
    verbose: u8,

    /// Number of iterations for benchmarks
    #[arg(short = 'n', long, default_value_t = 1000)]
    iterations: usize,

    /// Vector dimensions to test
    #[arg(short = 'd', long, value_delimiter = ',', default_values = vec!["128", "256", "512"])]
    dimensions: Vec<usize>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run all benchmarks
    All,

    /// Benchmark distance computation algorithms
    Distance {
        /// Specific metrics to test (cosine, euclidean, dot_product, manhattan)
        #[arg(short, long, value_delimiter = ',')]
        metrics: Option<Vec<String>>,
    },

    /// Benchmark vector operations
    Vectors {
        /// Number of vectors to test with
        #[arg(short, long, default_value_t = 1000)]
        num_vectors: usize,
    },

    /// Benchmark index operations
    Index {
        /// Index types to test (hnsw, lsh)
        #[arg(short, long, value_delimiter = ',')]
        types: Option<Vec<String>>,

        /// Number of vectors for index tests
        #[arg(short = 'n', long, default_value_t = 1000)]
        num_vectors: usize,
    },
}

// ============= Distance Computation Benchmarks =============

fn benchmark_distance_metrics(
    dimensions: &[usize],
    iterations: usize,
) -> Result<HashMap<(usize, DistanceMetric), f64>> {
    println!("Starting Distance Computation Benchmarks");
    println!("Starting Distance Computation Benchmarks");
    println!("Dimensions: {:?}, Iterations: {}", dimensions, iterations);

    let mut results = HashMap::new();
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
    ];

    println!("About to create engines with GPU disabled...");
    let mut engines = HashMap::new();

    for metric in &metrics {
        println!("Creating UnifiedDistanceCompute for metric: {:?}", metric);
        let engine = UnifiedDistanceCompute::new(*metric);
        // GPU is lazily initialized only when needed
        println!("Created engine for {:?}", metric);
        engines.insert(*metric, engine);
    }

    println!("All engines created successfully");

    for &dim in dimensions {
        println!("Testing dimension: {}", dim);
        println!("\nTesting dimension: {}", dim);

        // Pre-allocate vectors outside the metric loop
        let vec1: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.1).collect();
        let vec2: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.2 + 1.0).collect();

        for metric in &metrics {
            println!("  Testing metric: {:?}", metric);
            let engine = &engines[metric];

            // Warm-up run to initialize any lazy structures
            println!("  Running warm-up...");
            let _ = engine.distance(&vec1, &vec2);
            println!("  Warm-up complete");

            let start = Instant::now();

            // Use the engine API for all iterations
            let mut sum = 0.0;
            for _ in 0..iterations {
                let dist = engine.distance(&vec1, &vec2);
                sum += dist; // Prevent optimization
            }

            let elapsed = start.elapsed().as_secs_f64();
            // Prevent dead code elimination
            if sum < 0.0 {
                println!("Unexpected negative sum: {}", sum);
            }
            let ops_per_sec = iterations as f64 / elapsed;

            println!(
                "  {:?} @ {}D: {:.0} ops/sec ({}ms total)",
                metric,
                dim,
                ops_per_sec,
                (elapsed * 1000.0) as u64
            );
            println!("  {:?} @ {}D: {:.0} ops/sec", metric, dim, ops_per_sec);
            results.insert((dim, *metric), ops_per_sec);
        }
    }

    Ok(results)
}

// ============= Vector Operations Benchmarks =============

fn benchmark_vector_operations(dimensions: &[usize], num_vectors: usize) -> Result<()> {
    println!("Starting Vector Operations Benchmarks");
    println!("Dimensions: {:?}, Vectors: {}", dimensions, num_vectors);

    // Limit vectors to prevent stack overflow
    let safe_num_vectors = std::cmp::min(num_vectors, 1000);
    if safe_num_vectors != num_vectors {
        warn!(
            "Limiting vectors from {} to {} to prevent memory issues",
            num_vectors, safe_num_vectors
        );
    }

    for &dimension in dimensions {
        println!("\nTesting dimension: {}", dimension);

        // Generate test vectors with chunked processing
        let chunk_size = 100;
        let mut total_ops = 0f64;

        for chunk_start in (0..safe_num_vectors).step_by(chunk_size) {
            let chunk_end = std::cmp::min(chunk_start + chunk_size, safe_num_vectors);
            let chunk_vectors: Vec<VectorRecord> = (chunk_start..chunk_end)
                .map(|i| VectorRecord {
                    id: format!("vec_{}", i),
                    vector: (0..dimension).map(|j| ((i + j) as f32) * 0.1).collect(),
                    metadata: HashMap::new(),
                    timestamp: 0,
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    quantized_vector: vec![],
                    source: None,
                })
                .collect();

            // Benchmark this chunk
            let start = Instant::now();
            let _vectors_copy: Vec<_> = chunk_vectors.iter().cloned().collect();
            let creation_time = start.elapsed();
            let ops_per_sec = (chunk_end - chunk_start) as f64 / creation_time.as_secs_f64();
            total_ops += ops_per_sec;

            println!(
                "  Chunk [{}-{}] creation: {:?} ({:.0} vectors/sec)",
                chunk_start, chunk_end, creation_time, ops_per_sec
            );
        }

        println!(
            "  Average creation rate: {:.0} vectors/sec",
            total_ops / (safe_num_vectors / chunk_size) as f64
        );
    }

    Ok(())
}

// ============= Index Operations Benchmarks =============

async fn benchmark_index_operations(
    dimensions: &[usize],
    num_vectors: usize,
    index_types: Option<Vec<String>>,
) -> Result<()> {
    println!("Starting Index Operations Benchmarks");
    println!("Dimensions: {:?}, Vectors: {}", dimensions, num_vectors);

    let all_types = vec!["hnsw".to_string(), "lsh".to_string()];
    let types_to_test = index_types.unwrap_or(all_types);

    for &dimension in dimensions {
        println!("\n=== Testing dimension: {} ===", dimension);

        // Generate test vectors
        let vectors: Vec<Vec<f32>> = (0..num_vectors)
            .map(|i| (0..dimension).map(|j| ((i + j) as f32) * 0.1).collect())
            .collect();

        for index_type in &types_to_test {
            match index_type.as_str() {
                "hnsw" => benchmark_hnsw(dimension, &vectors).await?,
                "lsh" => benchmark_lsh(dimension, &vectors).await?,
                _ => warn!("Unknown index type: {}", index_type),
            }
        }
    }

    Ok(())
}

async fn benchmark_hnsw(dimension: usize, vectors: &[Vec<f32>]) -> Result<()> {
    println!("  HNSW Index:");

    let config = AxisHnswConfig {
        m: 16,
        ef_construction: 200,
        ef: 100,
        max_layers: 16,
        distance_metric: DistanceMetric::Cosine,
    };

    let index = create_hnsw_index(config, dimension)?;

    // Benchmark insertions
    let start = Instant::now();
    for (i, vec) in vectors.iter().enumerate() {
        index.add(format!("vec_{}", i), vec.clone()).await?;
    }
    let insert_time = start.elapsed();
    println!(
        "    Insert time: {:?} ({:.0} vectors/sec)",
        insert_time,
        vectors.len() as f64 / insert_time.as_secs_f64()
    );

    // Benchmark searches
    let query = &vectors[0];
    let start = Instant::now();
    let num_searches = 100;
    for _ in 0..num_searches {
        let _ = index.search(query, 10, None).await;
    }
    let search_time = start.elapsed();
    println!(
        "    Search time: {:?} ({:.0} searches/sec)",
        search_time,
        num_searches as f64 / search_time.as_secs_f64()
    );

    Ok(())
}

async fn benchmark_lsh(dimension: usize, vectors: &[Vec<f32>]) -> Result<()> {
    println!("  LSH Index:");

    let config = AxisLshConfig {
        n_tables: 8,
        n_hashes: 10,
        hash_width: 4.0,
        distance_metric: DistanceMetric::Cosine,
        binary_mode: false,
        seed: 42,
    };

    let index = AxisLshIndex::new(config, dimension);

    // Benchmark insertions
    let start = Instant::now();
    for (i, vec) in vectors.iter().enumerate() {
        index
            .add_vector(Some(format!("vec_{}", i)), vec.clone())
            .await?;
    }
    let insert_time = start.elapsed();
    println!(
        "    Insert time: {:?} ({:.0} vectors/sec)",
        insert_time,
        vectors.len() as f64 / insert_time.as_secs_f64()
    );

    // Benchmark searches
    let query = &vectors[0];
    let start = Instant::now();
    let num_searches = 100;
    for _ in 0..num_searches {
        let _ = index.search(query, 10, None).await;
    }
    let search_time = start.elapsed();
    println!(
        "    Search time: {:?} ({:.0} searches/sec)",
        search_time,
        num_searches as f64 / search_time.as_secs_f64()
    );

    Ok(())
}

// ============= Main Entry Point =============

#[tokio::main]
async fn main() -> Result<()> {
    println!("Starting ProximaDB benchmark...");

    // Initialize tracing
    tracing_subscriber::fmt::init();
    println!("Tracing initialized");

    // Initialize hardware capabilities
    println!("About to initialize hardware capabilities...");
    proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default()?;
    println!("Hardware capabilities initialized");

    let cli = Cli::parse();
    println!("CLI parsed");

    println!("ProximaDB Unified Benchmark Suite");
    println!("==================================");

    match &cli.command {
        Some(Commands::All) | None => {
            // Run all benchmarks with defaults
            benchmark_distance_metrics(&cli.dimensions, cli.iterations)?;
            benchmark_vector_operations(&cli.dimensions, 1000)?;
            benchmark_index_operations(&cli.dimensions, 1000, None).await?;
        }
        Some(Commands::Distance { metrics: _ }) => {
            // TODO: Filter metrics if specified
            benchmark_distance_metrics(&cli.dimensions, cli.iterations)?;
        }
        Some(Commands::Vectors { num_vectors }) => {
            benchmark_vector_operations(&cli.dimensions, *num_vectors)?;
        }
        Some(Commands::Index { types, num_vectors }) => {
            benchmark_index_operations(&cli.dimensions, *num_vectors, types.clone()).await?;
        }
    }

    println!("\nBenchmark suite completed successfully!");

    Ok(())
}
