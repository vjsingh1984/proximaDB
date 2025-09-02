// Engine type constants for consistent identification across the system
// These are used for:
// 1. IntelligentFilesystem cache segregation  
// 2. Magic bytes in file formats
// 3. Metrics and logging

/// SST (Sorted String Table) engine - optimized for real-time OLTP
pub const ENGINE_SST: &str = "sst";
pub const SST_MAGIC: [u8; 4] = *b"SST1";

/// SWIFT engine - SST with superblock optimization
pub const ENGINE_SWIFT: &str = "swift";
pub const SWIFT_MAGIC: [u8; 4] = *b"SWFT";

/// VIPER engine - columnar Parquet format for analytics
pub const ENGINE_VIPER: &str = "viper";
pub const VIPER_MAGIC: [u8; 4] = *b"VIPR";

/// NOVA engine - enhanced columnar with hierarchical stats
pub const ENGINE_NOVA: &str = "nova";
pub const NOVA_MAGIC: [u8; 4] = *b"NOVA";

/// Raptor engine - auto-tiering hybrid storage
pub const ENGINE_RAPTOR: &str = "raptor";
pub const RAPTOR_MAGIC: [u8; 4] = *b"RPTR";

/// Generic columnar identifier for shared components
pub const ENGINE_COLUMNAR: &str = "columnar";

// File extensions for each engine type
// These ensure consistency across flush, compaction, and file operations

/// SST file extension for sorted string table files
pub const SST_FILE_EXT: &str = ".sst";

/// SWIFT file extension for superblock-optimized SST files
pub const SWIFT_FILE_EXT: &str = ".swift";

/// VIPER uses standard Parquet format
pub const VIPER_FILE_EXT: &str = ".parquet";

/// NOVA uses Parquet with enhanced metadata
pub const NOVA_FILE_EXT: &str = ".parquet"; // Same as VIPER but with different metadata

/// Raptor uses single file with aggressive compaction
pub const RAPTOR_FILE_EXT: &str = ".raptor";

/// Prism uses hybrid format
pub const PRISM_FILE_EXT: &str = ".prism";

/// Metadata file extensions
pub const METADATA_EXT: &str = ".meta";
pub const STATS_EXT: &str = ".stats";
pub const INDEX_EXT: &str = ".idx";
pub const BLOOM_FILTER_EXT: &str = ".bloom";