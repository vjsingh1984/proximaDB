//! Serialization Strategies for Memtable Data
//! 
//! Provides pluggable serialization formats for different use cases:
//! - Avro: Schema evolution support
//! - Bincode: High performance native Rust
//! - JSON: Human readable debugging

pub mod avro;
pub mod bincode;
pub mod json;

use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

/// Generic serialization trait
pub trait MemtableSerializer: Send + Sync {
    /// Serialize data to bytes
    fn serialize<T: Serialize>(&self, data: &T) -> Result<Vec<u8>>;
    
    /// Deserialize data from bytes
    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T>;
    
    /// Get serializer name
    fn name(&self) -> &'static str;
}