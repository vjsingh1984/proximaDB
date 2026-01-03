//! # Multi-Process Coordination for Embedded ProximaDB
//!
//! This module provides file-based coordination mechanisms for multiple processes
//! to safely access the same embedded database. It supports three access modes:
//!
//! - **Exclusive**: Single writer with exclusive access
//! - **SharedRead**: Multiple readers, no writers
//! - **LeaderFollower**: One leader (write), many followers (read)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::embedded::coordination::{AccessMode, FileLockManager};
//!
//! // Exclusive access
//! let lock_manager = FileLockManager::new("/path/to/data", AccessMode::Exclusive)?;
//!
//! // Shared read access
//! let lock_manager = FileLockManager::new("/path/to/data", AccessMode::SharedRead)?;
//!
//! // Leader/follower mode
//! let lock_manager = FileLockManager::new("/path/to/data", AccessMode::LeaderFollower)?;
//! ```
//!
//! ## Implementation
//!
//! Uses advisory file locking (flock/fcntl on Unix) for cross-process coordination.
//! Lock files are created in the data directory with the `.lock` extension.

mod file_lock;
mod leader_election;

pub use file_lock::{AccessMode, FileLockError, FileLockManager};
pub use leader_election::{LeaderElection, LeaderElectionError, LeaderStatus};

/// Coordination error type combining all coordination-related errors
#[derive(Debug)]
pub enum CoordinationError {
    /// File lock error
    FileLock(FileLockError),
    /// Leader election error
    LeaderElection(LeaderElectionError),
    /// Write operation attempted in read-only mode
    ReadOnlyMode,
    /// Lock acquisition timed out
    LockTimeout,
}

impl std::fmt::Display for CoordinationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileLock(e) => write!(f, "File lock error: {}", e),
            Self::LeaderElection(e) => write!(f, "Leader election error: {}", e),
            Self::ReadOnlyMode => write!(f, "Write operation attempted in read-only mode"),
            Self::LockTimeout => write!(f, "Lock acquisition timed out"),
        }
    }
}

impl std::error::Error for CoordinationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileLock(e) => Some(e),
            Self::LeaderElection(e) => Some(e),
            Self::ReadOnlyMode | Self::LockTimeout => None,
        }
    }
}

impl From<FileLockError> for CoordinationError {
    fn from(err: FileLockError) -> Self {
        Self::FileLock(err)
    }
}

impl From<LeaderElectionError> for CoordinationError {
    fn from(err: LeaderElectionError) -> Self {
        Self::LeaderElection(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_exclusive_lock() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let path = temp_dir.path().to_path_buf();

        // First process acquires exclusive lock
        let lock1 = FileLockManager::new(&path, AccessMode::Exclusive);
        assert!(lock1.is_ok(), "First exclusive lock should succeed");

        // Second process should fail to acquire exclusive lock (non-blocking)
        let lock2 = FileLockManager::try_new(&path, AccessMode::Exclusive);
        assert!(lock2.is_err(), "Second exclusive lock should fail");
    }

    #[test]
    fn test_shared_read_lock() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let path = temp_dir.path().to_path_buf();

        // Multiple readers should succeed
        let lock1 = FileLockManager::new(&path, AccessMode::SharedRead);
        assert!(lock1.is_ok(), "First shared lock should succeed");

        let lock2 = FileLockManager::new(&path, AccessMode::SharedRead);
        assert!(lock2.is_ok(), "Second shared lock should succeed");
    }

    #[test]
    fn test_access_mode_can_write() {
        assert!(AccessMode::Exclusive.can_write());
        assert!(!AccessMode::SharedRead.can_write());
        assert!(AccessMode::LeaderFollower.can_write()); // Leader can write
    }

    #[test]
    fn test_leader_election() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let path = temp_dir.path().to_path_buf();

        // First node becomes leader
        let election1 = LeaderElection::new(&path, "node1");
        assert!(election1.is_ok());
        let election1 = election1.unwrap();

        // Check that node1 is leader
        assert!(election1.is_leader());
        assert_eq!(election1.leader_id(), Some("node1".to_string()));
    }
}
