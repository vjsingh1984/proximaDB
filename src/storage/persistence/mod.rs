//! Data Persistence Layer
//!
//! This module contains all data persistence components including
//! WAL, filesystem abstraction, and disk management.

pub mod wal;
pub mod filesystem;
pub mod disk_manager;

// Re-export main persistence types
pub use wal::{WalConfig, WalManager, WalFactory, WalStrategyType};
pub use filesystem::{FilesystemConfig, FilesystemFactory};
pub use disk_manager::DiskManager;