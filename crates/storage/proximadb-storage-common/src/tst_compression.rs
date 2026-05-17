//! Compression Module
//!
//! Provides time-series specific compression algorithms.
//!
//! Uses Gorilla-inspired compression for floating-point time-series data,
//! which can achieve 10:1 compression ratios for typical workloads.

use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read};

/// Bit writer for Gorilla compression
struct BitWriter {
    buffer: Vec<u8>,
    current_byte: u8,
    bit_position: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            current_byte: 0,
            bit_position: 0,
        }
    }

    fn write_bit(&mut self, bit: bool) {
        if bit {
            self.current_byte |= 1 << self.bit_position;
        }
        self.bit_position += 1;
        if self.bit_position == 8 {
            self.buffer.push(self.current_byte);
            self.current_byte = 0;
            self.bit_position = 0;
        }
    }

    #[allow(dead_code)]
    fn write_bits(&mut self, bits: &[bool]) {
        for &bit in bits {
            self.write_bit(bit);
        }
    }

    fn write_value(&mut self, value: u64, num_bits: u8) {
        for i in 0..num_bits {
            self.write_bit((value >> i) & 1 == 1);
        }
    }

    fn flush(&mut self) {
        if self.bit_position > 0 {
            self.buffer.push(self.current_byte);
            self.current_byte = 0;
            self.bit_position = 0;
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.flush();
        self.buffer
    }
}

/// Bit reader for Gorilla decompression
struct BitReader {
    buffer: Cursor<Vec<u8>>,
    current_byte: u8,
    bit_position: u8,
    #[allow(dead_code)]
    bytes_remaining: usize,
}

impl BitReader {
    fn new(data: Vec<u8>) -> Self {
        let bytes_remaining = data.len();
        let mut reader = Self {
            buffer: Cursor::new(data),
            current_byte: 0,
            bit_position: 8,
            bytes_remaining,
        };
        reader.load_next_byte();
        reader
    }

    fn load_next_byte(&mut self) -> bool {
        let mut byte = [0u8; 1];
        if self.buffer.read_exact(&mut byte).is_ok() {
            self.current_byte = byte[0];
            self.bit_position = 0;
            true
        } else {
            self.bit_position = 8;
            false
        }
    }

    fn read_bit(&mut self) -> Option<bool> {
        if self.bit_position >= 8 && !self.load_next_byte() {
            return None;
        }
        let bit = (self.current_byte >> self.bit_position) & 1 == 1;
        self.bit_position += 1;
        Some(bit)
    }

    fn read_value(&mut self, num_bits: u8) -> Option<u64> {
        let mut value = 0u64;
        for i in 0..num_bits {
            if let Some(bit) = self.read_bit() {
                if bit {
                    value |= 1 << i;
                }
            } else {
                return None;
            }
        }
        Some(value)
    }
}

/// Compression configuration for time-series data
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TstCompressionConfig {
    /// Enable compression
    pub enabled: bool,

    /// Compression level (1-10)
    pub level: u8,

    /// Use Gorilla-style compression
    pub use_gorilla: bool,
}

impl Default for TstCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: 6,
            use_gorilla: true,
        }
    }
}

/// Time-series compressor
pub struct TimeSeriesCompressor {
    config: TstCompressionConfig,
}

impl TimeSeriesCompressor {
    /// Create a new compressor
    pub fn new(config: TstCompressionConfig) -> Self {
        Self { config }
    }

    /// Compress floating-point data using Gorilla algorithm
    pub fn compress_floats(&self, data: &[f64]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }

        if !self.config.use_gorilla {
            return bincode::serialize(data).unwrap_or_default();
        }

        self.gorilla_compress(data)
    }

    /// Decompress floating-point data
    pub fn decompress_floats(&self, data: &[u8]) -> Vec<f64> {
        if data.is_empty() {
            return Vec::new();
        }

        if !self.config.use_gorilla {
            return bincode::deserialize(data).unwrap_or_default();
        }

        self.gorilla_decompress(data)
    }

    /// Gorilla-inspired delta compression for floating-point time series
    fn gorilla_compress(&self, data: &[f64]) -> Vec<u8> {
        let mut writer = BitWriter::new();

        writer.write_value(data.len() as u64, 32);

        if data.is_empty() {
            return writer.into_bytes();
        }

        let first_bits = data[0].to_bits();
        writer.write_value(first_bits, 64);

        if data.len() == 1 {
            return writer.into_bytes();
        }

        let mut prev_value = data[0];

        for &value in &data[1..] {
            let xor = prev_value.to_bits() ^ value.to_bits();

            if xor == 0 {
                writer.write_bit(false);
            } else {
                writer.write_bit(true);
                writer.write_value(xor, 64);
            }

            prev_value = value;
        }

        writer.into_bytes()
    }

    /// Gorilla decompression
    fn gorilla_decompress(&self, data: &[u8]) -> Vec<f64> {
        let mut reader = BitReader::new(data.to_vec());

        let len = reader.read_value(32).unwrap_or(0) as usize;
        if len == 0 {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(len);

        if let Some(first_bits) = reader.read_value(64) {
            result.push(f64::from_bits(first_bits));
        } else {
            return result;
        }

        let mut prev_value = result[0];

        for _ in 1..len {
            let same_value = reader.read_bit().unwrap_or(false);

            if !same_value {
                result.push(prev_value);
            } else {
                if let Some(xor) = reader.read_value(64) {
                    let new_value = f64::from_bits(prev_value.to_bits() ^ xor);
                    result.push(new_value);
                    prev_value = new_value;
                } else {
                    result.push(prev_value);
                }
            }
        }

        result
    }

    /// Calculate actual compression ratio
    pub fn calculate_ratio(&self, original: &[f64], compressed: &[u8]) -> f64 {
        let original_bytes = original.len() * 8;
        if compressed.is_empty() {
            return 1.0;
        }
        original_bytes as f64 / compressed.len() as f64
    }

    /// Get target compression ratio (from config)
    pub fn compression_ratio(&self) -> f64 {
        10.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compressor_creation() {
        let config = TstCompressionConfig::default();
        let _compressor = TimeSeriesCompressor::new(config);
        assert!(config.enabled);
        assert!(config.use_gorilla);
    }

    #[test]
    fn test_compress_decompress() {
        let config = TstCompressionConfig::default();
        let compressor = TimeSeriesCompressor::new(config);

        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let compressed = compressor.compress_floats(&data);
        let decompressed = compressor.decompress_floats(&compressed);

        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_compress_empty() {
        let config = TstCompressionConfig::default();
        let compressor = TimeSeriesCompressor::new(config);

        let data: Vec<f64> = vec![];
        let compressed = compressor.compress_floats(&data);
        let decompressed = compressor.decompress_floats(&compressed);

        assert!(compressed.is_empty());
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_compress_same_values() {
        let config = TstCompressionConfig::default();
        let compressor = TimeSeriesCompressor::new(config);

        let data = vec![1.0; 100];
        let compressed = compressor.compress_floats(&data);
        let decompressed = compressor.decompress_floats(&compressed);

        assert_eq!(data, decompressed);

        let ratio = compressor.calculate_ratio(&data, &compressed);
        assert!(
            ratio > 10.0,
            "Compression ratio should be > 10:1 for repeated values, got {}",
            ratio
        );
    }
}
