//! Compatibility types for SST quantization
//! 
//! Minimal types to maintain compatibility while transitioning to universal quantization.
//! These will be phased out as the codebase migrates to the universal quantization system.

use serde::{Deserialize, Serialize};

/// Compatibility type for PQ codes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PQCode {
    pub codes: Vec<u8>,
}

/// Compatibility type for binary sketches  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySketch {
    pub bits: Vec<u8>,
    pub dimension: usize,
}

/// Compatibility type for INT8 quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Int8Quantization {
    pub quantized_data: Vec<i8>,
    pub scale: f32,
    pub offset: f32,
}

/// Compatibility type for quantized section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedSection {
    pub pq_codes: Vec<PQCode>,
    pub binary_sketches: Vec<BinarySketch>,
    pub int8_vectors: Option<Vec<Int8Quantization>>,
    pub int8_params: Option<(f32, f32)>, // scale, offset
}

impl QuantizedSection {
    /// Create empty quantized section
    pub fn empty() -> Self {
        Self {
            pq_codes: Vec::new(),
            binary_sketches: Vec::new(),
            int8_vectors: None,
            int8_params: None,
        }
    }
    
    /// Default implementation
    pub fn default() -> Self {
        Self::empty()
    }
    
    /// Create from vectors (compatibility method)
    pub fn from_vectors(
        _vectors: &[crate::core::VectorRecord], 
        _config: Option<&str>, 
        _enable_binary: bool
    ) -> Self {
        // Return empty section for now - this is a compatibility shim
        Self::empty()
    }
    
    /// Create from universal quantization data
    pub fn from_storage_quantized_data(
        data: &[crate::compute::quantization::storage_engine::StorageQuantizedData]
    ) -> Self {
        let mut pq_codes = Vec::new();
        let mut binary_sketches = Vec::new();
        
        for item in data {
            // Convert primary quantization to PQ codes
            if let Some(ref primary) = item.primary {
                pq_codes.push(PQCode {
                    codes: primary.data.clone(),
                });
            }
            
            // Convert filter quantization to binary sketches
            if let Some(ref filter) = item.filter {
                binary_sketches.push(BinarySketch {
                    bits: filter.data.clone(),
                    dimension: item.dimension,
                });
            }
        }
        
        Self {
            pq_codes,
            binary_sketches,
            int8_vectors: None, // TODO: Handle fast quantization
            int8_params: None,
        }
    }
}