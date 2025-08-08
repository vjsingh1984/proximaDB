use crate::storage::cache::backend::CacheTier;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Cache metrics collector
#[derive(Debug, Clone)]
pub struct CacheMetrics {
    // Hit/Miss counters per tier
    l1_hits: Arc<AtomicU64>,
    l2_hits: Arc<AtomicU64>,
    l3_hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    
    // Operation counters
    gets: Arc<AtomicU64>,
    puts: Arc<AtomicU64>,
    invalidations: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
    
    // Size metrics
    total_entries: Arc<AtomicUsize>,
    total_bytes: Arc<AtomicUsize>,
    
    // Latency tracking (in microseconds)
    get_latency_sum: Arc<AtomicU64>,
    get_latency_count: Arc<AtomicU64>,
    put_latency_sum: Arc<AtomicU64>,
    put_latency_count: Arc<AtomicU64>,
    
    // Start time for rate calculations
    start_time: Instant,
}

impl CacheMetrics {
    pub fn new() -> Self {
        Self {
            l1_hits: Arc::new(AtomicU64::new(0)),
            l2_hits: Arc::new(AtomicU64::new(0)),
            l3_hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            gets: Arc::new(AtomicU64::new(0)),
            puts: Arc::new(AtomicU64::new(0)),
            invalidations: Arc::new(AtomicU64::new(0)),
            evictions: Arc::new(AtomicU64::new(0)),
            total_entries: Arc::new(AtomicUsize::new(0)),
            total_bytes: Arc::new(AtomicUsize::new(0)),
            get_latency_sum: Arc::new(AtomicU64::new(0)),
            get_latency_count: Arc::new(AtomicU64::new(0)),
            put_latency_sum: Arc::new(AtomicU64::new(0)),
            put_latency_count: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
        }
    }
    
    pub fn record_hit(&self, tier: CacheTier) {
        match tier {
            CacheTier::L1 => self.l1_hits.fetch_add(1, Ordering::Relaxed),
            CacheTier::L2 => self.l2_hits.fetch_add(1, Ordering::Relaxed),
            CacheTier::L3 => self.l3_hits.fetch_add(1, Ordering::Relaxed),
        };
        self.gets.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.gets.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_put(&self) {
        self.puts.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_invalidation(&self) {
        self.invalidations.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_get_latency(&self, duration: Duration) {
        let micros = duration.as_micros() as u64;
        self.get_latency_sum.fetch_add(micros, Ordering::Relaxed);
        self.get_latency_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_put_latency(&self, duration: Duration) {
        let micros = duration.as_micros() as u64;
        self.put_latency_sum.fetch_add(micros, Ordering::Relaxed);
        self.put_latency_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn update_size(&self, entries: usize, bytes: usize) {
        self.total_entries.store(entries, Ordering::Relaxed);
        self.total_bytes.store(bytes, Ordering::Relaxed);
    }
    
    pub fn snapshot(&self) -> CacheMetricsSnapshot {
        let total_hits = self.l1_hits.load(Ordering::Relaxed)
            + self.l2_hits.load(Ordering::Relaxed)
            + self.l3_hits.load(Ordering::Relaxed);
        let total_gets = self.gets.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        
        let hit_rate = if total_gets > 0 {
            (total_hits as f64) / (total_gets as f64)
        } else {
            0.0
        };
        
        let get_latency_count = self.get_latency_count.load(Ordering::Relaxed);
        let avg_get_latency_us = if get_latency_count > 0 {
            self.get_latency_sum.load(Ordering::Relaxed) / get_latency_count
        } else {
            0
        };
        
        let put_latency_count = self.put_latency_count.load(Ordering::Relaxed);
        let avg_put_latency_us = if put_latency_count > 0 {
            self.put_latency_sum.load(Ordering::Relaxed) / put_latency_count
        } else {
            0
        };
        
        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        let gets_per_sec = if elapsed_secs > 0.0 {
            (total_gets as f64) / elapsed_secs
        } else {
            0.0
        };
        
        CacheMetricsSnapshot {
            l1_hits: self.l1_hits.load(Ordering::Relaxed),
            l2_hits: self.l2_hits.load(Ordering::Relaxed),
            l3_hits: self.l3_hits.load(Ordering::Relaxed),
            misses,
            total_gets,
            total_puts: self.puts.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            hit_rate,
            total_entries: self.total_entries.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            avg_get_latency_us,
            avg_put_latency_us,
            gets_per_sec,
            uptime_secs: elapsed_secs as u64,
        }
    }
    
    pub fn reset(&self) {
        self.l1_hits.store(0, Ordering::Relaxed);
        self.l2_hits.store(0, Ordering::Relaxed);
        self.l3_hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.gets.store(0, Ordering::Relaxed);
        self.puts.store(0, Ordering::Relaxed);
        self.invalidations.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.get_latency_sum.store(0, Ordering::Relaxed);
        self.get_latency_count.store(0, Ordering::Relaxed);
        self.put_latency_sum.store(0, Ordering::Relaxed);
        self.put_latency_count.store(0, Ordering::Relaxed);
    }
}

impl Default for CacheMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of cache metrics at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetricsSnapshot {
    pub l1_hits: u64,
    pub l2_hits: u64,
    pub l3_hits: u64,
    pub misses: u64,
    pub total_gets: u64,
    pub total_puts: u64,
    pub invalidations: u64,
    pub evictions: u64,
    pub hit_rate: f64,
    pub total_entries: usize,
    pub total_bytes: usize,
    pub avg_get_latency_us: u64,
    pub avg_put_latency_us: u64,
    pub gets_per_sec: f64,
    pub uptime_secs: u64,
}

impl CacheMetricsSnapshot {
    pub fn print_summary(&self) {
        println!("=== Cache Metrics Summary ===");
        println!("Hit Rate: {:.2}%", self.hit_rate * 100.0);
        println!("L1 Hits: {} ({:.1}%)", self.l1_hits, 
                 (self.l1_hits as f64 / self.total_gets.max(1) as f64) * 100.0);
        println!("L2 Hits: {} ({:.1}%)", self.l2_hits,
                 (self.l2_hits as f64 / self.total_gets.max(1) as f64) * 100.0);
        println!("L3 Hits: {} ({:.1}%)", self.l3_hits,
                 (self.l3_hits as f64 / self.total_gets.max(1) as f64) * 100.0);
        println!("Misses: {} ({:.1}%)", self.misses,
                 (self.misses as f64 / self.total_gets.max(1) as f64) * 100.0);
        println!("Total Gets: {}", self.total_gets);
        println!("Total Puts: {}", self.total_puts);
        println!("Evictions: {}", self.evictions);
        println!("Invalidations: {}", self.invalidations);
        println!("Entries: {}", self.total_entries);
        println!("Size: {} MB", self.total_bytes / (1024 * 1024));
        println!("Avg Get Latency: {} μs", self.avg_get_latency_us);
        println!("Avg Put Latency: {} μs", self.avg_put_latency_us);
        println!("Gets/sec: {:.1}", self.gets_per_sec);
        println!("Uptime: {} seconds", self.uptime_secs);
    }
}