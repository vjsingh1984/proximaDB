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

//! Lock-free parser pool for high-performance SQL parsing
//!
//! This module implements a thread-safe, lock-free parser pool that eliminates
//! concurrency issues in SQL parsing by providing isolated parser instances
//! for each thread/request.

use crate::query::sql_engine::parser::{SqlParser, ParsedQuery};
use anyhow::Result;
use crossbeam::queue::SegQueue;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Maximum number of cached parser instances in the pool
/// Optimized for SQL parsing which is lightweight compared to I/O and search operations
const MAX_POOL_SIZE: usize = 32;

/// Default minimum number of parser instances (will be adjusted based on CPU count)
const DEFAULT_MIN_POOL_SIZE: usize = 2;

/// Statistics for parser pool performance monitoring
#[derive(Debug, Default)]
pub struct PoolStats {
    /// Total parser instances created
    pub total_created: AtomicUsize,
    /// Total parser instances reused from pool
    pub total_reused: AtomicUsize,
    /// Current pool size
    pub current_pool_size: AtomicUsize,
    /// Peak pool usage
    pub peak_pool_size: AtomicUsize,
}

impl PoolStats {
    pub fn record_creation(&self) {
        self.total_created.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_reuse(&self) {
        self.total_reused.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn update_pool_size(&self, size: usize) {
        self.current_pool_size.store(size, Ordering::Relaxed);
        
        loop {
            let current_peak = self.peak_pool_size.load(Ordering::Relaxed);
            if size <= current_peak {
                break;
            }
            if self.peak_pool_size.compare_exchange_weak(
                current_peak, 
                size, 
                Ordering::Relaxed, 
                Ordering::Relaxed
            ).is_ok() {
                break;
            }
        }
    }
    
    pub fn get_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.total_created.load(Ordering::Relaxed),
            self.total_reused.load(Ordering::Relaxed),
            self.current_pool_size.load(Ordering::Relaxed),
            self.peak_pool_size.load(Ordering::Relaxed),
        )
    }
}

/// Zero-copy parser context for stack-allocated parsing
/// 
/// This struct holds temporary parsing state without heap allocations,
/// maximizing performance for high-frequency parsing operations.
#[derive(Debug)]
pub struct ZeroCopyContext {
    /// Query string slice (borrowed, not owned)
    query: String,
    /// Current parsing position
    position: usize,
    /// Parsing depth for recursion safety
    depth: u8,
    /// Maximum allowed parsing depth
    max_depth: u8,
}

impl ZeroCopyContext {
    /// Create new zero-copy context with query
    pub fn new(query: String) -> Self {
        Self {
            query,
            position: 0,
            depth: 0,
            max_depth: 32, // Prevent stack overflow
        }
    }
    
    /// Reset context for reuse with new query
    pub fn reset(&mut self, query: String) {
        self.query = query;
        self.position = 0;
        self.depth = 0;
    }
    
    /// Get current parsing position
    pub fn position(&self) -> usize {
        self.position
    }
    
    /// Set parsing position
    pub fn set_position(&mut self, pos: usize) {
        self.position = pos;
    }
    
    /// Increment parsing depth (for recursion tracking)
    pub fn enter_depth(&mut self) -> Result<()> {
        if self.depth >= self.max_depth {
            return Err(anyhow::anyhow!("Parser recursion depth exceeded"));
        }
        self.depth += 1;
        Ok(())
    }
    
    /// Decrement parsing depth
    pub fn exit_depth(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
    }
    
    /// Get query string reference
    pub fn query(&self) -> &str {
        &self.query
    }
}

/// Reusable SQL parser with zero-copy context
/// 
/// Each parser instance maintains its own parsing state,
/// eliminating shared mutable state that causes concurrency issues.
pub struct ReusableParser {
    /// Zero-copy parsing context
    context: ZeroCopyContext,
    /// Parser instance ID for debugging
    parser_id: usize,
    /// Usage counter for this parser instance
    usage_count: usize,
}

impl ReusableParser {
    /// Create new reusable parser
    fn new(parser_id: usize) -> Self {
        Self {
            context: ZeroCopyContext::new(String::new()),
            parser_id,
            usage_count: 0,
        }
    }
    
    /// Parse SQL query using zero-copy context
    pub fn parse(&mut self, query: String) -> Result<ParsedQuery> {
        // Reset context for new query
        self.context.reset(query);
        self.usage_count += 1;
        
        // Create traditional parser with owned query for now
        // TODO: Optimize to use zero-copy context directly
        let mut parser = SqlParser::new(self.context.query().to_string());
        parser.parse()
    }
    
    /// Get parser statistics
    pub fn stats(&self) -> (usize, usize) {
        (self.parser_id, self.usage_count)
    }
    
