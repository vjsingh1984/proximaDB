//! Bincode serialization for memtable data

use super::MemtableSerializer;
use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};

pub struct BincodeSerializer;

impl MemtableSerializer for BincodeSerializer {
    fn serialize<T: Serialize>(&self, data: &T) -> Result<Vec<u8>> {
        Ok(bincode::serialize(data)?)
    }
    
    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T> {
        Ok(bincode::deserialize(data)?)
    }
    
    fn name(&self) -> &'static str {
        "bincode"
    }
}