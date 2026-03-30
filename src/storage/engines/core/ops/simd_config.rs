use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Global SIMD configuration instance
static SIMD_CONFIG: OnceLock<SIMDConfiguration> = OnceLock::new();

/// SIMD optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SIMDConfiguration {
    /// Enable SIMD optimizations globally
    pub enabled: bool,

    /// Minimum vector count to trigger SIMD optimization
    pub min_vectors_for_simd: usize,

    /// Minimum dimension size for SIMD transpose
    pub min_dimension_for_transpose: usize,

    /// Enable parallel encoding of dimensions
    pub parallel_encoding: bool,

    /// Number of worker threads for parallel encoding
    pub encoding_threads: usize,

    /// Memory pool configuration
    pub memory_pool: MemoryPoolConfig,

    /// Engine-specific settings
    pub engine_settings: EngineSettings,

    /// Encoding algorithm preferences
    pub encoding_preferences: EncodingPreferences,

    /// Performance monitoring
    pub monitoring: MonitoringConfig,
}

/// Memory pool configuration for SIMD operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPoolConfig {
    /// Enable memory pooling
    pub enabled: bool,

    /// Maximum pool size in MB
    pub max_pool_size_mb: usize,

    /// Buffer alignment in bytes
    pub alignment_bytes: usize,

    /// Pre-allocate buffers on startup
    pub pre_allocate: bool,

    /// Number of buffers to pre-allocate
    pub pre_allocate_count: usize,
}

/// Engine-specific SIMD settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct EngineSettings {
    /// HELIX engine settings
    pub helix: HelixSettings,

    /// SST engine settings
    pub sst: SSTSettings,

    /// SWIFT engine settings
    pub swift: SwiftSettings,
}

/// HELIX engine SIMD settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelixSettings {
    /// Enable spatial clustering optimization
    pub spatial_clustering: bool,

    /// Block size for spatial grouping
    pub spatial_block_size: usize,

    /// Enable PCA preprocessing
    pub pca_preprocessing: bool,

    /// Preferred encoding layout
    pub preferred_layout: String,
}

/// SST engine SIMD settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SSTSettings {
    /// Maximum compression mode
    pub max_compression: bool,

    /// Aggressive encoding threshold
    pub aggressive_threshold: f32,

    /// Enable multi-pass optimization
    pub multi_pass: bool,

    /// Preferred encoding layout
    pub preferred_layout: String,
}

/// SWIFT engine SIMD settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwiftSettings {
    /// Enable hierarchical optimization
    pub hierarchical_optimization: bool,

    /// Group size for dimension grouping
    pub dimension_group_size: usize,

    /// Enable low-latency mode
    pub low_latency_mode: bool,

    /// Preferred encoding layout
    pub preferred_layout: String,
}

/// Encoding algorithm preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodingPreferences {
    /// Enable PForDelta encoding
    pub pfor_delta: bool,

    /// Enable Zigzag encoding
    pub zigzag: bool,

    /// Enable Simple8b encoding
    pub simple8b: bool,

    /// Enable VByte encoding
    pub vbyte: bool,

    /// Enable DoubleDelta encoding
    pub double_delta: bool,

    /// Enable Run-Length encoding
    pub run_length: bool,

    /// Enable Hybrid encoding
    pub hybrid: bool,

    /// Automatic algorithm selection
    pub auto_select: bool,
}

/// Performance monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    /// Enable performance tracking
    pub enabled: bool,

    /// Track compression ratios
    pub track_compression: bool,

    /// Track encoding times
    pub track_timing: bool,

    /// Track memory usage
    pub track_memory: bool,

    /// Log level for SIMD operations
    pub log_level: String,
}

impl Default for SIMDConfiguration {
    fn default() -> Self {
        Self {
            enabled: true,
            min_vectors_for_simd: 100,
            min_dimension_for_transpose: 64,
            parallel_encoding: true,
            encoding_threads: num_cpus::get(),
            memory_pool: MemoryPoolConfig::default(),
            engine_settings: EngineSettings::default(),
            encoding_preferences: EncodingPreferences::default(),
            monitoring: MonitoringConfig::default(),
        }
    }
}

impl Default for MemoryPoolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pool_size_mb: 256,
            alignment_bytes: 64, // Cache line alignment
            pre_allocate: true,
            pre_allocate_count: 10,
        }
    }
}


