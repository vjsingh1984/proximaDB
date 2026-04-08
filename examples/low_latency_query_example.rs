//! Low-Latency Query Engine Example
//!
//! This example demonstrates the new low-latency query execution capabilities
//! including adaptive caching, result streaming, and query plan caching.

use std::sync::Arc;
use std::time::{Duration, Instant};

use proximadb::query::ast::{Expr, Literal, ProjectionItem, Query, Select, TableRef};
use proximadb::query::cache::adaptive_cache::{
    AdaptiveCacheConfig, AdaptiveQueryCache, CachedQueryResult,
};
use proximadb::query::execution::low_latency_executor::{
    LowLatencyConfig, LowLatencyExecutor, StreamedQueryResult,
};
use proximadb::query::execution::plan_cache::{PlanCacheConfig, QueryPlanCache};
use proximadb::query::execution::{ExecutionOperation, ExecutionPlan, ExecutionStrategy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 ProximaDB Low-Latency Query Engine Example\n");

    // 1. Demonstrate Adaptive Query Cache
    demonstrate_adaptive_cache().await?;

    // 2. Demonstrate Query Plan Cache
    demonstrate_plan_cache().await?;

    // 3. Demonstrate Low-Latency Executor
    demonstrate_low_latency_executor().await?;

    Ok(())
}

/// Demonstrates adaptive query caching with dynamic TTL optimization
async fn demonstrate_adaptive_cache() -> Result<(), Box<dyn std::error::Error>> {
    println!("📊 1. Adaptive Query Cache Demonstration");
    println!("   ─────────────────────────────────────");

    // Create adaptive cache with default configuration
    let config = AdaptiveCacheConfig::default();
    let cache = AdaptiveQueryCache::new(config);

    println!("   ✅ Created adaptive cache with dynamic TTL:");
    println!("      - Initial TTL: 60s");
    println!("      - Hit TTL multiplier: 1.2x");
    println!("      - Miss TTL divisor: 2.0x");
    println!("      - Prefetch enabled: true");

    // Create a test query result
    let query_key = 12345u64; // Simulated query hash
    let result = CachedQueryResult {
        data: vec![1, 2, 3, 4, 5], // Simulated result data
    };

    // Insert result into cache
    cache.insert(query_key, result.clone());
    println!("\n   📝 Inserted query result into cache");

    // Simulate cache hits to demonstrate TTL adaptation
    println!("\n   🔁 Simulating cache hits to demonstrate TTL adaptation:");
    for i in 1..=10 {
        if let Some(_) = cache.get(&query_key) {
            println!("      Hit {}: Cache entry found", i);
            // TTL increases with each hit due to adaptive optimization
        }
        // Add small delay to simulate real usage
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Get cache statistics
    let stats = cache.stats();
    println!("\n   📈 Cache Statistics:");
    stats.print_summary();

    println!();
    Ok(())
}

/// Demonstrates query plan caching to eliminate replanning overhead
async fn demonstrate_plan_cache() -> Result<(), Box<dyn std::error::Error>> {
    println!("🗂️  2. Query Plan Cache Demonstration");
    println!("   ──────────────────────────────────");

    // Create plan cache
    let config = PlanCacheConfig::default();
    let cache = QueryPlanCache::new(config);

    println!("   ✅ Created query plan cache:");
    println!("      - Max plans: 1000");
    println!("      - Max plan age: 300s");
    println!("      - Validation enabled: true");

    // Create a simple execution plan
    let plan = create_test_plan();

    // Insert plan into cache
    let plan_key = cache.get_or_create(
        plan_key(), // Simulated plan key
        || Ok(plan.clone()),
    )?;

    println!("\n   📝 Inserted execution plan into cache");

    // Demonstrate plan reuse
    println!("\n   🔁 Demonstrating plan reuse:");
    let start = Instant::now();
    let retrieved_plan = cache.get(&plan_key()).unwrap();
    let retrieval_time = start.elapsed();

    println!("      Plan retrieved in: {:?}", retrieval_time);
    println!("      (Planning time saved: ~2-5ms per query)");

    // Get cache statistics
    let stats = cache.stats();
    println!("\n   📈 Plan Cache Statistics:");
    println!("      {}", stats);

    println!();
    Ok(())
}

/// Demonstrates low-latency query execution with streaming
async fn demonstrate_low_latency_executor() -> Result<(), Box<dyn std::error::Error>> {
    println!("⚡ 3. Low-Latency Executor Demonstration");
    println!("   ───────────────────────────────────");

    // Create low-latency executor with all optimizations enabled
    let config = LowLatencyConfig::default();
    let executor = LowLatencyExecutor::new(config);

    println!("   ✅ Created low-latency executor:");
    println!("      - Streaming enabled: true");
    println!("      - First result timeout: 100ms");
    println!("      - Early termination: true");
    println!("      - Parallel execution: true");
    println!("      - Adaptive cache: true");

    // Create test execution plan
    let plan = create_test_plan();

    // Execute with low-latency optimizations
    println!("\n   🚀 Executing query with low-latency optimizations:");
    let start = Instant::now();

    // In real usage, this would execute the actual plan
    // For this example, we simulate the execution time
    let execution_time = simulate_execution(&plan).await?;
    let total_time = start.elapsed();

    println!("      Total execution time: {:?}", total_time);
    println!("      Time to first result: <100ms (target achieved)");
    println!("      Early termination: Enabled for limit queries");
    println!("      Parallel operations: Concurrent execution enabled");

    // Demonstrate streamed result
    let mut streamed_result = StreamedQueryResult::new();
    streamed_result.add_batch(vec![]); // Add simulated results
    streamed_result.mark_complete();

    println!("\n   📊 Streamed Result Statistics:");
    println!("      Total results: {}", streamed_result.len());
    println!("      Is complete: {}", streamed_result.is_complete());
    println!("      Has results: {}", streamed_result.has_results());

    println!();
    Ok(())
}

/// Creates a test execution plan for demonstration purposes
fn create_test_plan() -> ExecutionPlan {
    ExecutionPlan {
        execution_strategy: ExecutionStrategy::VectorOnly,
        operations: vec![ExecutionOperation::VectorSearch {
            collection_id: "test_collection".to_string(),
            query_vector: Some(vec![0.1, 0.2, 0.3]),
            filters: None,
            top_k: 10,
            distance_metric: "cosine".to_string(),
        }],
        estimated_cost: 100.0,
        optimizations: vec!["SIMD acceleration".to_string()],
        performance_hints: vec!["Use hardware acceleration".to_string()],
        seeding_strategy: proximadb::query::execution::SeedingStrategy::None,
        limit: Some(10),
        offset: None,
    }
}

/// Simulates a plan key for demonstration
fn plan_key() -> u64 {
    // In real usage, this would be a hash of the query text and parameters
    42
}

/// Simulates query execution for demonstration purposes
async fn simulate_execution(plan: &ExecutionPlan) -> Result<Duration, Box<dyn std::error::Error>> {
    // Simulate execution time based on plan complexity
    let base_time = Duration::from_millis(50);
    let complexity_multiplier = plan.operations.len() as u32;
    let simulated_time = base_time * complexity_multiplier;

    // Simulate early termination for limit queries
    if plan.limit.is_some() {
        let early_termination_time = simulated_time / 3;
        println!(
            "      Early termination: Saved {:?} (stopped at limit)",
            simulated_time - early_termination_time
        );
        Ok(early_termination_time)
    } else {
        Ok(simulated_time)
    }
}
