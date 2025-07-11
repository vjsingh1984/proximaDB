//! Data Persistence Layer
//!
//! This module contains all data persistence components including
//! WAL, filesystem abstraction, disk management, and atomicity coordination.

pub mod disk_manager;
pub mod filesystem;
pub mod wal;

// Re-export main persistence types
pub use disk_manager::DiskManager;
pub use filesystem::{FilesystemConfig, FilesystemFactory};
pub use wal::{WalConfig, WalManager, WalStrategyType};
// WalFactory removed - use WalBatchFactory for modern implementations