impl Default for HelixSettings {
    fn default() -> Self {
        Self {
            spatial_clustering: true,
            spatial_block_size: 1024,
            pca_preprocessing: false,
            preferred_layout: "TransposeFieldEncodedAndCompressedVector".to_string(),
        }
    }
}

impl Default for SSTSettings {
    fn default() -> Self {
        Self {
            max_compression: true,
            aggressive_threshold: 0.7,
            multi_pass: false,
            preferred_layout: "TransposeFieldEncodedAndCompressedVector".to_string(),
        }
    }
}

impl Default for SwiftSettings {
    fn default() -> Self {
        Self {
            hierarchical_optimization: true,
            dimension_group_size: 32,
            low_latency_mode: true,
            preferred_layout: "GroupedFieldEncodedAndCompressedVector".to_string(),
        }
    }
}

impl Default for EncodingPreferences {
    fn default() -> Self {
        Self {
            pfor_delta: true,
            zigzag: true,
            simple8b: true,
            vbyte: true,
            double_delta: true,
            run_length: true,
            hybrid: false, // Experimental
            auto_select: true,
        }
    }
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: cfg!(debug_assertions), // Enable in debug builds
            track_compression: true,
            track_timing: true,
            track_memory: false, // Can be expensive
            log_level: "debug".to_string(),
        }
    }
}

impl SIMDConfiguration {
    /// Get the global SIMD configuration
    pub fn global() -> &'static SIMDConfiguration {
        SIMD_CONFIG.get_or_init(|| {
            // Try to load from config file
            Self::load_from_file().unwrap_or_default()
        })
    }

    /// Load configuration from TOML file
    pub fn load_from_file() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path =
            std::env::var("PROXIMADB_CONFIG").unwrap_or_else(|_| "config/simd.toml".to_string());

        if std::path::Path::new(&config_path).exists() {
            let content = std::fs::read_to_string(config_path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    /// Initialize with custom configuration
    pub fn init_with(config: SIMDConfiguration) -> Result<(), SIMDConfiguration> {
        SIMD_CONFIG.set(config)
    }

    /// Check if SIMD should be used for given parameters
    pub fn should_use_simd(&self, vector_count: usize, dimension: usize) -> bool {
        self.enabled
            && vector_count >= self.min_vectors_for_simd
            && dimension >= self.min_dimension_for_transpose
    }

    /// Get engine-specific layout preference
    pub fn get_engine_layout(&self, engine: &str) -> String {
        match engine {
            "helix" => self.engine_settings.helix.preferred_layout.clone(),
            "sst" => self.engine_settings.sst.preferred_layout.clone(),
            "swift" => self.engine_settings.swift.preferred_layout.clone(),
            _ => "TransposeFieldEncodedAndCompressedVector".to_string(),
        }
    }
}

/// Feature flags for compile-time optimization
pub mod features {
    /// Enable AVX2 instructions
    #[cfg(target_arch = "x86_64")]
    pub const AVX2_ENABLED: bool = cfg!(target_feature = "avx2");

    /// Enable AVX512 instructions
    #[cfg(target_arch = "x86_64")]
    pub const AVX512_ENABLED: bool = cfg!(target_feature = "avx512f");

    /// Enable NEON instructions
    #[cfg(target_arch = "aarch64")]
    pub const NEON_ENABLED: bool = true;

    /// Enable parallel encoding
    pub const PARALLEL_ENCODING: bool = true;

    /// Enable memory pooling
    pub const MEMORY_POOLING: bool = true;

    /// Enable advanced encoding algorithms
    pub const ADVANCED_ENCODINGS: bool = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SIMDConfiguration::default();
        assert!(config.enabled);
        assert_eq!(config.min_vectors_for_simd, 100);
        assert!(config.parallel_encoding);
    }

    #[test]
    fn test_should_use_simd() {
        let config = SIMDConfiguration::default();

        // Should use SIMD for sufficient vectors and dimensions
        assert!(config.should_use_simd(1000, 768));

        // Should not use SIMD for too few vectors
        assert!(!config.should_use_simd(10, 768));

        // Should not use SIMD for too small dimensions
        assert!(!config.should_use_simd(1000, 32));
    }

    #[test]
    fn test_engine_layout_preferences() {
        let config = SIMDConfiguration::default();

        assert_eq!(
            config.get_engine_layout("helix"),
            "TransposeFieldEncodedAndCompressedVector"
        );

        assert_eq!(
            config.get_engine_layout("swift"),
            "GroupedFieldEncodedAndCompressedVector"
        );
    }
}
