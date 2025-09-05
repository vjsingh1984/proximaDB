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
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::core::VectorRecord;
use proximadb::index::axis::indexes::{
    hnsw_index::{AxisHnswConfig, AxisHnswIndex, create_hnsw_index},
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

fn benchmark_distance_metrics(dimensions: &[usize], iterations: usize) -> Result<HashMap<(usize, DistanceMetric), f64>> {
    info!("Starting Distance Computation Benchmarks");
    info!("Dimensions: {:?}, Iterations: {}", dimensions, iterations);
    
    let mut results = HashMap::new();
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
    ];

    for &dim in dimensions {
        info!("\nTesting dimension: {}", dim);
        
        // Pre-allocate vectors outside the metric loop
        let vec1: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.1).collect();
        let vec2: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.2 + 1.0).collect();
        
        for metric in &metrics {
            // Create engine once per metric - using intuitive API
            let engine = UnifiedDistanceCompute::new(*metric);
            
            // Warm-up run to initialize any lazy structures
            let _ = engine.distance(&vec1, &vec2);
            
            let start = Instant::now();
            
            // Use the intuitive API for all iterations
            for _ in 0..iterations {
                let _ = engine.distance(&vec1, &vec2);
            }
            
            let elapsed = start.elapsed().as_secs_f64();
            let ops_per_sec = iterations as f64 / elapsed;
            
            info!("  {:?} @ {}D: {:.0} ops/sec", metric, dim, ops_per_sec);
            results.insert((dim, *metric), ops_per_sec);
        }
    }
    
    Ok(results)
}

// ============= Vector Operations Benchmarks =============

fn benchmark_vector_operations(dimensions: &[usize], num_vectors: usize) -> Result<()> {
    info!("Starting Vector Operations Benchmarks");
    info!("Dimensions: {:?}, Vectors: {}", dimensions, num_vectors);
    
    for &dimension in dimensions {
        info!("\nTesting dimension: {}", dimension);
        
        // Generate test vectors
        let vectors: Vec<VectorRecord> = (0..num_vectors)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: (0..dimension).map(|j| ((i + j) as f32) * 0.1).collect(),
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: None,
                source: None,
            })
            .collect();
        
        // Benchmark vector creation
        let start = Instant::now();
        let _vectors_copy: Vec<_> = vectors.iter().cloned().collect();
        let creation_time = start.elapsed();
        info!("  Vector creation time: {:?} ({:.0} vectors/sec)", 
              creation_time, num_vectors as f64 / creation_time.as_secs_f64());
        
        // Benchmark vector access
        let start = Instant::now();
        let mut sum = 0.0f32;
        for vec in &vectors {
            sum += vec.vector[0];
        }
        let access_time = start.elapsed();
        info!("  Vector access time: {:?} ({:.0} accesses/sec, sum={})", 
              access_time, num_vectors as f64 / access_time.as_secs_f64(), sum);
        
        // Benchmark distance calculations between vectors
        if num_vectors >= 2 {
            let engine = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
            let start = Instant::now();
            let num_comparisons = std::cmp::min(1000, num_vectors * (num_vectors - 1) / 2);
            let mut comparison_count = 0;
            
            'outer: for i in 0..num_vectors {
                for j in i+1..num_vectors {
                    let _ = engine.distance(&vectors[i].vector, &vectors[j].vector);
                    comparison_count += 1;
                    if comparison_count >= num_comparisons {
                        break 'outer;
                    }
                }
            }
            
            let comparison_time = start.elapsed();
            info!("  Pairwise distance time: {:?} ({:.0} comparisons/sec)", 
                  comparison_time, num_comparisons as f64 / comparison_time.as_secs_f64());
        }
    }
    
    Ok(())
}

// ============= Index Operations Benchmarks =============

async fn benchmark_index_operations(
    dimensions: &[usize], 
    num_vectors: usize, 
    index_types: Option<Vec<String>>
) -> Result<()> {
    info!("Starting Index Operations Benchmarks");
    info!("Dimensions: {:?}, Vectors: {}", dimensions, num_vectors);
    
    let all_types = vec!["hnsw".to_string(), "lsh".to_string()];
    let types_to_test = index_types.unwrap_or(all_types);
    
    for &dimension in dimensions {
        info!("\n=== Testing dimension: {} ===", dimension);
        
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
    info!("  HNSW Index:");
    
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
    info!("    Insert time: {:?} ({:.0} vectors/sec)", 
          insert_time, vectors.len() as f64 / insert_time.as_secs_f64());
    
    // Benchmark searches
    let query = &vectors[0];
    let start = Instant::now();
    let num_searches = 100;
    for _ in 0..num_searches {
        let _ = index.search(query, 10, None).await;
    }
    let search_time = start.elapsed();
    info!("    Search time: {:?} ({:.0} searches/sec)", 
          search_time, num_searches as f64 / search_time.as_secs_f64());
    
    Ok(())
}

async fn benchmark_lsh(dimension: usize, vectors: &[Vec<f32>]) -> Result<()> {
    info!("  LSH Index:");
    
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
        index.add_vector(Some(format!("vec_{}", i)), vec.clone()).await?;
    }
    let insert_time = start.elapsed();
    info!("    Insert time: {:?} ({:.0} vectors/sec)", 
          insert_time, vectors.len() as f64 / insert_time.as_secs_f64());
    
    // Benchmark searches
    let query = &vectors[0];
    let start = Instant::now();
    let num_searches = 100;
    for _ in 0..num_searches {
        let _ = index.search(query, 10, None).await;
    }
    let search_time = start.elapsed();
    info!("    Search time: {:?} ({:.0} searches/sec)", 
          search_time, num_searches as f64 / search_time.as_secs_f64());
    
    Ok(())
}

// ============= Main Entry Point =============

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    // Initialize hardware capabilities
    proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default()?;
    
    let cli = Cli::parse();
    
    info!("ProximaDB Unified Benchmark Suite");
    info!("==================================");
    
    match &cli.command {
        Some(Commands::All) | None => {
            // Run all benchmarks with defaults
            benchmark_distance_metrics(&cli.dimensions, cli.iterations)?;
            benchmark_vector_operations(&cli.dimensions, 1000)?;
            benchmark_index_operations(&cli.dimensions, 1000, None).await?;
        }
        Some(Commands::Distance { metrics }) => {
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
    
    info!("\nBenchmark suite completed successfully!");
    
    Ok(())
}