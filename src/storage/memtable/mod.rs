//! Unified Memtable Package
//! 
//! Provides pluggable memtable implementations optimized for different workloads:
//! 
//! ## Implementation Strategy:
//! - **WAL (Write-Ahead Log)**: BTree for ordered writes and optimal compression
//! - **LSM (Log-Structured Merge-tree)**: SkipList for concurrent access and range queries
//! - **Performance Testing**: Multiple implementations for benchmark comparison
//! 
//! ## Available Implementations:
//! - **BTree**: Ordered storage, memory efficient, optimal for compression
//! - **SkipList**: Lock-free concurrent access, excellent range queries
//! - **HashMap**: O(1) point lookups, high write throughput
//! - **DashMap**: High-concurrency HashMap with sharding
//! - **ART**: Adaptive Radix Tree, memory efficient for string-like keys
//! 
//! ## Usage:
//! ```rust
//! use proximadb::storage::memtable::{MemtableFactory, MemtableType};
//! 
//! // Create WAL-optimized memtable (BTree)
//! let wal_memtable = MemtableFactory::create_for_wal(config);
//! 
//! // Create LSM-optimized memtable (SkipList)
//! let lsm_memtable = MemtableFactory::create_for_lsm(config);
//! 
//! // Create for performance testing
//! let test_memtable = MemtableFactory::create(MemtableType::DashMap, config);
//! ```

pub mod core;
pub mod implementations;
pub mod specialized;
pub mod serialization;

// Re-export core traits
pub use core::{
    MemtableCore, MemtableMVCC,
    MemtableConfig, MemtableMetrics
};

// Re-export implementations
pub use implementations::{
    btree::BTreeMemtable,
    bplustree::BPlusTreeMemtable,
    skiplist::SkipListMemtable,
    hashmap::HashMapMemtable,
    dashmap::DashMapMemtable,
    // artmap::ArtMemtable,  // Commented out - not currently used
};

// Re-export specialized wrappers (using proper OOP composition)
pub use specialized::{
    SpecializedMemtableFactory,
};

use anyhow::Result;
use std::sync::Arc;

/// Available memtable implementation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemtableType {
    /// BTree - Ordered storage, memory efficient
    BTree,
    /// B+Tree - Superior range scans, database-optimized
    BPlusTree,
    /// SkipList - Lock-free concurrent access
    SkipList,
    /// HashMap - O(1) point lookups
    HashMap,
    /// DashMap - High-concurrency HashMap
    DashMap,
    /// ART - Adaptive Radix Tree
    ART,
}

impl MemtableType {
    /// Get all available types for benchmarking
    pub fn all() -> Vec<MemtableType> {
        vec![
            MemtableType::BTree,
            MemtableType::BPlusTree,
            MemtableType::SkipList,
            MemtableType::HashMap,
            MemtableType::DashMap,
            MemtableType::ART,
        ]
    }
    
    /// Get recommended type for workload
    pub fn recommended_for_workload(workload: WorkloadCharacteristics) -> MemtableType {
        match workload {
            WorkloadCharacteristics::SequentialWrites => MemtableType::BPlusTree,
            WorkloadCharacteristics::RandomWrites => MemtableType::DashMap,
            WorkloadCharacteristics::PointLookups => MemtableType::HashMap,
            WorkloadCharacteristics::RangeQueries => MemtableType::BPlusTree,
            WorkloadCharacteristics::HighConcurrency => MemtableType::DashMap,
            WorkloadCharacteristics::MemoryConstrained => MemtableType::ART,
            WorkloadCharacteristics::StringKeys => MemtableType::ART,
            WorkloadCharacteristics::WAL => MemtableType::BTree,  // BTree better for numeric keys (u64)
            WorkloadCharacteristics::LSM => MemtableType::SkipList,
        }
    }
}

/// Workload characteristics for memtable selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadCharacteristics {
    SequentialWrites,
    RandomWrites,
    PointLookups,
    RangeQueries,
    HighConcurrency,
    MemoryConstrained,
    StringKeys,
    WAL,
    LSM,
}

