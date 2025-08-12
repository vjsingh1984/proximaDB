//! ProximaDB Version Constants
//!
//! Centralized version management for ProximaDB.
//! This module provides a single source of truth for version information.

/// The current version of ProximaDB
/// 
/// This version should match the version in Cargo.toml and be used consistently
/// across all modules, health checks, and API responses.
pub const PROXIMADB_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Application name constant
pub const PROXIMADB_NAME: &str = env!("CARGO_PKG_NAME");

/// Application description
pub const PROXIMADB_DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

/// Version helper functions
impl VersionInfo {
    /// Get the current ProximaDB version
    pub fn version() -> &'static str {
        PROXIMADB_VERSION
    }
    
    /// Get the application name
    pub fn name() -> &'static str {
        PROXIMADB_NAME
    }
    
    /// Get the application description
    pub fn description() -> &'static str {
        PROXIMADB_DESCRIPTION
    }
    
    /// Get version info as a formatted string
    pub fn version_string() -> String {
        format!("{} v{}", PROXIMADB_NAME, PROXIMADB_VERSION)
    }
}

/// Version information struct
pub struct VersionInfo;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_constants() {
        assert!(!PROXIMADB_VERSION.is_empty());
        assert!(!PROXIMADB_NAME.is_empty());
        assert!(!PROXIMADB_DESCRIPTION.is_empty());
    }

    #[test]
    fn test_version_info() {
        assert_eq!(VersionInfo::version(), PROXIMADB_VERSION);
        assert_eq!(VersionInfo::name(), PROXIMADB_NAME);
        assert!(VersionInfo::version_string().contains(PROXIMADB_VERSION));
    }
}