//! Compaction Module
//!
//! Comprehensive compaction framework with pluggable strategies and priority scheduling.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                  CompactionScheduler                         │
//! │  (Priority Queue, Concurrency Control, Rate Limiting)       │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │              CompactionStrategyRegistry                      │
//! │  (Strategy Selection, Cost Estimation, Engine Matching)     │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!         ┌────────────────────┼────────────────────┐
//!         ▼                    ▼                    ▼
//! ┌───────────────┐   ┌───────────────┐   ┌───────────────┐
//! │   Leveled     │   │    Tiered     │   │   Custom      │
//! │  (SST, Nova)  │   │ (Viper, Helix)│   │  (Extension)  │
//! └───────────────┘   └───────────────┘   └───────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::storage::operations::compaction::{
//!     CompactionScheduler, CompactionStrategyRegistry, FileMetadata,
//! };
//!
//! // Create scheduler
//! let scheduler = CompactionScheduler::new();
//!
//! // Check if compaction needed and schedule
//! let files = get_collection_files(collection_id);
//! scheduler.check_and_schedule(collection_id, "sst", &files).await?;
//!
//! // Execute with custom executor
//! scheduler.execute_next(|plan| async {
//!     // Perform actual compaction
//!     execute_compaction(plan).await
//! }).await?;
//! ```

pub mod manager;
pub mod scheduler;
pub mod strategies;

// Re-export main types
pub use manager::{CompactionManager, CompactionStatus, CompactionType};
pub use scheduler::{CompactionScheduler, SchedulerConfig, SchedulerStats};
pub use strategies::{
    CompactionCostEstimate, CompactionExecutionResult, CompactionParameters, CompactionPlan,
    CompactionStrategy, CompactionStrategyRegistry, FileMetadata, FileStatistics,
    LeveledCompactionStrategy, TieredCompactionStrategy,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify all types are exported correctly
        let _scheduler = CompactionScheduler::new();
        let _registry = CompactionStrategyRegistry::new();
        let _file = FileMetadata::new("test", "/path", 1024);
        let _leveled = LeveledCompactionStrategy::new();
        let _tiered = TieredCompactionStrategy::new();
    }

    #[tokio::test]
    async fn test_integration() {
        let scheduler = CompactionScheduler::new();

        // Create test files
        let files = vec![
            FileMetadata::new("f1", "/data/f1.sst", 32 * 1024 * 1024).with_level(0),
            FileMetadata::new("f2", "/data/f2.sst", 32 * 1024 * 1024).with_level(0),
            FileMetadata::new("f3", "/data/f3.sst", 32 * 1024 * 1024).with_level(0),
            FileMetadata::new("f4", "/data/f4.sst", 32 * 1024 * 1024).with_level(0),
        ];

        // Check and schedule
        let result = scheduler.check_and_schedule("test", "sst", &files).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some()); // Should schedule L0 compaction
    }
}
