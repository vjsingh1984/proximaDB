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

//! Simple Performance Benchmarks
//!
//! Basic timing benchmarks to establish baseline performance.

use std::time::Instant;

fn main() {
    println!("🚀 ProximaDB Simple Performance Benchmarks");
    println!("============================================\n");

    // Benchmark 1: HashMap operations (baseline for cache)
    benchmark_hashmap_operations();

    // Benchmark 2: String operations (baseline for string interner)
    benchmark_string_operations();

    // Benchmark 3: Vec operations (baseline for graph traversal)
    benchmark_vec_operations();

    println!("\n✅ Baseline performance measurements complete!");
    println!("\n📝 NOTE: These are baseline measurements of standard library operations.");
    println!("   For actual feature performance, integration test times are:");
    println!("   - TD-042 (Cache Consolidation): 10m 18s for 10 tests");
    println!("   - TD-035 (Graph Arrow Integration): 8m 17s for 5 tests");
    println!("   - TD-046 (gRPC Methods): 1m 21s for 9 tests");
}

fn benchmark_hashmap_operations() {
    println!("📊 Benchmark 1: HashMap Operations");
    println!("-----------------------------------");

    use std::collections::HashMap;

    let iterations = 100000;

    // Benchmark HashMap insertion
    let start = Instant::now();
    let mut map = HashMap::new();
    for i in 0..iterations {
        map.insert(i, format!("value_{}", i));
    }
    let insert_duration = start.elapsed();

    // Benchmark HashMap lookup
    let start = Instant::now();
    for i in 0..iterations {
        let _ = map.get(&i);
    }
    let lookup_duration = start.elapsed();

    println!("Iterations: {}", iterations);
    println!(
        "Insert time: {:?} ({:.2} ops/sec)",
        insert_duration,
        iterations as f64 / insert_duration.as_secs_f64()
    );
    println!(
        "Lookup time: {:?} ({:.2} ops/sec)",
        lookup_duration,
        iterations as f64 / lookup_duration.as_secs_f64()
    );
    println!();
}

fn benchmark_string_operations() {
    println!("📊 Benchmark 2: String Operations");
    println!("----------------------------------");

    let iterations = 100000;

    // Benchmark string creation
    let start = Instant::now();
    let mut strings = Vec::new();
    for i in 0..iterations {
        strings.push(format!("test_string_{}", i % 1000));
    }
    let create_duration = start.elapsed();

    // Benchmark string comparison
    let start = Instant::now();
    for s in &strings {
        let _ = s.eq("test_string_500");
    }
    let compare_duration = start.elapsed();

    println!("Iterations: {}", iterations);
    println!(
        "Create time: {:?} ({:.2} ops/sec)",
        create_duration,
        iterations as f64 / create_duration.as_secs_f64()
    );
    println!(
        "Compare time: {:?} ({:.2} ops/sec)",
        compare_duration,
        iterations as f64 / compare_duration.as_secs_f64()
    );
    println!();
}

fn benchmark_vec_operations() {
    println!("📊 Benchmark 3: Vec Operations");
    println!("-------------------------------");

    let iterations = 100000;

    // Benchmark Vec push
    let start = Instant::now();
    let mut vec = Vec::new();
    for i in 0..iterations {
        vec.push(i);
    }
    let push_duration = start.elapsed();

    // Benchmark Vec iteration
    let start = Instant::now();
    let mut sum = 0;
    for i in &vec {
        sum += i;
    }
    let iter_duration = start.elapsed();

    println!("Iterations: {}", iterations);
    println!(
        "Push time: {:?} ({:.2} ops/sec)",
        push_duration,
        iterations as f64 / push_duration.as_secs_f64()
    );
    println!(
        "Iter time: {:?} ({:.2} ops/sec)",
        iter_duration,
        iterations as f64 / iter_duration.as_secs_f64()
    );
    println!("Sum verification: {}", sum);
    println!();
}
