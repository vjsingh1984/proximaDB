//! Storage Engine Identity Trait
//!
//! Defines the core identity and capability information for storage engines.
//! This trait satisfies the Single Responsibility Principle by focusing solely
//! on engine identification and capability reporting.

use crate::index::axis::eventlog::StorageEngineType;
use crate::storage::traits::StorageEngineStrategy;

/// Core identity trait for storage engines
///
/// Every storage engine must provide basic identification and capability information.
/// This is the foundational trait that all other traits build upon.
///
/// # Example
///
/// ```rust,ignore
/// impl StorageIdentity for SstEngine {
///     fn engine_name(&self) -> &'static str { "SST" }
///     fn engine_version(&self) -> &'static str { "1.0.0" }
///     fn strategy(&self) -> StorageEngineStrategy { StorageEngineStrategy::Sst }
/// }
/// ```
pub trait StorageIdentity: Send + Sync {
    /// Human-readable engine name (e.g., "SST", "VIPER", "HELIX")
    fn engine_name(&self) -> &'static str;

    /// Engine version string (semver format preferred)
    fn engine_version(&self) -> &'static str;

    /// Storage strategy this engine implements
    fn strategy(&self) -> StorageEngineStrategy;

    /// Get the storage engine type for AXIS indexing and event logging
    ///
    /// This method eliminates the need for string matching on engine_name(),
    /// following the Open/Closed Principle. Each engine provides its type
    /// directly, so adding new engines doesn't require modifying dispatch code.
    fn engine_type(&self) -> StorageEngineType {
        // Default implementation maps from strategy for backward compatibility
        match self.strategy() {
            StorageEngineStrategy::Sst => StorageEngineType::SST,
            StorageEngineStrategy::Viper => StorageEngineType::VIPER,
            StorageEngineStrategy::Helix => StorageEngineType::HELIX,
            StorageEngineStrategy::Nova => StorageEngineType::NOVA,
            StorageEngineStrategy::Swift => StorageEngineType::SWIFT,
            StorageEngineStrategy::Raptor => StorageEngineType::RAPTOR,
            StorageEngineStrategy::TimeSeries => StorageEngineType::TST,
            // Future engines should override this method
            _ => StorageEngineType::SST,
        }
    }

    /// Check if engine supports collection-level operations
    ///
    /// Engines that return false operate on the entire database.
    fn supports_collection_level_operations(&self) -> bool {
        match self.strategy() {
            StorageEngineStrategy::Sst => false, // SST operates on entire tree
            _ => true,
        }
    }

    /// Check if engine supports atomic operations
    ///
    /// Atomic operations guarantee all-or-nothing semantics.
    fn supports_atomic_operations(&self) -> bool {
        match self.strategy() {
            StorageEngineStrategy::Sst => false,    // Eventual consistency
            StorageEngineStrategy::Raptor => false, // Eventual consistency
            _ => true,
        }
    }

    /// Check if engine supports background operations
    fn supports_background_operations(&self) -> bool {
        true // All engines support background operations by default
    }

    /// Check if engine supports a specific named feature
    fn supports_feature(&self, feature: &str) -> bool {
        match feature {
            "collection_level_operations" => self.supports_collection_level_operations(),
            "atomic_operations" => self.supports_atomic_operations(),
            "background_operations" => self.supports_background_operations(),
            _ => false,
        }
    }
}
