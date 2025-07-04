//! JSON serialization for memtable data

use super::MemtableSerializer;
use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

pub struct JsonSerializer;

impl MemtableSerializer for JsonSerializer {
    fn serialize<T: Serialize>(&self, data: &T) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(data)?)
    }
    
    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(data)?)
    }
    
    fn name(&self) -> &'static str {
        "json"
    }
}