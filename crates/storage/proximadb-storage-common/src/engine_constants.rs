// Engine type constants for consistent identification across the system.

pub const ENGINE_SST: &str = "sst";
pub const SST_MAGIC: [u8; 4] = *b"SST1";

pub const ENGINE_SWIFT: &str = "swift";
pub const SWIFT_MAGIC: [u8; 4] = *b"SWFT";

pub const ENGINE_VIPER: &str = "viper";
pub const VIPER_MAGIC: [u8; 4] = *b"VIPR";

pub const ENGINE_NOVA: &str = "nova";
pub const NOVA_MAGIC: [u8; 4] = *b"NOVA";

pub const ENGINE_RAPTOR: &str = "raptor";
pub const RAPTOR_MAGIC: [u8; 4] = *b"RPTR";

pub const ENGINE_COLUMNAR: &str = "columnar";

pub const ENGINE_HELIX: &str = "helix";
pub const HELIX_MAGIC: [u8; 4] = *b"HELX";
pub const HELIX_FILE_EXT: &str = ".helix";

pub const SST_FILE_EXT: &str = ".sst";
pub const SWIFT_FILE_EXT: &str = ".swift";
pub const VIPER_FILE_EXT: &str = ".parquet";
pub const NOVA_FILE_EXT: &str = ".parquet";
pub const RAPTOR_FILE_EXT: &str = ".raptor";
pub const PRISM_FILE_EXT: &str = ".prism";

pub const METADATA_EXT: &str = ".meta";
pub const STATS_EXT: &str = ".stats";
pub const INDEX_EXT: &str = ".idx";
pub const BLOOM_FILTER_EXT: &str = ".bloom";