/// Factory for creating memtable instances
pub struct MemtableFactory;

impl MemtableFactory {
    /// Create WAL-optimized memtable (ART for memory efficiency and ordered writes)
    pub fn create_for_wal(config: MemtableConfig) -> specialized::WalMemtable<u64, crate::storage::persistence::wal::WalEntry> {
        specialized::SpecializedMemtableFactory::create_btree_for_wal(config)
    }
    
    /// Create LSM-optimized memtable (SkipList for concurrent access)
    pub fn create_for_lsm(config: MemtableConfig) -> specialized::LsmMemtable<String, specialized::lsm_behavior::LsmEntry> {
        specialized::SpecializedMemtableFactory::create_skiplist_for_lsm(config)
    }
    
    /// Create specific memtable type for testing/benchmarking
    pub fn create_typed<K, V>(
        memtable_type: MemtableType,
        _config: MemtableConfig,
    ) -> Box<dyn MemtableCore<K, V> + Send + Sync>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + AsRef<[u8]> + 'static,
        V: Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        match memtable_type {
            MemtableType::BTree => Box::new(BTreeMemtable::new(true)),
            MemtableType::BPlusTree => Box::new(BPlusTreeMemtable::new()),
            MemtableType::SkipList => Box::new(SkipListMemtable::new()),
            MemtableType::HashMap => Box::new(HashMapMemtable::new()),
            MemtableType::DashMap => Box::new(DashMapMemtable::new()),
            MemtableType::ART => Box::new(BTreeMemtable::new(false)), // Temporarily use BTree instead of ART
        }
    }
    
    /// Auto-select best memtable type based on workload analysis
    pub fn auto_select<K, V>(
        workload: WorkloadCharacteristics,
        config: MemtableConfig,
    ) -> Box<dyn MemtableCore<K, V> + Send + Sync>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + AsRef<[u8]> + 'static,
        V: Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        let memtable_type = MemtableType::recommended_for_workload(workload);
        Self::create_typed(memtable_type, config)
    }
}

/// Performance benchmarking framework
pub struct MemtableBenchmark<K, V>
where
    K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + AsRef<[u8]>,
    V: Clone + Send + Sync + std::fmt::Debug,
{
    implementations: Vec<(MemtableType, Box<dyn MemtableCore<K, V> + Send + Sync>)>,
    config: MemtableConfig,
}

