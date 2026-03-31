//! Database management operations — backup, restore, and maintenance.

/// Incremental and full backup with S3/GCS/local targets.
pub mod backup;
/// Point-in-time restore from backup manifests.
pub mod restore;

// Re-export commonly used types
pub use backup::{BackupConfig, BackupManager, BackupManifest, BackupTarget, BackupType};
pub use restore::{RestoreConfig, RestoreManager, RestoreResult, ValidationResult};
