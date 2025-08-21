//! Internal quantization types (Release 1 - no legacy compatibility)
//! 
//! These types are used internally for quantization operations.
//! The proto QuantizationConfig is simplified for user-facing API.

use serde::{Deserialize, Serialize};

/// Unified quantization level configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct UnifiedQuantizationLevel {
    pub level_type: Option<QuantizationLevelType>,
}

/// Quantization level types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum QuantizationLevelType {
    None(NoQuantization),
    Uniform(UniformQuantization),
    Pq(ProductQuantization),
    Scalar(ScalarQuantization),
    Binary(BinaryQuantization),
    Custom(CustomQuantization),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NoQuantization {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UniformQuantization {
    pub bits: i32,
    pub scale: Option<f32>,
    pub offset: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProductQuantization {
    pub bits_per_code: i32,
    pub num_subvectors: i32,
    pub codebook_id: Option<String>,
    pub adaptive_subvectors: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScalarQuantization {
    pub bits: i32,
    pub scale: f32,
    pub offset: f32,
    pub clamp_values: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinaryQuantization {
    pub threshold: Option<f32>,
    pub sign_based: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomQuantization {
    pub type_id: String,
    pub bits_per_element: i32,
    pub config: std::collections::HashMap<String, String>,
}

impl UnifiedQuantizationLevel {
    /// Common quantization level constants for easy access
    pub const Binary: Self = Self {
        level_type: Some(QuantizationLevelType::Binary(BinaryQuantization {
            threshold: None,
            sign_based: false,
        })),
    };
    
    pub const Int8: Self = Self {
        level_type: Some(QuantizationLevelType::Scalar(ScalarQuantization {
            bits: 8,
            scale: 1.0,
            offset: 0.0,
            clamp_values: true,
        })),
    };
    
    /// Create a PQ4 constant (requires runtime initialization due to parameter)
    pub const Pq4: Self = Self {
        level_type: Some(QuantizationLevelType::Pq(ProductQuantization {
            bits_per_code: 4,
            num_subvectors: 8, // Default value
            codebook_id: None,
            adaptive_subvectors: false,
        })),
    };
    
    /// Create a PQ8 constant (requires runtime initialization due to parameter)
    pub const Pq8: Self = Self {
        level_type: Some(QuantizationLevelType::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 8, // Default value
            codebook_id: None,
            adaptive_subvectors: false,
        })),
    };
    
    /// Create a PQ8 configuration (common case)
    pub fn pq8(num_subvectors: u8) -> Self {
        Self {
            level_type: Some(QuantizationLevelType::Pq(ProductQuantization {
                bits_per_code: 8,
                num_subvectors: num_subvectors as i32,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }
    }
    
    /// Create a PQ4 configuration (higher compression)
    pub fn pq4(num_subvectors: u8) -> Self {
        Self {
            level_type: Some(QuantizationLevelType::Pq(ProductQuantization {
                bits_per_code: 4,
                num_subvectors: num_subvectors as i32,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }
    }
    
    /// Create an INT8 scalar quantization
    pub fn int8() -> Self {
        Self {
            level_type: Some(QuantizationLevelType::Scalar(ScalarQuantization {
                bits: 8,
                scale: 1.0,
                offset: 0.0,
                clamp_values: true,
            })),
        }
    }
    
    /// Create a binary quantization
    pub fn binary() -> Self {
        Self {
            level_type: Some(QuantizationLevelType::Binary(BinaryQuantization {
                threshold: None,
                sign_based: false,
            })),
        }
    }
    
    /// Get the number of bits per element
    pub fn bits_per_element(&self) -> u32 {
        match &self.level_type {
            Some(QuantizationLevelType::Pq(pq)) => pq.bits_per_code as u32,
            Some(QuantizationLevelType::Scalar(sq)) => sq.bits as u32,
            Some(QuantizationLevelType::Binary(_)) => 1,
            Some(QuantizationLevelType::Uniform(uq)) => uq.bits as u32,
            Some(QuantizationLevelType::Custom(cq)) => cq.bits_per_element as u32,
            _ => 32, // Full precision
        }
    }
    
    /// Calculate bytes per vector based on quantization level
    pub fn bytes_per_vector(&self, dimension: usize) -> usize {
        match &self.level_type {
            Some(QuantizationLevelType::Pq(pq)) => {
                let codes_per_vector = pq.num_subvectors as usize;
                let bytes_per_code = ((pq.bits_per_code + 7) / 8) as usize;
                codes_per_vector * bytes_per_code
            }
            Some(QuantizationLevelType::Scalar(sq)) => {
                dimension * ((sq.bits + 7) / 8) as usize
            }
            Some(QuantizationLevelType::Binary(_)) => {
                (dimension + 7) / 8  // 1 bit per dimension
            }
            Some(QuantizationLevelType::Uniform(uq)) => {
                dimension * ((uq.bits + 7) / 8) as usize
            }
            Some(QuantizationLevelType::Custom(cq)) => {
                dimension * ((cq.bits_per_element + 7) / 8) as usize
            }
            _ => dimension * 4,  // Full FP32 precision
        }
    }
    
    /// Get compression ratio compared to FP32
    pub fn compression_ratio(&self, dimension: usize) -> f32 {
        let fp32_bytes = dimension * 4;
        let compressed_bytes = self.bytes_per_vector(dimension);
        fp32_bytes as f32 / compressed_bytes.max(1) as f32
    }
}