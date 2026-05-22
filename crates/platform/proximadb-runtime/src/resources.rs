//! # Resource Management
//!
//! Memory and CPU resource allocation and monitoring.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Resource manager for tracking system resource usage
pub struct ResourceManager {
    total_memory_mb: usize,
    available_memory_mb: Arc<AtomicU64>,
    cpu_count: usize,
}

impl ResourceManager {
    pub fn new() -> Self {
        let sys = sysinfo::System::new_all();
        let total = sys.total_memory() / 1024 / 1024;
        Self {
            total_memory_mb: total as usize,
            available_memory_mb: Arc::new(AtomicU64::new(total)),
            cpu_count: num_cpus::get(),
        }
    }

    pub fn total_memory_mb(&self) -> usize {
        self.total_memory_mb
    }

    pub fn available_memory_mb(&self) -> u64 {
        self.available_memory_mb.load(Ordering::Relaxed)
    }

    pub fn cpu_count(&self) -> usize {
        self.cpu_count
    }

    /// Update available memory (should be called periodically)
    pub fn refresh_memory(&self) {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let available = sys.available_memory() / 1024 / 1024;
        self.available_memory_mb.store(available, Ordering::Relaxed);
    }

    /// Calculate recommended thread count for a workload
    pub fn recommended_threads(&self, memory_per_thread_mb: usize) -> usize {
        let memory_limited = self.available_memory_mb() as usize / memory_per_thread_mb.max(1);
        std::cmp::min(self.cpu_count, memory_limited.max(1))
    }
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Memory budget allocator
pub struct MemoryBudget {
    total_mb: usize,
    reserved_mb: usize,
}

impl MemoryBudget {
    pub fn new(total_mb: usize) -> Self {
        Self {
            total_mb,
            reserved_mb: 0,
        }
    }

    pub fn with_reserve(mut self, reserve_mb: usize) -> Self {
        self.reserved_mb = reserve_mb;
        self
    }

    pub fn available(&self) -> usize {
        self.total_mb.saturating_sub(self.reserved_mb)
    }

    pub fn allocate(&mut self, amount_mb: usize) -> bool {
        if self.available() >= amount_mb {
            self.reserved_mb += amount_mb;
            true
        } else {
            false
        }
    }

    pub fn release(&mut self, amount_mb: usize) {
        self.reserved_mb = self.reserved_mb.saturating_sub(amount_mb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_budget() {
        let mut budget = MemoryBudget::new(1000);
        assert_eq!(budget.available(), 1000);

        assert!(budget.allocate(200));
        assert_eq!(budget.available(), 800);

        assert!(!budget.allocate(900));
        assert_eq!(budget.available(), 800);

        budget.release(100);
        assert_eq!(budget.available(), 900);
    }

    #[test]
    fn memory_budget_reserve_and_saturating_release_are_stable() {
        let mut budget = MemoryBudget::new(512).with_reserve(128);
        assert_eq!(budget.available(), 384);
        assert!(budget.allocate(384));
        assert_eq!(budget.available(), 0);
        assert!(!budget.allocate(1));
        budget.release(1_000);
        assert_eq!(budget.available(), 512);
    }

    #[test]
    fn resource_manager_reports_host_resources_and_recommends_threads() {
        let manager = ResourceManager::new();
        let _default = ResourceManager::default();

        assert!(manager.total_memory_mb() > 0);
        assert!(manager.available_memory_mb() > 0);
        assert!(manager.cpu_count() > 0);
        assert!(manager.recommended_threads(usize::MAX) >= 1);
        assert!(manager.recommended_threads(1) <= manager.cpu_count());

        manager.refresh_memory();
        assert!(manager.available_memory_mb() > 0);
    }
}
