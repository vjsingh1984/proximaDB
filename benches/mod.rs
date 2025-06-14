// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Performance benchmarking suite for ProximaDB
//! 
//! This module provides comprehensive benchmarks for all major database operations
//! including vector insertion, search, storage, and API performance.

pub mod vector_operations;
pub mod storage_performance;
pub mod search_benchmarks;
pub mod api_benchmarks;
pub mod axis_benchmarks;
pub mod throughput_tests;
pub mod latency_tests;
pub mod scalability_tests;

use std::time::{Duration, Instant};
use std::sync::Arc;
use criterion::{BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;
use uuid::Uuid;
use chrono::Utc;

use proximadb::core::{VectorRecord, CollectionId, VectorId, Config};
use proximadb::storage::StorageEngine;

/// Benchmark configuration
#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    /// Number of vectors for each benchmark
    pub vector_counts: Vec<usize>,
    
    /// Vector dimensions to test
    pub dimensions: Vec<usize>,
    
    /// Sparsity levels (0.0 = dense, 0.9 = very sparse)
    pub sparsity_levels: Vec<f32>,
    
    /// Number of iterations per benchmark
    pub iterations: usize,
    
    /// Warm-up iterations
    pub warmup_iterations: usize,
    
    /// Enable detailed profiling
    pub enable_profiling: bool,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            vector_counts: vec![1_000, 10_000, 100_000, 500_000],
            dimensions: vec![128, 256, 512, 1024, 1536], // Common embedding dimensions
            sparsity_levels: vec![0.0, 0.1, 0.5, 0.8, 0.9],
            iterations: 100,
            warmup_iterations: 10,
            enable_profiling: false,
        }
    }
}

/// Benchmark results aggregation
#[derive(Debug, Clone)]
pub struct BenchmarkResults {
    pub operation: String,
    pub configuration: String,
    pub mean_duration: Duration,
    pub min_duration: Duration,
    pub max_duration: Duration,
    pub p50_duration: Duration,
    pub p95_duration: Duration,
    pub p99_duration: Duration,
    pub throughput_per_second: f64,
    pub memory_usage_mb: f64,
    pub cpu_usage_percent: f64,
}

/// Benchmark data generator
pub struct BenchmarkDataGenerator {
    config: BenchmarkConfig,
}

impl BenchmarkDataGenerator {
    pub fn new(config: BenchmarkConfig) -> Self {
        Self { config }
    }
    
    /// Generate test vectors with specified characteristics
    pub fn generate_vectors(
        &self,
        count: usize,
        dimension: usize,
        sparsity: f32,
        collection_id: &str,
    ) -> Vec<VectorRecord> {
        let mut vectors = Vec::with_capacity(count);
        
        for i in 0..count {
            let vector_data = self.generate_single_vector(dimension, sparsity);
            let metadata = self.generate_metadata(i);
            
            vectors.push(VectorRecord {
                id: Uuid::new_v4(),
                collection_id: collection_id.to_string(),
                vector: vector_data,
                metadata,
                timestamp: Utc::now(),
                expires_at: None,
            });
        }
        
        vectors
    }
    
    /// Generate a single vector with specified sparsity
    fn generate_single_vector(&self, dimension: usize, sparsity: f32) -> Vec<f32> {
        let mut vector = vec![0.0; dimension];
        let non_zero_count = ((dimension as f32) * (1.0 - sparsity)) as usize;
        
        for i in 0..non_zero_count {
            let idx = (i * dimension / non_zero_count) % dimension;
            vector[idx] = (i as f32 * 0.1) % 1.0; // Deterministic values for reproducibility
        }
        
        // Normalize the vector
        let magnitude: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            for v in vector.iter_mut() {
                *v /= magnitude;
            }
        }
        
        vector
    }
    
    /// Generate metadata for vectors
    fn generate_metadata(&self, index: usize) -> std::collections::HashMap<String, serde_json::Value> {
        let mut metadata = std::collections::HashMap::new();
        
        // Add some realistic metadata fields
        metadata.insert("category".to_string(), serde_json::json!(format!("cat_{}", index % 10)));
        metadata.insert("priority".to_string(), serde_json::json!(index % 5));
        metadata.insert("created_at".to_string(), serde_json::json!(Utc::now().timestamp()));
        metadata.insert("source".to_string(), serde_json::json!(format!("source_{}", index % 3)));
        metadata.insert("version".to_string(), serde_json::json!(1));
        
        metadata
    }
    
    /// Generate query vectors for search benchmarks
    pub fn generate_query_vectors(
        &self,
        count: usize,
        dimension: usize,
        sparsity: f32,
    ) -> Vec<Vec<f32>> {
        (0..count)
            .map(|i| self.generate_single_vector(dimension, sparsity))
            .collect()
    }
}

/// Benchmark runner utility
pub struct BenchmarkRunner {
    pub runtime: Arc<Runtime>,
    pub config: BenchmarkConfig,
}