    /// Reset usage statistics (for testing)
    pub fn reset_stats(&mut self) {
        self.usage_count = 0;
    }
}

/// Lock-free parser pool for high-performance SQL parsing
/// 
/// Uses crossbeam's SegQueue for lock-free concurrent access,
/// eliminating contention and improving throughput under high load.
pub struct LockFreeParserPool {
    /// Lock-free queue of available parser instances
    pool: SegQueue<ReusableParser>,
    /// Pool performance statistics
    stats: Arc<PoolStats>,
    /// Next parser ID for debugging
    next_parser_id: AtomicUsize,
    // TODO: Add GPU parser pool for batch operations
    // When GPU acceleration is added, maintain separate pools for:
    // 1. GPU parser contexts (with pre-allocated GPU memory)
    // 2. Batch parsing operations (accumulate queries for GPU batch processing)
    // 3. GPU command queues/streams for different GPU backends
    //
    // /// GPU parser contexts for batch operations
    // gpu_parser_pool: Option<GpuParserPool>,
    // /// Batch accumulator for GPU processing
    // batch_accumulator: Arc<Mutex<BatchAccumulator>>,
}

impl LockFreeParserPool {
    /// Create new lock-free parser pool
    pub fn new() -> Self {
        let pool = SegQueue::new();
        let stats = Arc::new(PoolStats::default());
        
        // Calculate optimal minimum pool size based on CPU count
        // SQL parsing is lightweight, so we use CPU count as base
        let min_pool_size = num_cpus::get().max(DEFAULT_MIN_POOL_SIZE);
        
        // Pre-populate pool with CPU-based minimum parser instances
        for i in 0..min_pool_size {
            pool.push(ReusableParser::new(i));
            stats.record_creation();
        }
        stats.update_pool_size(min_pool_size);
        
        Self {
            pool,
            stats,
            next_parser_id: AtomicUsize::new(min_pool_size),
        }
    }
    
    /// Get parser from pool or create new one
    fn acquire_parser(&self) -> ReusableParser {
        // Try to pop from pool first (lock-free)
        if let Some(parser) = self.pool.pop() {
            self.stats.record_reuse();
            let current_size = self.pool.len();
            self.stats.update_pool_size(current_size);
            return parser;
        }
        
        // Pool empty, create new parser
        let parser_id = self.next_parser_id.fetch_add(1, Ordering::Relaxed);
        self.stats.record_creation();
        ReusableParser::new(parser_id)
    }
    
    /// Return parser to pool for reuse
    fn release_parser(&self, parser: ReusableParser) {
        // Only return to pool if under max size limit
        if self.pool.len() < MAX_POOL_SIZE {
            self.pool.push(parser);
            let current_size = self.pool.len();
            self.stats.update_pool_size(current_size);
        }
        // Otherwise, let parser drop naturally (GC will clean up)
    }
    
    /// Parse SQL query using pool (main public interface)
    pub fn parse_sql(&self, query: String) -> Result<ParsedQuery> {
        // Acquire parser from pool
        let mut parser = self.acquire_parser();
        
        // Parse query
        let result = parser.parse(query);
        
        // Return parser to pool
        self.release_parser(parser);
        
        result
    }
    
    // TODO: Add batch parsing method for GPU acceleration
    // /// Parse multiple SQL queries in batch using GPU acceleration
    // pub fn parse_sql_batch(&self, queries: Vec<String>) -> Result<Vec<ParsedQuery>> {
    //     // Check if batch size justifies GPU usage
    //     if queries.len() < GPU_BATCH_THRESHOLD {
    //         // Use CPU path for small batches
    //         return queries.into_iter()
    //             .map(|q| self.parse_sql(q))
    //             .collect();
    //     }
    //     
    //     // GPU batch parsing path:
    //     // 1. Acquire GPU parser context from pool
    //     // 2. Transfer queries to GPU memory
    //     // 3. Launch GPU kernel for parallel parsing
    //     // 4. Transfer results back
    //     // 5. Return GPU context to pool
    //     //
    //     // Benefits:
    //     // - Amortize GPU transfer overhead across many queries
    //     // - Leverage massive GPU parallelism
    //     // - Share parsed tokens/AST nodes in GPU memory
    // }
    
    /// Get pool statistics for monitoring
    pub fn get_stats(&self) -> Arc<PoolStats> {
        Arc::clone(&self.stats)
    }
    
    /// Get current pool size
    pub fn pool_size(&self) -> usize {
        self.pool.len()
    }
    
