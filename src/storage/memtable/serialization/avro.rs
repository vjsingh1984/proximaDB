//! Avro serialization for memtable data

use super::MemtableSerializer;
use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

pub struct AvroSerializer;

impl MemtableSerializer for AvroSerializer {
    fn serialize<T: Serialize>(&self, data: &T) -> Result<Vec<u8>> {
        Ok(bincode::serialize(data)?) // Placeholder - implement actual Avro
    }
    
    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T> {
        Ok(bincode::deserialize(data)?) // Placeholder - implement actual Avro
    }
    
    fn name(&self) -> &'static str {
        "avro"
    }
}