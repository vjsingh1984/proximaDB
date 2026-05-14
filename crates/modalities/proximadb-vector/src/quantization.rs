//! # Vector Quantization
//!
//! This module provides vector quantization for compression and fast approximate search.
//!
//! ## Quantization Types
//!
//! - **Binary** - 1-bit quantization (0/1)
//! - **Scalar (Int8)** - 8-bit scalar quantization
//! - **Product (PQ)** - Product quantization with sub-vector codebooks
//!
//! ## Note on Engine Consolidation
//!
//! The production quantization engine (7,556 lines) remains in `src/compute/quantization/`
//! due to complex dependencies on storage, core, and compute infrastructure.
//! This will be migrated to vector modality in Phase 6B after dependency untangling.

pub mod binary;
pub mod internal_types;
pub mod product;
pub mod scalar;

use serde::{Deserialize, Serialize};

// Re-export proto types for compatibility
pub use proximadb_proto::v1::{QuantizationConfig, QuantizationLevel};

// Import QuantizationType from nested module
pub use proximadb_proto::v1::quantization_level::QuantizationType;

// Re-export implementations
pub use binary::{BinaryQuantizer, BinaryVector};
pub use product::{PQCodebook, PQVector, ProductQuantizer};
pub use scalar::{Int8Vector, ScalarQuantizer};

/// Quantized vector data
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuantizedVectorData {
    /// Binary quantized (1 bit per dimension)
    Binary(BinaryVector),
    /// Scalar int8 quantized (8 bits per dimension)
    Int8(Int8Vector),
    /// Product quantized (codebook indices)
    PQ(PQVector),
}

impl QuantizedVectorData {
    pub fn quantization_type(&self) -> QuantizationType {
        match self {
            QuantizedVectorData::Binary(_) => QuantizationType::Binary,
            QuantizedVectorData::Int8(_) => QuantizationType::Scalar,
            QuantizedVectorData::PQ(_) => QuantizationType::Product,
        }
    }

    pub fn size_bytes(&self) -> usize {
        match self {
            QuantizedVectorData::Binary(v) => v.data.len(),
            QuantizedVectorData::Int8(v) => v.data.len(),
            QuantizedVectorData::PQ(v) => v.indices.len() * std::mem::size_of::<u8>(),
        }
    }
}

/// Common quantization trait
pub trait Quantizer<T, U> {
    fn train(&mut self, vectors: &[T]) -> Result<(), String>;
    fn quantize(&self, vector: &[T]) -> Result<U, String>;
    fn unquantize(&self, quantized: &U) -> Vec<T>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantization_type() {
        let binary = QuantizedVectorData::Binary(BinaryVector {
            data: vec![0b10101010],
        });
        assert_eq!(binary.quantization_type(), QuantizationType::Binary);

        let int8 = QuantizedVectorData::Int8(Int8Vector {
            data: vec![1i8, 2, 3],
            scale: 0.01,
            min: vec![-1.0, -1.0, -1.0],
        });
        assert_eq!(int8.quantization_type(), QuantizationType::Scalar);
    }

    #[test]
    fn test_size_bytes() {
        let binary = QuantizedVectorData::Binary(BinaryVector {
            data: vec![0b10101010],
        });
        assert_eq!(binary.size_bytes(), 1);

        let int8 = QuantizedVectorData::Int8(Int8Vector {
            data: vec![1i8, 2, 3],
            scale: 0.01,
            min: vec![-1.0, -1.0, -1.0],
        });
        assert_eq!(int8.size_bytes(), 3);
    }
}
