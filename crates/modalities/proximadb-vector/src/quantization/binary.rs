//! # Binary Quantization
//!
//! 1-bit quantization for vectors.

use serde::{Deserialize, Serialize};

/// Binary quantized vector (1 bit per dimension)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryVector {
    /// Packed binary data (8 dimensions per byte)
    pub data: Vec<u8>,
}

impl BinaryVector {
    pub fn new(dimensions: usize) -> Self {
        let bytes = dimensions.div_ceil(8);
        Self {
            data: vec![0u8; bytes],
        }
    }

    pub fn from_f32(vector: &[f32]) -> Self {
        let mut result = Self::new(vector.len());
        for (i, &val) in vector.iter().enumerate() {
            if val >= 0.0 {
                result.set_bit(i, true);
            }
        }
        result
    }

    pub fn get_bit(&self, index: usize) -> bool {
        let byte_index = index / 8;
        let bit_index = index % 8;
        (self.data[byte_index] >> bit_index) & 1 == 1
    }

    pub fn set_bit(&mut self, index: usize, value: bool) {
        let byte_index = index / 8;
        let bit_index = index % 8;
        if value {
            self.data[byte_index] |= 1 << bit_index;
        } else {
            self.data[byte_index] &= !(1 << bit_index);
        }
    }

    pub fn dimensions(&self) -> usize {
        self.data.len() * 8
    }

    /// Calculate Hamming distance between two binary vectors
    pub fn hamming_distance(&self, other: &BinaryVector) -> usize {
        let mut distance = 0;
        for (a, b) in self.data.iter().zip(other.data.iter()) {
            distance += (a ^ b).count_ones() as usize;
        }
        distance
    }
}

/// Binary quantizer
pub struct BinaryQuantizer;

impl BinaryQuantizer {
    pub fn new() -> Self {
        Self
    }

    pub fn quantize(&self, vector: &[f32]) -> BinaryVector {
        BinaryVector::from_f32(vector)
    }

    pub fn batch_quantize(&self, vectors: &[Vec<f32>]) -> Vec<BinaryVector> {
        vectors.iter().map(|v| self.quantize(v)).collect()
    }
}

impl Default for BinaryQuantizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_vector() {
        let vector = vec![1.0, -1.0, 0.5, -0.5];
        let binary = BinaryVector::from_f32(&vector);

        assert!(binary.get_bit(0)); // 1.0 >= 0
        assert!(!binary.get_bit(1)); // -1.0 < 0
        assert!(binary.get_bit(2)); // 0.5 >= 0
        assert!(!binary.get_bit(3)); // -0.5 < 0
    }

    #[test]
    fn test_hamming_distance() {
        let a = BinaryVector::from_f32(&[1.0, 1.0, 1.0]);
        let b = BinaryVector::from_f32(&[1.0, -1.0, 1.0]);
        assert_eq!(a.hamming_distance(&b), 1);
    }
}
