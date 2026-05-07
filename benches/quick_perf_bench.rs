/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Quick Performance Benchmarks
//!
//! Simple timing-based benchmarks to establish baseline performance measurements.
//! These benchmarks use std::time::Instant to measure actual operation timings.

use proximadb::storage::cache::unified_cache::UnifiedCacheCoordinator;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    println!("🚀 ProximaDB Quick Performance Benchmarks");
    println!("===========================================\n");

    // Benchmark 1: String Interner
    benchmark_string_interner();

    // Benchmark 2: Cache Coordinator Creation
    benchmark_coordinator_creation();

    // Benchmark 3: Cache Operations
    benchmark_cache_operations();

    println!("\n✅ Baseline performance measurements complete!");
}

fn benchmark_string_interner() {
    println!("📊 Benchmark 1: String Interner Performance");
    println!("-------------------------------------------");

    let coordinator = UnifiedCacheCoordinator::new();
    let iterations = 10000;
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Benchmark string interning
    let start = Instant::now();
    for i in 0..iterations {
        let test_string = format!("test_string_{}", i % 1000); // 1000 unique strings
        let _id = rt.block_on(coordinator.string_interner().intern(&test_string));
    }
    let duration = start.elapsed();

    println!("Iterations: {}", iterations);
    println!("Total time: {:?}", duration);
    println!("Average time per operation: {:?}", duration / iterations);
    println!(
        "Operations per second: {:.2}",
        iterations as f64 / duration.as_secs_f64()
    );
    println!();
}

fn benchmark_coordinator_creation() {
    println!("📊 Benchmark 2: Cache Coordinator Creation");
    println!("-------------------------------------------");

    let iterations = 1000;

    let start = Instant::now();
    for _ in 0..iterations {
        let _coordinator = UnifiedCacheCoordinator::new();
    }
    let duration = start.elapsed();

    println!("Iterations: {}", iterations);
    println!("Total time: {:?}", duration);
    println!("Average time per creation: {:?}", duration / iterations);
    println!(
        "Creations per second: {:.2}",
        iterations as f64 / duration.as_secs_f64()
    );
    println!();
}

fn benchmark_cache_operations() {
    println!("📊 Benchmark 3: Cache Operations");
    println!("----------------------------------");

    let coordinator = Arc::new(UnifiedCacheCoordinator::new());
    let iterations = 10000;
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Benchmark get_memory_usage (get_all_stats equivalent)
    let start = Instant::now();
    for _ in 0..iterations {
        let _stats = rt.block_on(coordinator.get_all_stats());
    }
    let duration = start.elapsed();

    println!("Iterations: {}", iterations);
    println!("get_all_stats() total time: {:?}", duration);
    println!("Average time per call: {:?}", duration / iterations);
    println!(
        "Calls per second: {:.2}",
        iterations as f64 / duration.as_secs_f64()
    );
    println!();
}
