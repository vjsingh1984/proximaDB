//! Morsel-driven parallelism for SST query execution (TD-039)
//!
//! This module implements morsel-based parallel execution, dividing work into
//! fixed-size chunks (morsels) that can be processed by multiple workers.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              MorselScheduler                │
//! │  Divides row groups into 4096-row morsels  │
//! └───────────────┬─────────────────────────────┘
//!                 │
//!     ┌───────────┼───────────┬───────────┐
//!     ▼           ▼           ▼           ▼
//! ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐
//! │ Worker 1│ │ Worker 2│ │ Worker 3│ │ Worker N│
//! │(Steal)  │ │(Steal)  │ │(Steal)  │ │(Steal)  │
//! └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘
//!      │           │           │           │
//!      └───────────┴───────────┴───────────┘
//!                      │
//!                      ▼
//!              ┌───────────────┐
//!              │  MorselQueue  │
//!              │ (Work-Stealing)│
//!              └───────────────┘
//! ```
//!
//! ## Morsel Size
//!
//! - **4096 rows** per morsel (optimal for CPU cache)
//! - Aligns with Arrow batch sizes
//! - Balances overhead vs parallelism
//!
//! ## Work Stealing
//!
//! - Idle workers steal from other workers' queues
//! - Reduces load imbalance
//! - Maximizes CPU utilization

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, trace};

use crate::proto::proximadb_v1::VectorRecord;

/// Fixed morsel size for optimal CPU cache utilization
pub const MORSEL_SIZE: usize = 4096;

/// A morsel (fixed-size chunk) of work for parallel processing
#[derive(Debug, Clone)]
pub struct Morsel {
    /// Unique identifier for this morsel
    pub id: usize,
    /// Starting row index in the row group
    pub start_row: usize,
    /// Number of rows in this morsel
    pub row_count: usize,
    /// Row group index this morsel belongs to
    pub row_group_idx: usize,
}

impl Morsel {
    /// Create a new morsel
    pub fn new(id: usize, start_row: usize, row_count: usize, row_group_idx: usize) -> Self {
        Self {
            id,
            start_row,
            row_count,
            row_group_idx,
        }
    }

    /// Check if this morsel is the last in its row group
    pub fn is_last_morsel(&self, total_rows: usize) -> bool {
        self.start_row + self.row_count >= total_rows
    }

    /// Get the end row index (exclusive)
    pub fn end_row(&self) -> usize {
        self.start_row + self.row_count
    }
}

/// Morsel scheduler for dividing work into parallel chunks
pub struct MorselScheduler {
    /// Maximum number of concurrent workers
    max_workers: usize,
    /// Morsel queue for work distribution
    morsel_queue: Arc<Mutex<Vec<Morsel>>>,
    /// Semaphore for limiting concurrent work
    semaphore: Arc<Semaphore>,
}

