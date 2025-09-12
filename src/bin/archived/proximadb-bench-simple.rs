//! ProximaDB Simple Benchmark Suite
//! 
//! A simplified benchmark suite for testing core ProximaDB functionality

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::core::VectorRecord;

#[derive(Parser)]
#[command(name = "proximadb-bench-simple")]
#[command(about = "ProximaDB Simple Benchmark Suite")]
struct Cli {
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
    Distance,
    
    /// Benchmark vector operations
    Vectors,
}

// ============= Distance Computation Benchmarks =============

fn benchmark_distance_metrics(dimensions: &[usize], iterations: usize) -> Result<()> {
    info!("Starting Distance Computation Benchmarks");
    info!("Dimensions: {:?}, Iterations: {}", dimensions, iterations);
    
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
            // Create engine once per metric - now with intuitive API!
            let engine = UnifiedDistanceCompute::new(*metric);
            
            // Warm-up run to initialize any lazy structures
            // Using the new intuitive API that doesn't require passing the metric
            let _ = engine.distance(&vec1, &vec2);
            
            let start = Instant::now();
            
            // Use the new intuitive API for all iterations
            for _ in 0..iterations {
                let _ = engine.distance(&vec1, &vec2);
            }
            
            let elapsed = start.elapsed().as_secs_f64();
            let ops_per_sec = iterations as f64 / elapsed;
            
            info!("  {:?} @ {}D: {:.0} ops/sec", metric, dim, ops_per_sec);
        }
    }
    
    Ok(())
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
        info!("  Vector access time: {:?} ({:.0} accesses/sec)", 
              access_time, num_vectors as f64 / access_time.as_secs_f64());
    }
    
    Ok(())
}

// ============= Main Entry Point =============

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    // Initialize hardware capabilities
    proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    let cli = Cli::parse();
    
    info!("ProximaDB Simple Benchmark Suite");
    info!("=================================");
    
    match &cli.command {
        Some(Commands::All) | None => {
            // Run all benchmarks
            benchmark_distance_metrics(&cli.dimensions, cli.iterations)?;
            benchmark_vector_operations(&cli.dimensions, 1000)?;
        }
        Some(Commands::Distance) => {
            // Run distance benchmarks
            benchmark_distance_metrics(&cli.dimensions, cli.iterations)?;
        }
        Some(Commands::Vectors) => {
            // Run vector benchmarks
            benchmark_vector_operations(&cli.dimensions, 1000)?;
        }
    }
    
    info!("\nBenchmark suite completed successfully!");
    
    Ok(())
}