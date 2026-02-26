// Operations module for database management tasks
//
// Provides:
// - Backup/restore functionality
// - Maintenance operations
// - Data migration tools

pub mod backup;
pub mod restore;

// Re-export commonly used types
pub use backup::{BackupConfig, BackupManager, BackupManifest, BackupTarget, BackupType};
pub use restore::{RestoreConfig, RestoreManager, RestoreResult, ValidationResult};