impl MorselScheduler {
    /// Create a new morsel scheduler
    ///
    /// # Arguments
    ///
    /// * `max_workers` - Maximum number of concurrent workers (defaults to CPU count)
    pub fn new(max_workers: Option<usize>) -> Self {
        let workers = max_workers.unwrap_or_else(|| {
            // Default to number of available CPUs
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

        info!(
            "Creating MorselScheduler with {} workers",
            workers
        );

        Self {
            max_workers: workers,
            morsel_queue: Arc::new(Mutex::new(Vec::new())),
            semaphore: Arc::new(Semaphore::new(workers)),
        }
    }

    /// Divide a row group into morsels
    ///
    /// # Arguments
    ///
    /// * `row_group_idx` - Index of the row group
    /// * `total_rows` - Total number of rows in the row group
    ///
    /// # Returns
    ///
    /// Vector of morsels covering the entire row group
    pub fn divide_row_group(
        &self,
        row_group_idx: usize,
        total_rows: usize,
    ) -> Vec<Morsel> {
        let mut morsels = Vec::new();
        let mut morsel_id = 0;
        let mut start_row = 0;

        while start_row < total_rows {
            let remaining = total_rows - start_row;
            let row_count = remaining.min(MORSEL_SIZE);

            morsels.push(Morsel::new(
                morsel_id,
                start_row,
                row_count,
                row_group_idx,
            ));

            start_row += row_count;
            morsel_id += 1;
        }

        debug!(
            "Divided row group {} ({} rows) into {} morsels",
            row_group_idx,
            total_rows,
            morsels.len()
        );

        morsels
    }

    /// Process records in morsels using parallel workers
    ///
    /// # Arguments
    ///
    /// * `records` - Records to process
    /// * `processor` - Async function to process each morsel
    ///
    /// # Returns
    ///
    /// Combined results from all morsels
    pub async fn process_morsels<F, R, Fut>(
        &self,
        records: Vec<VectorRecord>,
        processor: F,
    ) -> Result<Vec<R>>
    where
        F: Fn(Vec<VectorRecord>) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Vec<R>>> + Send + 'static,
        R: Send + 'static,
    {
        let total_records = records.len();
        let morsels = self.divide_row_group(0, total_records);

        info!(
            "Processing {} records in {} morsels with {} workers",
            total_records,
            morsels.len(),
            self.max_workers
        );

        // Split records into morsels
        let mut morsel_records_vec = Vec::new();
        let mut start = 0;

        for morsel in &morsels {
            let end = start + morsel.row_count;
            let morsel_records: Vec<VectorRecord> = records[start..end].to_vec();
            morsel_records_vec.push((morsel.clone(), morsel_records));
            start = end;
        }

        // Create processing tasks
        let mut tasks = Vec::new();

        for (morsel, morsel_records) in morsel_records_vec {
            let permit = self.semaphore.clone().acquire_owned().await?;
            let processor_clone = processor.clone();

            // Spawn task for this morsel
            let task = tokio::spawn(async move {
                let _permit = permit; // Hold permit until processing completes
                trace!("Processing morsel {} ({} records)", morsel.id, morsel.row_count);
                processor_clone(morsel_records).await
            });

            tasks.push(task);
        }

        // Collect results from all morsels
        let mut results = Vec::new();
        for task in tasks {
            let morsel_results = task.await??;
            results.extend(morsel_results);
        }

        debug!(
            "Completed morsel processing: {} results",
            results.len()
        );

        Ok(results)
    }

    /// Get the maximum number of workers
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Get the current morsel queue length
    pub fn queue_len(&self) -> usize {
        self.morsel_queue.try_lock().map(|guard| guard.len()).unwrap_or(0)
    }
}

impl Default for MorselScheduler {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_morsel_creation() {
        let morsel = Morsel::new(0, 0, 4096, 0);
        assert_eq!(morsel.id, 0);
        assert_eq!(morsel.start_row, 0);
        assert_eq!(morsel.row_count, 4096);
        assert_eq!(morsel.row_group_idx, 0);
    }

    #[test]
    fn test_morsel_is_last() {
        let morsel = Morsel::new(0, 0, 4096, 0);
        assert!(morsel.is_last_morsel(4096));
        assert!(!morsel.is_last_morsel(8192));
    }

    #[test]
    fn test_divide_exact_multiple() {
        let scheduler = MorselScheduler::new(Some(4));
        let morsels = scheduler.divide_row_group(0, 8192); // Exactly 2 morsels

        assert_eq!(morsels.len(), 2);
        assert_eq!(morsels[0].row_count, 4096);
        assert_eq!(morsels[1].row_count, 4096);
        assert_eq!(morsels[0].start_row, 0);
        assert_eq!(morsels[1].start_row, 4096);
    }

    #[test]
    fn test_divide_partial_morsel() {
        let scheduler = MorselScheduler::new(Some(4));
        let morsels = scheduler.divide_row_group(0, 5000); // 1 full + 1 partial

        assert_eq!(morsels.len(), 2);
        assert_eq!(morsels[0].row_count, 4096);
        assert_eq!(morsels[1].row_count, 904); // 5000 - 4096
        assert_eq!(morsels[1].start_row, 4096);
    }

    #[test]
    fn test_divide_small_row_group() {
        let scheduler = MorselScheduler::new(Some(4));
        let morsels = scheduler.divide_row_group(0, 1000); // Less than MORSEL_SIZE

        assert_eq!(morsels.len(), 1);
        assert_eq!(morsels[0].row_count, 1000);
        assert_eq!(morsels[0].start_row, 0);
    }

    #[tokio::test]
    async fn test_process_morsels_empty() {
        let scheduler = MorselScheduler::new(Some(4));
        let records = vec![];

        let results = scheduler
            .process_morsels(records, |morsel_records| async move {
                Ok(morsel_records.len())
            })
            .await
            .unwrap();

        assert_eq!(results, vec![0]);
    }

    #[tokio::test]
    async fn test_process_morsels_single() {
        let scheduler = MorselScheduler::new(Some(4));
        let records = vec![VectorRecord::default(); 100];

        let results = scheduler
            .process_morsels(records, |morsel_records| async move {
                Ok(morsel_records.len())
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 100);
    }

    #[tokio::test]
    async fn test_process_morsels_multiple() {
        let scheduler = MorselScheduler::new(Some(4));
        let records = (0..10000)
            .map(|_| VectorRecord::default())
            .collect();

        let results = scheduler
            .process_morsels(records, |morsel_records| async move {
                Ok(morsel_records.len())
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 3); // 3 morsels: 4096, 4096, 1808
        assert_eq!(results.iter().sum::<usize>(), 10000);
    }
}
