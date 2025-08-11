/*
 * Copyright 2025 ProximaDB
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

//! Comprehensive integration tests for high-performance SQL engine
//!
//! Tests the complete integration of:
//! - Lock-free parser pool
//! - SIMD-optimized vector parsing  
//! - Query result caching
//! - Concurrent access patterns

#[cfg(test)]
mod tests {
    use crate::query::sql_engine::{
        get_global_pool, parse_sql_global, parse_vector_simd,
        get_global_query_cache, QueryCacheKey, cache_query_result, get_cached_query_result
    };
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{debug, error, info};
    
    /// Test complete SQL parsing pipeline with all optimizations
    #[test]
    fn test_complete_sql_pipeline() {
        debug!("🧪 Testing complete high-performance SQL pipeline...");
        
        // Test queries with vector similarity
        let test_queries = vec![
            "SELECT * FROM products LIMIT 10",
            "SELECT id, name FROM users WHERE metadata.active = true",
            "SELECT vector FROM embeddings ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 5",
            "SELECT metadata FROM docs WHERE metadata.category IN ('tech', 'science')",
            "SELECT id FROM collections WHERE metadata.size BETWEEN 100 AND 1000",
        ];
        
        let start = Instant::now();
        
        // Parse all queries multiple times to test parser pool reuse
        for iteration in 0..10 {
            for (i, query) in test_queries.iter().enumerate() {
                let result = parse_sql_global(query.to_string());
                assert!(result.is_ok(), "Query {} failed at iteration {}: {:?}", i, iteration, result.err());
                
                let parsed = result.unwrap();
                match i {
                    0 => {
                        assert_eq!(parsed.from_collection, "products");
                        assert_eq!(parsed.limit, Some(10));
                    }
                    1 => {
                        assert_eq!(parsed.from_collection, "users");
                        assert!(parsed.where_conditions.is_some());
                    }
                    2 => {
                        assert_eq!(parsed.from_collection, "embeddings");
                        assert!(parsed.order_by.is_some());
                        assert_eq!(parsed.limit, Some(5));
                    }
                    _ => {} // Other queries just need to parse successfully
                }
            }
        }
        
        let elapsed = start.elapsed();
        let total_queries = test_queries.len() * 10;
        
        info!("✅ Parsed {} queries in {:?} ({:.0} queries/sec)", 
               total_queries, elapsed, total_queries as f64 / elapsed.as_secs_f64());
        
        // Check parser pool statistics
        let pool = get_global_pool();
        let stats = pool.get_stats();
        let (created, reused, pool_size, peak) = stats.get_stats();
        
        debug!("📊 Parser Pool Stats - Created: {}, Reused: {}, Pool Size: {}, Peak: {}", 
               created, reused, pool_size, peak);
        
        // Should have excellent reuse ratio
        if created + reused > 0 {
            let reuse_ratio = reused as f64 / (created + reused) as f64;
            assert!(reuse_ratio > 0.7, "Parser reuse ratio should be >70%, got {:.1}%", reuse_ratio * 100.0);
            info!("✅ Parser reuse ratio: {:.1}%", reuse_ratio * 100.0);
        }
    }
    
    /// Test SIMD vector parsing integration
    #[test]
    fn test_simd_vector_integration() {
        debug!("🧪 Testing SIMD vector parsing integration...");
        
        let vector_test_cases = vec![
            "[1.0, 2.0, 3.0, 4.0]", // SSE4.1 size
            "[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]", // AVX2 size
            format!("[{}]", (0..16).map(|i| format!("{}.5", i)).collect::<Vec<_>>().join(", ")), // Large vector
            format!("[{}]", (0..128).map(|i| format!("{}.25", i)).collect::<Vec<_>>().join(", ")), // Very large vector
        ];
        
        let start = Instant::now();
        
        for (i, vector_str) in vector_test_cases.iter().enumerate() {
            // Test SIMD parsing directly
            let result = parse_vector_simd(vector_str);
            assert!(result.is_ok(), "SIMD parsing failed for case {}: {:?}", i, result.err());
            
            let vector = result.unwrap();
            match i {
                0 => assert_eq!(vector, vec![1.0, 2.0, 3.0, 4.0]),
                1 => assert_eq!(vector, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]),
                2 => {
                    assert_eq!(vector.len(), 16);
                    assert_eq!(vector[0], 0.5);
                    assert_eq!(vector[15], 15.5);
                }
                3 => {
                    assert_eq!(vector.len(), 128);
                    assert_eq!(vector[0], 0.25);
                    assert_eq!(vector[127], 127.25);
                }
                _ => {}
            }
            
            // Test integration with SQL parsing
            let sql_query = format!("SELECT vector FROM test ORDER BY VECTOR_SIMILARITY(vector, {}, 'cosine') LIMIT 5", vector_str);
            let sql_result = parse_sql_global(sql_query);
            assert!(sql_result.is_ok(), "SQL parsing with SIMD vector failed for case {}", i);
        }
        
        let elapsed = start.elapsed();
        info!("✅ SIMD vector parsing completed in {:?}", elapsed);
    }
    
    /// Test query result caching integration
    #[test]
    fn test_query_result_caching() {
        debug!("🧪 Testing query result caching integration...");
        
        let cache = get_global_query_cache();
        let initial_stats = cache.stats();
        let initial_hits = initial_stats.hits.load(Ordering::Relaxed);
        let initial_misses = initial_stats.misses.load(Ordering::Relaxed);
        
        // Test cache operations
        let test_cases = vec![
            ("SELECT * FROM products", "collection_1", b"result_data_1".to_vec()),
            ("SELECT id FROM users", "collection_2", b"result_data_2".to_vec()),
            ("SELECT vector FROM docs", "collection_1", b"result_data_3".to_vec()),
        ];
        
        // Insert test data
        for (query, collection, data) in &test_cases {
            let key = QueryCacheKey::new(query, collection, None);
            cache_query_result(key, data.clone(), query.to_string());
        }
        
        // Test cache hits
        for (query, collection, expected_data) in &test_cases {
            let key = QueryCacheKey::new(query, collection, None);
            let cached_result = get_cached_query_result(&key);
            assert_eq!(cached_result, Some(expected_data.clone()), "Cache miss for query: {}", query);
        }
        
        // Test cache miss
        let miss_key = QueryCacheKey::new("SELECT * FROM nonexistent", "collection_x", None);
        let miss_result = get_cached_query_result(&miss_key);
        assert!(miss_result.is_none(), "Expected cache miss for nonexistent query");
        
        // Check statistics
        let final_stats = cache.stats();
        let final_hits = final_stats.hits.load(Ordering::Relaxed);
        let final_misses = final_stats.misses.load(Ordering::Relaxed);
        
        let hits_delta = final_hits - initial_hits;
        let misses_delta = final_misses - initial_misses;
        
        assert!(hits_delta >= 3, "Expected at least 3 cache hits, got {}", hits_delta);
        assert!(misses_delta >= 1, "Expected at least 1 cache miss, got {}", misses_delta);
        
        info!("✅ Cache operations: {} hits, {} misses", hits_delta, misses_delta);
        debug!("📊 Cache Stats: {}", final_stats.summary());
        
        // Test collection-based invalidation
        cache.invalidate_collection("collection_1");
        
        // collection_1 queries should now miss
        let key1 = QueryCacheKey::new("SELECT * FROM products", "collection_1", None);
        let key3 = QueryCacheKey::new("SELECT vector FROM docs", "collection_1", None);
        assert!(get_cached_query_result(&key1).is_none(), "collection_1 should be invalidated");
        assert!(get_cached_query_result(&key3).is_none(), "collection_1 should be invalidated");
        
        // collection_2 should still be cached
        let key2 = QueryCacheKey::new("SELECT id FROM users", "collection_2", None);
        assert!(get_cached_query_result(&key2).is_some(), "collection_2 should still be cached");
        
        info!("✅ Collection-based cache invalidation working");
    }
    
    /// Test concurrent access to all components
    #[test]
    fn test_concurrent_integration() {
        debug!("🧪 Testing concurrent integration of all SQL engine components...");
        
        let num_threads = num_cpus::get();
        let queries_per_thread = 25; // Minimum as requested
        
        debug!("📊 Testing with {} threads, {} queries per thread", num_threads, queries_per_thread);
        
        let success_counter = Arc::new(AtomicUsize::new(0));
        let error_counter = Arc::new(AtomicUsize::new(0));
        
        let start = Instant::now();
        
        let handles: Vec<_> = (0..num_threads).map(|thread_id| {
            let success_counter = Arc::clone(&success_counter);
            let error_counter = Arc::clone(&error_counter);
            
            thread::spawn(move || {
                for query_id in 0..queries_per_thread {
                    // Mix of different query types and operations
                    let operation_type = query_id % 4;
                    
                    match operation_type {
                        0 => {
                            // Test SQL parsing with vector similarity
                            let vector_size = 4 + (query_id % 8); // Vary vector sizes
                            let vector_str = format!("[{}]", 
                                (0..vector_size).map(|i| format!("{}.{}", thread_id, i))
                                .collect::<Vec<_>>().join(", "));
                            
                            let query = format!(
                                "SELECT * FROM collection_{} ORDER BY VECTOR_SIMILARITY(vector, {}, 'cosine') LIMIT {}",
                                thread_id, vector_str, query_id % 10 + 1
                            );
                            
                            match parse_sql_global(query.clone()) {
                                Ok(parsed) => {
                                    assert_eq!(parsed.from_collection, format!("collection_{}", thread_id));
                                    success_counter.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    error_counter.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        1 => {
                            // Test SIMD vector parsing
                            let vector_str = format!("[{}]", 
                                (0..8).map(|i| format!("{}.{}", thread_id, i))
                                .collect::<Vec<_>>().join(", "));
                            
                            match parse_vector_simd(&vector_str) {
                                Ok(vector) => {
                                    assert_eq!(vector.len(), 8);
                                    success_counter.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    error_counter.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        2 => {
                            // Test query result caching
                            let key = QueryCacheKey::new(
                                &format!("SELECT * FROM test_{}", query_id),
                                &format!("collection_{}", thread_id),
                                None
                            );
                            let result_data = format!("result_{}_{}", thread_id, query_id).into_bytes();
                            
                            // Cache the result
                            cache_query_result(key.clone(), result_data.clone(), 
                                format!("SELECT * FROM test_{}", query_id));
                            
                            // Retrieve it
                            match get_cached_query_result(&key) {
                                Some(cached) if cached == result_data => {
                                    success_counter.fetch_add(1, Ordering::Relaxed);
                                }
                                _ => {
                                    error_counter.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        _ => {
                            // Test complex SQL parsing
                            let query = format!(
                                "SELECT id, metadata FROM collection_{} WHERE metadata.category = 'test' AND metadata.score > {} LIMIT {}",
                                thread_id, query_id % 100, query_id % 20 + 1
                            );
                            
                            match parse_sql_global(query) {
                                Ok(parsed) => {
                                    assert!(parsed.where_conditions.is_some());
                                    success_counter.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    error_counter.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }
            })
        }).collect();
        
        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }
        
        let elapsed = start.elapsed();
        let total_operations = num_threads * queries_per_thread;
        let successes = success_counter.load(Ordering::Relaxed);
        let errors = error_counter.load(Ordering::Relaxed);
        
        info!("🎯 Concurrent Integration Test Results:");
        debug!("  Total operations: {}", total_operations);
        debug!("  Successful: {}", successes);
        debug!("  Errors: {}", errors);
        debug!("  Success rate: {:.1}%", (successes as f64 / total_operations as f64) * 100.0);
        debug!("  Duration: {:?}", elapsed);
        debug!("  Throughput: {:.0} operations/sec", total_operations as f64 / elapsed.as_secs_f64());
        
        // Should have very high success rate
        assert_eq!(successes, total_operations, "All operations should succeed in concurrent test");
        assert_eq!(errors, 0, "No errors should occur in concurrent test");
        
        // Check component statistics
        let pool = get_global_pool();
        let pool_stats = pool.get_stats();
        let (created, reused, pool_size, peak) = pool_stats.get_stats();
        
        debug!("📊 Final Component Statistics:");
        debug!("  Parser Pool - Created: {}, Reused: {}, Pool Size: {}, Peak: {}", 
               created, reused, pool_size, peak);
        
        let cache = get_global_query_cache();
        let cache_stats = cache.stats();
        debug!("  Query Cache - {}", cache_stats.summary());
        
        // Validate resource efficiency
        let cpu_count = num_cpus::get();
        assert!(created <= cpu_count * 3, "Parser creation should be limited (created: {}, CPU: {})", created, cpu_count);
        
        if created + reused > 0 {
            let reuse_ratio = reused as f64 / (created + reused) as f64;
            assert!(reuse_ratio > 0.6, "Parser reuse ratio should be >60% under load, got {:.1}%", reuse_ratio * 100.0);
        }
        
        info!("✅ All concurrent integration tests passed!");
    }
    
    /// Test memory efficiency under load
    #[test]
    fn test_memory_efficiency() {
        debug!("🧪 Testing memory efficiency of SQL engine components...");
        
        let initial_cache_memory = get_global_query_cache().get_total_memory_usage();
        
        // Generate load with many different queries
        let start = Instant::now();
        for i in 0..500 { // Moderate load
            // Parse query
            let query = format!("SELECT * FROM test_{} WHERE id > {} LIMIT {}", 
                              i % 50, i, i % 20 + 1);
            let result = parse_sql_global(query.clone());
            assert!(result.is_ok());
            
            // Cache some results (not all to test eviction)
            if i % 3 == 0 {
                let key = QueryCacheKey::new(&format!("SELECT * FROM cached_{}", i), 
                                           &format!("collection_{}", i % 10), None);
                let result_data = format!("cached_result_{}", i).into_bytes();
                cache_query_result(key, result_data, format!("SELECT * FROM cached_{}", i));
            }
        }
        
        let elapsed = start.elapsed();
        let final_cache_memory = get_global_query_cache().get_total_memory_usage();
        
        info!("✅ Processed 500 operations in {:?}", elapsed);
        debug!("📊 Memory usage - Initial: {}KB, Final: {}KB, Delta: {}KB",
               initial_cache_memory / 1024,
               final_cache_memory / 1024,
               (final_cache_memory.saturating_sub(initial_cache_memory)) / 1024);
        
        // Check component resource usage
        let pool = get_global_pool();
        let pool_stats = pool.get_stats();
        let (created, reused, pool_size, _) = pool_stats.get_stats();
        
        let cache = get_global_query_cache();
        let cache_size = cache.size();
        
        debug!("📊 Resource Usage Summary:");
        debug!("  Parser instances created: {}", created);
        debug!("  Current parser pool size: {}", pool_size);
        debug!("  Query cache entries: {}", cache_size);
        debug!("  Cache memory usage: {}KB", final_cache_memory / 1024);
        
        // Resource usage should be reasonable
        let cpu_count = num_cpus::get();
        assert!(created <= cpu_count * 4, "Parser creation should be conservative");
        assert!(pool_size <= 32, "Parser pool should not grow too large");
        assert!(cache_size <= 1000, "Cache should respect size limits");
        assert!(final_cache_memory < 10 * 1024 * 1024, "Cache memory should be <10MB");
        
        // Reuse ratio should be excellent
        if created + reused > 0 {
            let reuse_ratio = reused as f64 / (created + reused) as f64;
            debug!("  Parser reuse ratio: {:.1}%", reuse_ratio * 100.0);
            assert!(reuse_ratio > 0.8, "Should have >80% reuse ratio under sustained load");
        }
        
        info!("✅ Memory efficiency test passed!");
    }
    
    /// Benchmark complete SQL engine performance
    #[test]
    fn test_performance_benchmark() {
        debug!("🧪 Benchmarking complete SQL engine performance...");
        
        // Warm up all components
        let pool = get_global_pool();
        pool.warmup(num_cpus::get());
        
        // Pre-populate cache with some common queries
        for i in 0..10 {
            let key = QueryCacheKey::new(&format!("SELECT * FROM common_{}", i), "collection", None);
            let result_data = format!("common_result_{}", i).into_bytes();
            cache_query_result(key, result_data, format!("SELECT * FROM common_{}", i));
        }
        
        // TODO: Add GPU benchmark tests when GPU support is implemented
        // This would test:
        // 1. GPU batch parsing performance vs CPU
        // 2. Optimal batch sizes for different GPU architectures
        // 3. GPU memory transfer overhead analysis
        // 4. Multi-GPU scaling tests
        //
        // Example GPU benchmark:
        // #[test]
        // fn test_gpu_batch_parsing_benchmark() {
        //     let batch_sizes = vec![10, 100, 1000, 10000];
        //     for size in batch_sizes {
        //         let queries: Vec<String> = (0..size)
        //             .map(|i| format!("SELECT vector FROM embeddings WHERE id = {} ORDER BY VECTOR_SIMILARITY(vector, [{}], 'cosine') LIMIT 10", 
        //                  i, (0..1024).map(|j| format!("{}", j as f32 * 0.001)).collect::<Vec<_>>().join(", ")))
        //             .collect();
        //         
        //         // Benchmark CPU batch parsing
        //         let cpu_start = Instant::now();
        //         let cpu_results = pool.parse_sql_batch_cpu(queries.clone());
        //         let cpu_elapsed = cpu_start.elapsed();
        //         
        //         // Benchmark GPU batch parsing
        //         let gpu_start = Instant::now();
        //         let gpu_results = pool.parse_sql_batch_gpu(queries);
        //         let gpu_elapsed = gpu_start.elapsed();
        //         
        //         debug!("Batch size {}: CPU {:?}, GPU {:?}, Speedup: {:.2}x", 
        //                  size, cpu_elapsed, gpu_elapsed, 
        //                  cpu_elapsed.as_secs_f64() / gpu_elapsed.as_secs_f64());
        //     }
        // }
        
        let benchmark_queries = vec![
            "SELECT * FROM products LIMIT 10",
            "SELECT id, name FROM users WHERE metadata.active = true",
            "SELECT vector FROM embeddings ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.3, 0.4], 'cosine') LIMIT 5",
            "SELECT metadata FROM docs WHERE metadata.category IN ('tech', 'science')",
            "SELECT * FROM common_5", // Should be cache hit
        ];
        
        let iterations = 1000;
        let start = Instant::now();
        
        // Benchmark parsing performance
        for i in 0..iterations {
            for query in &benchmark_queries {
                let result = parse_sql_global(query.to_string());
                assert!(result.is_ok(), "Benchmark query failed: {}", query);
            }
        }
        
        let elapsed = start.elapsed();
        let total_queries = iterations * benchmark_queries.len();
        let throughput = total_queries as f64 / elapsed.as_secs_f64();
        
        debug!("🚀 Performance Benchmark Results:");
        debug!("  Total queries: {}", total_queries);
        debug!("  Duration: {:?}", elapsed);
        debug!("  Throughput: {:.0} queries/sec", throughput);
        debug!("  Average latency: {:.2}μs per query", 
               elapsed.as_micros() as f64 / total_queries as f64);
        
        // Performance should be excellent
        assert!(throughput > 10000.0, "Throughput should be >10K queries/sec, got {:.0}", throughput);
        
        // Check final statistics
        let pool_stats = pool.get_stats();
        let (created, reused, _, _) = pool_stats.get_stats();
        
        let cache = get_global_query_cache();
        let cache_stats = cache.stats();
        
        debug!("📊 Final Performance Statistics:");
        debug!("  Parser Pool - Created: {}, Reused: {}", created, reused);
        debug!("  Query Cache - {}", cache_stats.summary());
        
        if created + reused > 0 {
            let reuse_ratio = reused as f64 / (created + reused) as f64;
            debug!("  Parser reuse efficiency: {:.1}%", reuse_ratio * 100.0);
        }
        
        info!("✅ Performance benchmark completed successfully!");
    }
}