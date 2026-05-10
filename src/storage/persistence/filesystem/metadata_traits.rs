//! Metadata serialization traits for engine-specific implementations
//!
//! This module defines the contract that storage engines must implement
//! to provide their own metadata serialization logic to the unified filesystem.
//!
//! Each engine owns its metadata format and provides a serializer implementation.

use anyhow::Result;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt::Debug;

/// Trait for engine-specific metadata serialization
///
/// Each storage engine implements this trait to handle its own metadata format.
/// This keeps the filesystem layer decoupled from engine-specific details.
pub trait EngineMetadataSerializer: Send + Sync + Debug {
    /// Serialize metadata to bytes
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes>;

    /// Deserialize metadata from bytes
    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>>;

    /// Get the engine type this serializer handles
    fn engine_type(&self) -> &str;

    /// Extract cacheable metadata components (e.g., Parquet footer for VIPER)
    /// Returns None if no special caching is needed
    fn extract_cacheable_component(&self, _data: &[u8], _file_path: &str) -> Option<Bytes> {
        None
    }

    /// Check if a file path should have metadata cached
    fn should_cache_metadata(&self, _file_path: &str) -> bool {
        true
    }
}

pub fn serialize_typed_metadata<T>(metadata: &dyn Any, expected_type: &str) -> Result<Bytes>
where
    T: Serialize + Send + Sync + 'static,
{
    if let Some(typed_metadata) = metadata.downcast_ref::<T>() {
        let bytes = bincode::serialize(typed_metadata)?;
        Ok(Bytes::from(bytes))
    } else {
        anyhow::bail!("Expected {expected_type} type for serializer")
    }
}

pub fn deserialize_typed_metadata<T>(bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let typed_metadata: T = bincode::deserialize(bytes)?;
    Ok(Box::new(typed_metadata))
}

/// Generic metadata that can be cached by the filesystem
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheableMetadata {
    /// File path
    pub file_path: String,

    /// File size in bytes
    pub file_size: u64,

    /// Last modified timestamp
    pub last_modified: i64,

    /// Engine-specific serialized metadata (stored as Vec<u8> for serialization)
    pub engine_metadata: Option<Vec<u8>>,

    /// Extracted cacheable component (e.g., Parquet footer, stored as Vec<u8> for serialization)
    pub cached_component: Option<Vec<u8>>,
}

/// Default serializer for unknown/generic engines
#[derive(Debug)]
pub struct GenericMetadataSerializer;

impl EngineMetadataSerializer for GenericMetadataSerializer {
    fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
        // For generic engines, we just store basic file info
        if let Some(file_meta) =
            metadata.downcast_ref::<crate::storage::persistence::filesystem::FileMetadata>()
        {
            let cacheable = CacheableMetadata {
                file_path: file_meta.path.clone(),
                file_size: file_meta.size,
                last_modified: file_meta.modified.map_or(0, |dt| dt.timestamp()),
                engine_metadata: None,
                cached_component: None,
            };
            let bytes = bincode::serialize(&cacheable)?;
            Ok(Bytes::from(bytes))
        } else {
            anyhow::bail!("Cannot serialize unknown metadata type")
        }
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        let cacheable: CacheableMetadata = bincode::deserialize(bytes)?;
        Ok(Box::new(cacheable))
    }

    fn engine_type(&self) -> &str {
        "generic"
    }
}

/// Example: VIPER engine metadata serializer
/// This would be implemented in the VIPER engine module
#[cfg(test)]
mod viper_example {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[allow(dead_code)]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ViperMetadata {
        pub row_groups: Vec<RowGroupInfo>,
        pub total_rows: usize,
        pub parquet_footer: Option<Vec<u8>>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RowGroupInfo {
        pub id: u32,
        pub row_count: usize,
        pub file_offset: u64,
    }

    #[allow(dead_code)]
    #[derive(Debug)]
    pub struct ViperMetadataSerializer;

    impl EngineMetadataSerializer for ViperMetadataSerializer {
        fn serialize(&self, metadata: &dyn Any) -> Result<Bytes> {
            if let Some(viper_meta) = metadata.downcast_ref::<ViperMetadata>() {
                let bytes = bincode::serialize(viper_meta)?;
                Ok(Bytes::from(bytes))
            } else {
                anyhow::bail!("Expected ViperMetadata type")
            }
        }

        fn deserialize(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
            let viper_meta: ViperMetadata = bincode::deserialize(bytes)?;
            Ok(Box::new(viper_meta))
        }

        fn engine_type(&self) -> &str {
            "viper"
        }

        fn extract_cacheable_component(&self, data: &[u8], file_path: &str) -> Option<Bytes> {
            // Extract Parquet footer for VIPER files
            if file_path.ends_with(".parquet") && data.len() > 8 {
                // Check for PAR1 magic bytes
                if &data[0..4] == b"PAR1" && &data[data.len() - 4..] == b"PAR1" {
                    // Read footer size (last 4 bytes before final PAR1)
                    let footer_size_bytes = &data[data.len() - 8..data.len() - 4];
                    let footer_size = u32::from_le_bytes([
                        footer_size_bytes[0],
                        footer_size_bytes[1],
                        footer_size_bytes[2],
                        footer_size_bytes[3],
                    ]) as usize;

                    if footer_size < data.len() - 8 {
                        let footer_start = data.len() - 8 - footer_size;
                        return Some(Bytes::copy_from_slice(&data[footer_start..data.len() - 8]));
                    }
                }
            }
            None
        }

        fn should_cache_metadata(&self, file_path: &str) -> bool {
            // Cache metadata for Parquet files
            file_path.ends_with(".parquet")
        }
    }
}
