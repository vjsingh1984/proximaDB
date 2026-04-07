//! Disk manager for multi-disk storage
//!
//! Manages multiple data directories for WAL files and storage segments,
//! providing round-robin write distribution and consolidated read access.

use crate::storage::{Result, StorageError};
use std::path::PathBuf;

/// Disk manager for multi-disk storage
///
/// Manages multiple data directories, distributing writes across them
/// in a round-robin fashion to maximize I/O throughput.
#[derive(Debug)]
pub struct DiskManager {
    /// Available data directories
    data_dirs: Vec<PathBuf>,
    /// Index of current write directory (for round-robin)
    current_write_dir: usize,
}

impl DiskManager {
    /// Create a new disk manager
    ///
    /// # Arguments
    ///
    /// * `data_dirs` - List of data directory paths to manage
    ///
    /// # Returns
    ///
    /// `Ok(DiskManager)` if all directories are accessible, `Err(StorageError)` otherwise
    pub fn new(data_dirs: Vec<PathBuf>) -> Result<Self> {
        // Create directories if they don't exist
        for dir in &data_dirs {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(StorageError::DiskIO)?;
            }
        }

        Ok(Self {
            data_dirs,
            current_write_dir: 0,
        })
    }

    /// Get the next write path (round-robin)
    ///
    /// Returns a path from the data directories and advances to the next
    /// directory for subsequent calls.
    ///
    /// # Returns
    ///
    /// Reference to the next path to write to
    pub fn get_write_path(&mut self) -> &PathBuf {
        let path = &self.data_dirs[self.current_write_dir];
        self.current_write_dir = (self.current_write_dir + 1) % self.data_dirs.len();
        path
    }

    /// Get all read paths
    ///
    /// # Returns
    ///
    /// Slice of all data directory paths for reading
    pub fn get_read_paths(&self) -> &[PathBuf] {
        &self.data_dirs
    }

    /// Get the base WAL directory
    ///
    /// Returns the first data directory, which is used as the base
    /// directory for WAL files.
    ///
    /// # Returns
    ///
    /// Reference to the base WAL directory path
    pub fn get_base_wal_dir(&self) -> &PathBuf {
        &self.data_dirs[0]
    }
}