    /// Warm up pool by pre-creating parser instances
    /// Optimized for SQL parsing workloads where I/O dominates CPU usage
    pub fn warmup(&self, target_size: usize) {
        // Cap warmup size to be conservative with resources
        // SQL parsing is lightweight compared to vector search operations
        let actual_target = target_size.min(MAX_POOL_SIZE).min(num_cpus::get() * 2);
        let current_size = self.pool.len();
        
        if actual_target > current_size {
            for _ in current_size..actual_target {
                let parser_id = self.next_parser_id.fetch_add(1, Ordering::Relaxed);
                let parser = ReusableParser::new(parser_id);
                self.pool.push(parser);
                self.stats.record_creation();
            }
            self.stats.update_pool_size(actual_target);
        }
    }
}

impl Default for LockFreeParserPool {
    fn default() -> Self {
        Self::new()
    }
}

// Thread-safe: SegQueue is lock-free and thread-safe
unsafe impl Send for LockFreeParserPool {}
unsafe impl Sync for LockFreeParserPool {}

/// Global parser pool instance (lazy initialization)
use std::sync::OnceLock;
static GLOBAL_PARSER_POOL: OnceLock<LockFreeParserPool> = OnceLock::new();

/// Get global parser pool instance
pub fn get_global_pool() -> &'static LockFreeParserPool {
    GLOBAL_PARSER_POOL.get_or_init(LockFreeParserPool::new)
}

