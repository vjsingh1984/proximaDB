/// Shared sizing constants for ProximaBlocks-based engines (SST/Swift/Helix)
pub const DEFAULT_BLOCK_METADATA_OVERHEAD_BYTES: usize = 200;

/// Target block size when dimension-aware sizing is used
pub const DEFAULT_TARGET_BLOCK_SIZE_BYTES: usize = 3 * 1024 * 1024;

/// Minimum allowed block size for sizing helpers
pub const MIN_TARGET_BLOCK_SIZE_BYTES: usize = 2 * 1024 * 1024;

/// Maximum allowed block size for sizing helpers
pub const MAX_TARGET_BLOCK_SIZE_BYTES: usize = 4 * 1024 * 1024;
