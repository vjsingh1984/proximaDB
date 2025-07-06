//! Data Persistence Layer
//!
//! This module contains all data persistence components including
//! WAL, filesystem abstraction, and disk management.

pub mod disk_manager;
pub mod filesystem;
pub mod wal;

// Re-export main persistence types
pub use disk_manager::DiskManager;
pub use filesystem::{FilesystemConfig, FilesystemFactory};
pub use wal::{WalConfig, WalFactory, WalManager, WalStrategyType};
