//! Engine-specific optimization profiles
//!
//! Defines engine profiles for optimized block encoding and quantization.

/// Engine-specific optimization profiles
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineProfile {
    /// SST: Write-optimized with filtering stages
    #[default]
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