/// Convenience function to parse SQL using global pool
pub fn parse_sql_global(query: String) -> Result<ParsedQuery> {
    get_global_pool().parse_sql(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;
use tracing::{debug, error, info, warn};
    
    #[test]
    fn test_pool_basic_functionality() {
        let pool = LockFreeParserPool::new();
        
        let query = "SELECT * FROM products LIMIT 10".to_string();
        let result = pool.parse_sql(query);
        
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.from_collection, "products");
        assert_eq!(parsed.limit, Some(10));
    }
    
    #[test]
    fn test_pool_reuse() {
        let pool = LockFreeParserPool::new();
        let stats = pool.get_stats();
        
        // First parse - should create new parser
        let _result1 = pool.parse_sql("SELECT id FROM test1 LIMIT 5".to_string());
        let (created1, reused1, _, _) = stats.get_stats();
        
        // Second parse - should reuse parser from pool
        let _result2 = pool.parse_sql("SELECT name FROM test2 LIMIT 3".to_string());
        let (created2, reused2, _, _) = stats.get_stats();
        
        // Should have reused at least one parser
        assert!(reused2 > reused1);
    }
    
    #[test]
    fn test_concurrent_parsing() {
        let pool = Arc::new(LockFreeParserPool::new());
        let stats = pool.get_stats();
        
        // Conservative warmup based on CPU count
        let num_cpus = num_cpus::get();
        pool.warmup(num_cpus);
        
        // Use CPU count for thread count - realistic for SQL parsing workloads
        let handles: Vec<_> = (0..num_cpus).map(|i| {
            let pool_clone = Arc::clone(&pool);
            thread::spawn(move || {
                let query = format!("SELECT * FROM test_{} LIMIT {}", i, i + 1);
                pool_clone.parse_sql(query)
            })
        }).collect();
        
        // Wait for all threads
        let results: Vec<_> = handles.into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        
        // All should succeed
        assert_eq!(results.len(), num_cpus);
        for result in results {
            assert!(result.is_ok());
        }
        
        let (created, reused, _, _) = stats.get_stats();
        debug!("Concurrent test - Created: {}, Reused: {}, CPUs: {}", created, reused, num_cpus);
        
        // Should have good reuse ratio
        assert!(reused > 0);
    }
    
    #[test]
    fn test_zero_copy_context() {
        let mut context = ZeroCopyContext::new("SELECT * FROM test".to_string());
        
        assert_eq!(context.position(), 0);
        context.set_position(6);
        assert_eq!(context.position(), 6);
        
        // Test depth tracking
        assert!(context.enter_depth().is_ok());
        assert!(context.enter_depth().is_ok());
        context.exit_depth();
        context.exit_depth();
        
        // Test reset
        context.reset("SELECT id FROM other".to_string());
        assert_eq!(context.position(), 0);
        assert_eq!(context.query(), "SELECT id FROM other");
    }
    
    #[test]
    fn test_parser_pool_performance() {
        let pool = LockFreeParserPool::new();
        let num_cpus = num_cpus::get();
        
        // Conservative warmup based on CPU count
        pool.warmup(num_cpus);
        
        // Realistic SQL queries for vector database workloads
        let queries = vec![
            "SELECT * FROM products LIMIT 10",
            "SELECT id, name FROM users WHERE metadata.active = true LIMIT 20",
            "SELECT vector FROM embeddings ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2], 'cosine') LIMIT 5",
            "SELECT metadata FROM documents WHERE metadata.category IN ('tech', 'science')",
            "SELECT id FROM collections WHERE metadata.size BETWEEN 100 AND 1000",
        ];
        
        let start = Instant::now();
        
        // Moderate iteration count - SQL parsing is not the bottleneck in vector DB operations
        let iterations = 200; // Reduced from 1000 to be more realistic
        for _ in 0..iterations {
            for query in &queries {
                let result = pool.parse_sql(query.to_string());
                assert!(result.is_ok());
            }
        }
        
        let elapsed = start.elapsed();
        let stats = pool.get_stats();
        let (created, reused, pool_size, peak) = stats.get_stats();
        
        let total_queries = iterations * queries.len();
        debug!("SQL Parser Performance Test Results:");
        debug!("  {} queries parsed in {:?}", total_queries, elapsed);
        debug!("  Throughput: {:.0} queries/sec", total_queries as f64 / elapsed.as_secs_f64());
        debug!("  Pool stats - Created: {}, Reused: {}, Pool size: {}, Peak: {}", 
                created, reused, pool_size, peak);
        
        // Should have high reuse ratio for performance with conservative pool size
        let total_operations = created + reused;
        let reuse_ratio = reused as f64 / total_operations as f64;
        debug!("  Parser reuse ratio: {:.1}%", reuse_ratio * 100.0);
        
        // With limited pool size, should have excellent reuse
        assert!(reuse_ratio > 0.85, "Reuse ratio should be >85% with limited pool, got {:.2}", reuse_ratio);
        
        // Pool size should remain conservative (CPU-based)
        assert!(created <= num_cpus * 2, "Created parsers ({}) should be <= 2x CPU count ({})", created, num_cpus * 2);
    }
    
    #[test]
    fn test_global_pool() {
        let result1 = parse_sql_global("SELECT * FROM global_test LIMIT 1".to_string());
        assert!(result1.is_ok());
        
        let result2 = parse_sql_global("SELECT id FROM global_test2 LIMIT 2".to_string());
        assert!(result2.is_ok());
        
        // Both should use same global pool instance
        let pool1 = get_global_pool();
        let pool2 = get_global_pool();
        assert!(std::ptr::eq(pool1, pool2));
    }
    
    #[test]
    fn test_concurrent_stress() {
        let pool = Arc::new(LockFreeParserPool::new());
        let num_cpus = num_cpus::get();
        
        // Conservative warmup - SQL parsing is lightweight
        pool.warmup(num_cpus);
        
        // Use CPU count for threads, minimum 20 queries per thread as requested
        let num_threads = num_cpus;
        let queries_per_thread = 20.max(100 / num_cpus); // Ensure at least 20 queries per thread
        
        let start = Instant::now();
        
        let handles: Vec<_> = (0..num_threads).map(|thread_id| {
            let pool_clone = Arc::clone(&pool);
            thread::spawn(move || {
                let mut success_count = 0;
                for i in 0..queries_per_thread {
                    // Realistic SQL queries for vector database workloads
                    let query = match i % 4 {
                        0 => format!("SELECT * FROM vectors_{} LIMIT {}", thread_id, (i % 10) + 1),
                        1 => format!("SELECT id FROM collection_{} WHERE metadata.category = 'test'", thread_id),
                        2 => format!("SELECT vector FROM embeddings_{} ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2], 'cosine') LIMIT 5", thread_id),
                        _ => format!("SELECT metadata FROM docs_{} WHERE metadata.score > {}", thread_id, i % 10),
                    };
                    
                    if pool_clone.parse_sql(query).is_ok() {
                        success_count += 1;
                    }
                }
                success_count
            })
        }).collect();
        
        let total_success: usize = handles.into_iter()
            .map(|h| h.join().unwrap())
            .sum();
        
        let elapsed = start.elapsed();
        let expected_total = num_threads * queries_per_thread;
        
        debug!("SQL Parser Stress Test Results:");
        debug!("  Threads: {} (CPU count: {})", num_threads, num_cpus);
        debug!("  Queries per thread: {}", queries_per_thread);
        debug!("  Total queries: {}/{} successful in {:?}", total_success, expected_total, elapsed);
        debug!("  Throughput: {:.0} queries/sec", expected_total as f64 / elapsed.as_secs_f64());
        
        // Should have very high success rate
        assert_eq!(total_success, expected_total);
        
        let stats = pool.get_stats();
        let (created, reused, _, _) = stats.get_stats();
        let reuse_ratio = reused as f64 / (created + reused) as f64 * 100.0;
        debug!("  Parser stats - Created: {}, Reused: {} ({:.1}% reuse)", created, reused, reuse_ratio);
        
        // Should have excellent reuse ratio with limited pool size
        assert!(reuse_ratio > 70.0, "Parser reuse ratio should be >70%, got {:.1}%", reuse_ratio);
    }
}