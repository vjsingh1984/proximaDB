//! Data Persistence Layer
//!
//! This module contains all data persistence components including
//! WAL, filesystem abstraction, disk management, and atomicity coordination.

pub mod disk_manager;
pub mod filesystem;
pub mod write_ahead_log;

// Re-export main persistence types
pub use disk_manager::DiskManager;
pub use filesystem::{FilesystemConfig, FilesystemFactory};
pub use write_ahead_log::{WALConfig, WriteAheadLogManager, WriteBufferStrategyType as WALStrategyType};
// WalFactory removed - use WALBatchFactory for modern implementations