impl BenchmarkRunner {
    pub fn new(config: BenchmarkConfig) -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("Failed to create async runtime for benchmarks")
        );
        
        Self { runtime, config }
    }
    
    /// Create a test storage engine for benchmarks
    pub async fn create_test_storage(&self) -> Arc<tokio::sync::RwLock<StorageEngine>> {
        let config = Config::default();
        let storage_engine = StorageEngine::new(config.storage).await
            .expect("Failed to create storage engine for benchmarks");
        Arc::new(tokio::sync::RwLock::new(storage_engine))
    }
    
    /// Measure operation duration with statistics
    pub fn measure_operation<F, R>(&self, operation: F) -> (R, Duration)
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let result = operation();
        let duration = start.elapsed();
        (result, duration)
    }
    
    /// Measure async operation duration
    pub async fn measure_async_operation<F, Fut, R>(&self, operation: F) -> (R, Duration)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        let start = Instant::now();
        let result = operation().await;
        let duration = start.elapsed();
        (result, duration)
    }
    
    /// Calculate statistics from multiple measurements
    pub fn calculate_stats(&self, durations: &[Duration]) -> BenchmarkResults {
        let mut sorted_durations = durations.to_vec();
        sorted_durations.sort();
        
        let count = sorted_durations.len();
        let sum: Duration = sorted_durations.iter().sum();
        let mean = sum / count as u32;
        
        let min = sorted_durations[0];
        let max = sorted_durations[count - 1];
        let p50 = sorted_durations[count / 2];
        let p95 = sorted_durations[count * 95 / 100];
        let p99 = sorted_durations[count * 99 / 100];
        
        let throughput = if mean.as_secs_f64() > 0.0 {
            1.0 / mean.as_secs_f64()
        } else {
            0.0
        };
        
        BenchmarkResults {
            operation: "".to_string(),
            configuration: "".to_string(),
            mean_duration: mean,
            min_duration: min,
            max_duration: max,
            p50_duration: p50,
            p95_duration: p95,
            p99_duration: p99,
            throughput_per_second: throughput,
            memory_usage_mb: 0.0, // TODO: Implement memory tracking
            cpu_usage_percent: 0.0, // TODO: Implement CPU tracking
        }
    }
}

/// Resource usage monitoring
pub struct ResourceMonitor {
    start_memory: usize,
    start_time: Instant,
}

impl ResourceMonitor {
    pub fn new() -> Self {
        Self {
            start_memory: Self::get_memory_usage(),
            start_time: Instant::now(),
        }
    }
    
    /// Get current memory usage in bytes
    fn get_memory_usage() -> usize {
        // TODO: Implement actual memory tracking
        // This would use system calls or libraries like sysinfo
        0
    }
    
    /// Get memory usage delta since creation
    pub fn memory_delta_mb(&self) -> f64 {
        let current_memory = Self::get_memory_usage();
        (current_memory.saturating_sub(self.start_memory)) as f64 / 1024.0 / 1024.0
    }
    
    /// Get elapsed time since creation
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// Benchmark report generator
pub struct BenchmarkReporter;

impl BenchmarkReporter {
    /// Generate a comprehensive benchmark report
    pub fn generate_report(results: &[BenchmarkResults]) -> String {
        let mut report = String::new();
        
        report.push_str("# ProximaDB Performance Benchmark Report\n\n");
        report.push_str(&format!("**Generated**: {}\n", Utc::now().format("%Y-%m-%d %H:%M:%S UTC")));
        report.push_str(&format!("**Total Benchmarks**: {}\n\n", results.len()));
        
        // Summary statistics
        if !results.is_empty() {
            let avg_throughput: f64 = results.iter().map(|r| r.throughput_per_second).sum::<f64>() / results.len() as f64;
            let avg_latency: f64 = results.iter().map(|r| r.mean_duration.as_millis() as f64).sum::<f64>() / results.len() as f64;
            
            report.push_str("## Summary\n\n");
            report.push_str(&format!("- **Average Throughput**: {:.2} ops/sec\n", avg_throughput));
            report.push_str(&format!("- **Average Latency**: {:.2} ms\n", avg_latency));
            report.push_str("\n");
        }
        
        // Detailed results
        report.push_str("## Detailed Results\n\n");
        report.push_str("| Operation | Configuration | Mean (ms) | P95 (ms) | P99 (ms) | Throughput (ops/s) |\n");
        report.push_str("|-----------|---------------|-----------|----------|----------|--------------------|\n");
        
        for result in results {
            report.push_str(&format!(
                "| {} | {} | {:.2} | {:.2} | {:.2} | {:.2} |\n",
                result.operation,
                result.configuration,
                result.mean_duration.as_millis() as f64,
                result.p95_duration.as_millis() as f64,
                result.p99_duration.as_millis() as f64,
                result.throughput_per_second
            ));
        }
        
        report.push_str("\n");
        
        // Performance guidelines
        report.push_str("## Performance Guidelines\n\n");
        report.push_str("- **Write Latency Target**: < 1ms (WAL + memtable)\n");
        report.push_str("- **Read Latency Target**: < 2ms (memtable/MMAP)\n");
        report.push_str("- **Search Latency Target**: < 10ms (HNSW)\n");
        report.push_str("- **Throughput Target**: > 10,000 ops/sec per core\n\n");
        
        report
    }
    
    /// Save report to file
    pub fn save_report(report: &str, filename: &str) -> std::io::Result<()> {
        std::fs::write(filename, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_benchmark_config_default() {
        let config = BenchmarkConfig::default();
        assert!(!config.vector_counts.is_empty());
        assert!(!config.dimensions.is_empty());
        assert!(config.iterations > 0);
    }
    
    #[test]
    fn test_data_generator() {
        let config = BenchmarkConfig::default();
        let generator = BenchmarkDataGenerator::new(config);
        
        let vectors = generator.generate_vectors(100, 128, 0.1, "test_collection");
        assert_eq!(vectors.len(), 100);
        assert_eq!(vectors[0].vector.len(), 128);
        assert_eq!(vectors[0].collection_id, "test_collection");
    }
    
    #[test]
    fn test_vector_normalization() {
        let config = BenchmarkConfig::default();
        let generator = BenchmarkDataGenerator::new(config);
        
        let vector = generator.generate_single_vector(128, 0.1);
        let magnitude: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        // Should be approximately normalized (within floating point precision)
        assert!((magnitude - 1.0).abs() < 0.01);
    }
}