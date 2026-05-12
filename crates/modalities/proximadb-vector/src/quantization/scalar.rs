//! # Scalar (Int8) Quantization
//!
//! 8-bit scalar quantization for vectors.

use serde::{Deserialize, Serialize};

/// Int8 quantized vector
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Int8Vector {
    /// Quantized int8 values
    pub data: Vec<i8>,
    /// Scale factor for reconstruction
    pub scale: f32,
    /// Minimum values per dimension for reconstruction
    pub min: Vec<f32>,
}

impl Int8Vector {
    pub fn from_f32(vector: &[f32], scale: f32, min: &[f32]) -> Self {
        let data = vector
            .iter()
            .zip(min.iter())
            .map(|(&val, &m)| {
                let normalized = (val - m) / scale;
                normalized.clamp(i8::MIN as f32, i8::MAX as f32) as i8
            })
            .collect();

        Self {
            data,
            scale,
            min: min.to_vec(),
        }
    }

    pub fn to_f32(&self) -> Vec<f32> {
        self.data
            .iter()
            .zip(self.min.iter())
            .map(|(&val, &m)| (val as f32) * self.scale + m)
            .collect()
    }

    pub fn dimensions(&self) -> usize {
        self.data.len()
    }

    pub fn compression_ratio(&self) -> f32 {
        (self.data.len() * std::mem::size_of::<f32>()) as f32 / self.data.len() as f32
    }
}

/// Scalar Int8 quantizer
pub struct ScalarQuantizer {
    scale: f32,
    trained: bool,
}

impl ScalarQuantizer {
    pub fn new() -> Self {
        Self {
            scale: 0.01,
            trained: false,
        }
    }

    pub fn with_scale(scale: f32) -> Self {
        Self {
            scale,
            trained: true,
        }
    }

    /// Train on a set of vectors to determine optimal scale
    pub fn train(&mut self, vectors: &[Vec<f32>]) -> Result<(), String> {
        if vectors.is_empty() {
            return Err("Cannot train on empty vector set".to_string());
        }

        let dims = vectors[0].len();
        if !vectors.iter().all(|v| v.len() == dims) {
            return Err("All vectors must have same dimension".to_string());
        }

        // Find range across all dimensions
        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;

        for vec in vectors {
            for &val in vec {
                min_val = min_val.min(val);
                max_val = max_val.max(val);
            }
        }

        let range = max_val - min_val;
        self.scale = range / (i8::MAX as f32 - i8::MIN as f32).max(1.0);
        self.trained = true;

        Ok(())
    }

    pub fn quantize(&self, vector: &[f32], min: &[f32]) -> Int8Vector {
        Int8Vector::from_f32(vector, self.scale, min)
    }

    pub fn unquantize(&self, quantized: &Int8Vector) -> Vec<f32> {
        quantized.to_f32()
    }
}

impl Default for ScalarQuantizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scalar_quantizer() {
        let mut quantizer = ScalarQuantizer::new();
        let vectors = vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]];

        quantizer.train(&vectors).unwrap();
        assert!(quantizer.trained);

        let min = &[0.0, 0.0, 0.0];
        let quantized = quantizer.quantize(&vectors[0], min);

        let reconstructed = quantizer.unquantize(&quantized);
        assert_eq!(reconstructed.len(), 3);
    }

    #[test]
    fn test_compression_ratio() {
        let vector = Int8Vector::from_f32(&[1.0, 2.0, 3.0], 0.01, &[0.0, 0.0, 0.0]);
        assert_eq!(vector.compression_ratio(), 4.0); // f32 is 4 bytes, i8 is 1 byte
    }
}