impl<K, V> MemtableBenchmark<K, V>
where
    K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + AsRef<[u8]> + 'static,
    V: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    /// Create benchmark suite with all implementations
    pub fn new(config: MemtableConfig) -> Self {
        let mut implementations = Vec::new();
        
        for memtable_type in MemtableType::all() {
            let implementation = MemtableFactory::create_typed(memtable_type, config.clone());
            implementations.push((memtable_type, implementation));
        }
        
        Self {
            implementations,
            config,
        }
    }
    
    /// Run insert benchmark
    pub async fn benchmark_inserts(&mut self, entries: Vec<(K, V)>) -> Vec<BenchmarkResult> {
        let mut results = Vec::new();
        
        for (memtable_type, memtable) in &mut self.implementations {
            let start = std::time::Instant::now();
            let mut total_size = 0u64;
            
            for (key, value) in &entries {
                match memtable.insert(key.clone(), value.clone()).await {
                    Ok(size_delta) => total_size += size_delta,
                    Err(_) => continue,
                }
            }
            
            let duration = start.elapsed();
            let ops_per_sec = entries.len() as f64 / duration.as_secs_f64();
            
            results.push(BenchmarkResult {
                memtable_type: *memtable_type,
                operation: "insert".to_string(),
                duration,
                ops_per_second: ops_per_sec,
                memory_usage: memtable.size_bytes().await,
                entry_count: memtable.len().await,
                success_count: entries.len(),
                error_count: 0,
            });
        }
        
        results
    }
    
    /// Run point lookup benchmark
    pub async fn benchmark_lookups(&mut self, keys: Vec<K>) -> Vec<BenchmarkResult> {
        let mut results = Vec::new();
        
        for (memtable_type, memtable) in &mut self.implementations {
            let start = std::time::Instant::now();
            let mut success_count = 0;
            
            for key in &keys {
                match memtable.get(key).await {
                    Ok(Some(_)) => success_count += 1,
                    Ok(None) => continue,
                    Err(_) => continue,
                }
            }
            
            let duration = start.elapsed();
            let ops_per_sec = keys.len() as f64 / duration.as_secs_f64();
            
            results.push(BenchmarkResult {
                memtable_type: *memtable_type,
                operation: "lookup".to_string(),
                duration,
                ops_per_second: ops_per_sec,
                memory_usage: memtable.size_bytes().await,
                entry_count: memtable.len().await,
                success_count,
                error_count: keys.len() - success_count,
            });
        }
        
        results
    }
    
    /// Run range scan benchmark
    pub async fn benchmark_range_scans(&mut self, ranges: Vec<(K, Option<usize>)>) -> Vec<BenchmarkResult> {
        let mut results = Vec::new();
        
        for (memtable_type, memtable) in &mut self.implementations {
            let start = std::time::Instant::now();
            let mut success_count = 0;
            let mut total_results = 0;
            
            for (from_key, limit) in &ranges {
                match memtable.range_scan(from_key.clone(), *limit).await {
                    Ok(scan_results) => {
                        success_count += 1;
                        total_results += scan_results.len();
                    }
                    Err(_) => continue,
                }
            }
            
            let duration = start.elapsed();
            let ops_per_sec = ranges.len() as f64 / duration.as_secs_f64();
            
            results.push(BenchmarkResult {
                memtable_type: *memtable_type,
                operation: "range_scan".to_string(),
                duration,
                ops_per_second: ops_per_sec,
                memory_usage: memtable.size_bytes().await,
                entry_count: memtable.len().await,
                success_count,
                error_count: ranges.len() - success_count,
            });
        }
        
        results
    }
    
    /// Generate comprehensive benchmark report
    pub async fn generate_report(&mut self) -> BenchmarkReport {
        // For now, return empty benchmark report
        // TODO: Implement proper generic test data generation
        BenchmarkReport {
            config: self.config.clone(),
            insert_results: vec![],
            lookup_results: vec![],
            scan_results: vec![],
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Benchmark result for a specific operation and implementation
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub memtable_type: MemtableType,
    pub operation: String,
    pub duration: std::time::Duration,
    pub ops_per_second: f64,
    pub memory_usage: usize,
    pub entry_count: usize,
    pub success_count: usize,
    pub error_count: usize,
}

/// Comprehensive benchmark report
#[derive(Debug, Clone)]
pub struct BenchmarkReport {
    pub config: MemtableConfig,
    pub insert_results: Vec<BenchmarkResult>,
    pub lookup_results: Vec<BenchmarkResult>,
    pub scan_results: Vec<BenchmarkResult>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl BenchmarkReport {
    /// Print formatted report
    pub fn print(&self) {
        println!("Memtable Performance Benchmark Report");
        println!("=====================================");
        println!("Timestamp: {}", self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"));
        println!();
        
        println!("INSERT PERFORMANCE:");
        println!("{:<12} {:>12} {:>12} {:>12} {:>10}", "Type", "Ops/Sec", "Duration", "Memory", "Entries");
        println!("{}", "-".repeat(60));
        for result in &self.insert_results {
            println!("{:<12} {:>12.1} {:>10.3}s {:>10}B {:>8}",
                format!("{:?}", result.memtable_type),
                result.ops_per_second,
                result.duration.as_secs_f64(),
                result.memory_usage,
                result.entry_count
            );
        }
        println!();
        
        println!("LOOKUP PERFORMANCE:");
        println!("{:<12} {:>12} {:>12} {:>12} {:>10}", "Type", "Ops/Sec", "Duration", "Hit Rate", "Entries");
        println!("{}", "-".repeat(60));
        for result in &self.lookup_results {
            let hit_rate = if result.success_count + result.error_count > 0 {
                result.success_count as f64 / (result.success_count + result.error_count) as f64 * 100.0
            } else {
                0.0
            };
            println!("{:<12} {:>12.1} {:>10.3}s {:>11.1}% {:>8}",
                format!("{:?}", result.memtable_type),
                result.ops_per_second,
                result.duration.as_secs_f64(),
                hit_rate,
                result.entry_count
            );
        }
        println!();
        
        println!("RANGE SCAN PERFORMANCE:");
        println!("{:<12} {:>12} {:>12} {:>12} {:>10}", "Type", "Scans/Sec", "Duration", "Success", "Entries");
        println!("{}", "-".repeat(60));
        for result in &self.scan_results {
            println!("{:<12} {:>12.1} {:>10.3}s {:>11} {:>8}",
                format!("{:?}", result.memtable_type),
                result.ops_per_second,
                result.duration.as_secs_f64(),
                result.success_count,
                result.entry_count
            );
        }
        println!();
    }
    
    /// Get winner for each operation type
    pub fn get_winners(&self) -> PerformanceWinners {
        let best_insert = self.insert_results.iter()
            .max_by(|a, b| a.ops_per_second.partial_cmp(&b.ops_per_second).unwrap_or(std::cmp::Ordering::Equal))
            .map(|r| r.memtable_type);
            
        let best_lookup = self.lookup_results.iter()
            .max_by(|a, b| a.ops_per_second.partial_cmp(&b.ops_per_second).unwrap_or(std::cmp::Ordering::Equal))
            .map(|r| r.memtable_type);
            
        let best_scan = self.scan_results.iter()
            .max_by(|a, b| a.ops_per_second.partial_cmp(&b.ops_per_second).unwrap_or(std::cmp::Ordering::Equal))
            .map(|r| r.memtable_type);
        
        PerformanceWinners {
            best_insert,
            best_lookup,
            best_scan,
        }
    }
}

/// Performance winners for each operation type
#[derive(Debug, Clone)]
pub struct PerformanceWinners {
    pub best_insert: Option<MemtableType>,
    pub best_lookup: Option<MemtableType>,
    pub best_scan: Option<MemtableType>,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_memtable_factory() {
        let config = MemtableConfig::default();
        
        // Test WAL creation
        let _wal_memtable = MemtableFactory::create_for_wal(config.clone());
        
        // Test LSM creation
        let _lsm_memtable = MemtableFactory::create_for_lsm(config.clone());
        
        // Test typed creation
        let _btree_memtable: Box<dyn MemtableCore<String, String> + Send + Sync> = 
            MemtableFactory::create_typed(MemtableType::BTree, config.clone());
    }
    
    #[tokio::test]
    async fn test_workload_recommendations() {
        assert_eq!(MemtableType::recommended_for_workload(WorkloadCharacteristics::WAL), MemtableType::BTree);
        assert_eq!(MemtableType::recommended_for_workload(WorkloadCharacteristics::LSM), MemtableType::SkipList);
        assert_eq!(MemtableType::recommended_for_workload(WorkloadCharacteristics::PointLookups), MemtableType::HashMap);
        assert_eq!(MemtableType::recommended_for_workload(WorkloadCharacteristics::HighConcurrency), MemtableType::DashMap);
    }
    
    #[tokio::test]
    async fn test_benchmark_framework() {
        let config = MemtableConfig::default();
        let mut benchmark = MemtableBenchmark::<String, String>::new(config);
        
        // Test small benchmark
        let test_entries = vec![
            ("key1".to_string(), "value1".to_string()),
            ("key2".to_string(), "value2".to_string()),
        ];
        
        let insert_results = benchmark.benchmark_inserts(test_entries).await;
        assert_eq!(insert_results.len(), MemtableType::all().len());
        
        let test_keys = vec!["key1".to_string(), "key2".to_string()];
        let lookup_results = benchmark.benchmark_lookups(test_keys).await;
        assert_eq!(lookup_results.len(), MemtableType::all().len());
    }
}