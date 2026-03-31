// Compact enum storage - save 3 bytes per enum (u8 vs i32)
// All ProximaDB enums have < 20 values, so u8 is more than sufficient

use crate::proto::proximadb_v1::{
    CollectionOperation, CompressionAlgorithm, DistanceMetric, IndexingAlgorithm, StorageEngine,
    VectorOperation,
};

/// Compact distance metric storage (1 byte vs 4)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompactDistanceMetric {
    Unspecified = 0,
    Cosine = 1,
    Euclidean = 2,
    DotProduct = 3,
    Hamming = 4,
    Manhattan = 5,
    Jaccard = 6,
    Custom = 7,
    Chebyshev = 8,
    Canberra = 9,
    Minkowski = 10,
    Angular = 11,
    BrayCurtis = 12,
    Hellinger = 13,
}

impl From<DistanceMetric> for CompactDistanceMetric {
    fn from(metric: DistanceMetric) -> Self {
        match metric {
            DistanceMetric::DistanceMetricUnspecified => Self::Unspecified,
            DistanceMetric::Cosine => Self::Cosine,
            DistanceMetric::Euclidean => Self::Euclidean,
            DistanceMetric::DotProduct => Self::DotProduct,
            DistanceMetric::Hamming => Self::Hamming,
            DistanceMetric::Manhattan => Self::Manhattan,
            DistanceMetric::Jaccard => Self::Jaccard,
            DistanceMetric::Custom => Self::Custom,
            DistanceMetric::Chebyshev => Self::Chebyshev,
            DistanceMetric::Canberra => Self::Canberra,
            DistanceMetric::Minkowski => Self::Minkowski,
            DistanceMetric::Angular => Self::Angular,
            DistanceMetric::BrayCurtis => Self::BrayCurtis,
            DistanceMetric::Hellinger => Self::Hellinger,
        }
    }
}

impl From<CompactDistanceMetric> for DistanceMetric {
    fn from(metric: CompactDistanceMetric) -> Self {
        match metric {
            CompactDistanceMetric::Unspecified => DistanceMetric::DistanceMetricUnspecified,
            CompactDistanceMetric::Cosine => DistanceMetric::Cosine,
            CompactDistanceMetric::Euclidean => DistanceMetric::Euclidean,
            CompactDistanceMetric::DotProduct => DistanceMetric::DotProduct,
            CompactDistanceMetric::Hamming => DistanceMetric::Hamming,
            CompactDistanceMetric::Manhattan => DistanceMetric::Manhattan,
            CompactDistanceMetric::Jaccard => DistanceMetric::Jaccard,
            CompactDistanceMetric::Custom => DistanceMetric::Custom,
            CompactDistanceMetric::Chebyshev => DistanceMetric::Chebyshev,
            CompactDistanceMetric::Canberra => DistanceMetric::Canberra,
            CompactDistanceMetric::Minkowski => DistanceMetric::Minkowski,
            CompactDistanceMetric::Angular => DistanceMetric::Angular,
            CompactDistanceMetric::BrayCurtis => DistanceMetric::BrayCurtis,
            CompactDistanceMetric::Hellinger => DistanceMetric::Hellinger,
        }
    }
}

/// Compact storage engine enum (1 byte vs 4)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompactStorageEngine {
    Unspecified = 0,
    Viper = 1,
    Sst = 2,
    Mmap = 3,
    Hybrid = 4,
    Swift = 5,
    Nova = 6,
}

impl From<StorageEngine> for CompactStorageEngine {
    fn from(engine: StorageEngine) -> Self {
        match engine {
            StorageEngine::StorageEngineUnspecified => Self::Unspecified,
            StorageEngine::Viper => Self::Viper,
            StorageEngine::Sst => Self::Sst,
            StorageEngine::Mmap => Self::Mmap,
            StorageEngine::Hybrid => Self::Hybrid,
            StorageEngine::Swift => Self::Swift,
            StorageEngine::Nova => Self::Nova,
            _ => Self::Unspecified,
        }
    }
}

impl From<CompactStorageEngine> for StorageEngine {
    fn from(engine: CompactStorageEngine) -> Self {
        match engine {
            CompactStorageEngine::Unspecified => StorageEngine::StorageEngineUnspecified,
            CompactStorageEngine::Viper => StorageEngine::Viper,
            CompactStorageEngine::Sst => StorageEngine::Sst,
            CompactStorageEngine::Mmap => StorageEngine::Mmap,
            CompactStorageEngine::Hybrid => StorageEngine::Hybrid,
            CompactStorageEngine::Swift => StorageEngine::Swift,
            CompactStorageEngine::Nova => StorageEngine::Nova,
        }
    }
}

/// Ultra-compact enum packing - pack 4 enums into a single u32
/// Saves 12 bytes per record (4 * i32 = 16 bytes vs 1 * u32 = 4 bytes)
#[derive(Debug, Clone, Copy)]
pub struct PackedEnums {
    // Bit layout:
    // [0-7]:   DistanceMetric (8 bits)
    // [8-15]:  StorageEngine (8 bits)
    // [16-23]: IndexingAlgorithm (8 bits)
    // [24-31]: CompressionAlgorithm (8 bits)
    packed: u32,
}

impl PackedEnums {
    pub fn new() -> Self {
        Self { packed: 0 }
    }

    pub fn set_distance_metric(&mut self, metric: CompactDistanceMetric) {
        self.packed = (self.packed & 0xFFFFFF00) | (metric as u32);
    }

    pub fn get_distance_metric(&self) -> CompactDistanceMetric {
        unsafe { std::mem::transmute((self.packed & 0xFF) as u8) }
    }

    pub fn set_storage_engine(&mut self, engine: CompactStorageEngine) {
        self.packed = (self.packed & 0xFFFF00FF) | ((engine as u32) << 8);
    }

    pub fn get_storage_engine(&self) -> CompactStorageEngine {
        unsafe { std::mem::transmute(((self.packed >> 8) & 0xFF) as u8) }
    }
}

impl Default for PackedEnums {
    fn default() -> Self {
        Self::new()
    }
}

/// Memory savings calculation:
/// - Original: Each enum as i32 = 4 bytes
/// - Compact: Each enum as u8 = 1 byte  
/// - Savings per enum: 3 bytes (75% reduction)
/// - For 1M records with 4 enums each: 12MB saved
/// - With bincode serialization: Even more savings due to compact representation

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_optimization() {
        use std::mem::size_of;

        // Proto enum size
        assert_eq!(size_of::<DistanceMetric>(), 4);
        assert_eq!(size_of::<StorageEngine>(), 4);

        // Compact enum size
        assert_eq!(size_of::<CompactDistanceMetric>(), 1);
        assert_eq!(size_of::<CompactStorageEngine>(), 1);

        // Packed enums size (4 enums in 4 bytes)
        assert_eq!(size_of::<PackedEnums>(), 4);

        println!(
            "Space savings: {} bytes per enum",
            size_of::<DistanceMetric>() - size_of::<CompactDistanceMetric>()
        );
    }
}
