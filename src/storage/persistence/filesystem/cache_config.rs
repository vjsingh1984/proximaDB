//! Compatibility shim — implementation now lives in `proximadb-storage-common`.
pub use proximadb_storage_common::unified_cache_config::ConfigError;
pub use proximadb_storage_common::unified_cache_config::UnifiedEvictionPolicy as EvictionPolicy;
pub use proximadb_storage_common::unified_cache_config::*;
