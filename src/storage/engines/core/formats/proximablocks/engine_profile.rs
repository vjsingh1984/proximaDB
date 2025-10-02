//! Engine-specific optimization profiles
//!
//! Defines engine profiles for optimized block encoding and quantization.

/// Engine-specific optimization profiles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineProfile {
    /// SST: Write-optimized with filtering stages
    SST,
    /// SWIFT: Low-latency optimization
    Swift,
    /// HELIX: Spatial locality optimization
    Helix,
    /// RAPTOR: Adaptive row-group management
    Raptor,
    /// VIPER: Columnar analytics
    Viper,
    /// NOVA: Progressive columnar
    Nova,
}

impl Default for EngineProfile {
    fn default() -> Self {
        EngineProfile::SST
    }
}
