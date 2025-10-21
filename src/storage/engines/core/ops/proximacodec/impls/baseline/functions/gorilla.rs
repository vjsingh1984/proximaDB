// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Gorilla Compression - Raw implementation (no headers)
//!
//! Facebook's Gorilla compression using XOR-based encoding for floats.
//! Excellent for time-series floats where consecutive values are similar.
//! Stores only the XOR difference between consecutive values.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use anyhow::Result;

use super::helpers;
use super::helpers::ToWireFormat;

// ===== Core wire format encoding functions =====

/// Core encoding logic for u32 wire type (used by f32 and i32)
fn encode_gorilla_u32_wire(wire_values: &[i32]) -> Result<Vec<u8>> {
    // Convert i32 wire values to u32 for XOR operations
    let u32_values: Vec<u32> = wire_values.iter().map(|&v| v as u32).collect();

    if u32_values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();
    result.extend_from_slice(&u32_values[0].to_le_bytes());

    if u32_values.len() == 1 {
        return Ok(result);
    }

    let mut bit_writer = BitWriter::new();
    let mut prev_value = u32_values[0];
    let mut prev_leading_zeros = 0u8;
    let mut prev_trailing_zeros = 0u8;

    for &current in &u32_values[1..] {
        let xor = prev_value ^ current;

        if xor == 0 {
            bit_writer.write_bit(false);
        } else {
            bit_writer.write_bit(true);

            let leading_zeros = xor.leading_zeros() as u8;
            let trailing_zeros = xor.trailing_zeros() as u8;
            let meaningful_bits = 32 - leading_zeros - trailing_zeros;

            if leading_zeros >= prev_leading_zeros
                && trailing_zeros >= prev_trailing_zeros
                && prev_leading_zeros + prev_trailing_zeros < 32
            {
                bit_writer.write_bit(false);
                let block_size = 32 - prev_leading_zeros - prev_trailing_zeros;
                let mask = if block_size >= 32 {
                    u32::MAX
                } else {
                    (1u32 << block_size) - 1
                };
                let value_bits = (xor >> prev_trailing_zeros) & mask;
                bit_writer.write_bits(value_bits as u64, block_size);
            } else {
                bit_writer.write_bit(true);
                bit_writer.write_bits(leading_zeros as u64, 5);
                bit_writer.write_bits(meaningful_bits as u64, 6);
                let mask = if meaningful_bits >= 32 {
                    u32::MAX
                } else {
                    (1u32 << meaningful_bits) - 1
                };
                let value_bits = (xor >> trailing_zeros) & mask;
                bit_writer.write_bits(value_bits as u64, meaningful_bits);
                prev_leading_zeros = leading_zeros;
                prev_trailing_zeros = trailing_zeros;
            }
        }

        prev_value = current;
    }

    let compressed = bit_writer.finish();
    result.extend(compressed);

    Ok(result)
}

/// Core encoding logic for u64 wire type (used by i64)
fn encode_gorilla_u64_wire(wire_values: &[i64]) -> Result<Vec<u8>> {
    let u64_values: Vec<u64> = wire_values.iter().map(|&v| v as u64).collect();

    if u64_values.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();
    result.extend_from_slice(&u64_values[0].to_le_bytes());

    if u64_values.len() == 1 {
        return Ok(result);
    }

    let mut bit_writer = BitWriter::new();
    let mut prev_value = u64_values[0];
    let mut prev_leading_zeros = 0u8;
    let mut prev_trailing_zeros = 0u8;

    for &current in &u64_values[1..] {
        let xor = prev_value ^ current;

        if xor == 0 {
            bit_writer.write_bit(false);
        } else {
            bit_writer.write_bit(true);

            let leading_zeros = xor.leading_zeros() as u8;
            let trailing_zeros = xor.trailing_zeros() as u8;
            let meaningful_bits = 64 - leading_zeros - trailing_zeros;

            if leading_zeros >= prev_leading_zeros
                && trailing_zeros >= prev_trailing_zeros
                && prev_leading_zeros + prev_trailing_zeros < 64
            {
                bit_writer.write_bit(false);
                let block_size = 64 - prev_leading_zeros - prev_trailing_zeros;
                let mask = if block_size >= 64 {
                    u64::MAX
                } else {
                    (1u64 << block_size) - 1
                };
                let value_bits = (xor >> prev_trailing_zeros) & mask;
                bit_writer.write_bits(value_bits, block_size);
            } else {
                bit_writer.write_bit(true);
                bit_writer.write_bits(leading_zeros as u64, 6);
                bit_writer.write_bits(meaningful_bits as u64, 7);
                let mask = if meaningful_bits >= 64 {
                    u64::MAX
                } else {
                    (1u64 << meaningful_bits) - 1
                };
                let value_bits = (xor >> trailing_zeros) & mask;
                bit_writer.write_bits(value_bits, meaningful_bits);
                prev_leading_zeros = leading_zeros;
                prev_trailing_zeros = trailing_zeros;
            }
        }

        prev_value = current;
    }

    let compressed = bit_writer.finish();
    result.extend(compressed);

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Encode f32 values using Gorilla compression (raw, no headers)
///
/// # Algorithm
/// 1. Store first value as-is
/// 2. For each subsequent value:
///    - XOR with previous value
///    - If XOR == 0: store single control bit (0)
///    - If XOR != 0: store control bit (1) + compressed XOR
///
/// # Format (raw data only, NO headers)
/// ```
/// [first_value:4 bytes][compressed_stream...]
/// ```
///
/// # Parameters
/// - `values`: f32 slice to encode
///
/// # Returns
/// Raw encoded bytes (NO scheme marker, NO count header)
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_gorilla_u32_wire)
}

/// Encode i64 values using Gorilla-style XOR compression
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_gorilla_u64_wire)
}

/// Encode i32 values using Gorilla-style XOR compression
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    helpers::encode_generic(values, encode_gorilla_u32_wire)
}

// ===== Core wire format decoding functions =====

/// Core decoding logic for u32 wire type
fn decode_gorilla_u32_wire(data: &[u8], count: usize) -> Result<Vec<i32>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 4 {
        return Err(anyhow::anyhow!("Gorilla decode: insufficient data"));
    }

    let first_value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let mut result = vec![first_value as i32];

    if count == 1 {
        return Ok(result);
    }

    let mut bit_reader = BitReader::new(&data[4..]);
    let mut prev_value = first_value;
    let mut prev_leading_zeros = 0u8;
    let mut prev_trailing_zeros = 0u8;

    for _ in 1..count {
        let control_bit = bit_reader.read_bit()?;

        if !control_bit {
            result.push(prev_value as i32);
        } else {
            let use_prev_block = !bit_reader.read_bit()?;

            let xor = if use_prev_block {
                let block_size = 32 - prev_leading_zeros - prev_trailing_zeros;
                let value_bits = bit_reader.read_bits(block_size)? as u32;
                value_bits << prev_trailing_zeros
            } else {
                let leading_zeros = bit_reader.read_bits(5)? as u8;
                let meaningful_bits = bit_reader.read_bits(6)? as u8;
                let value_bits = bit_reader.read_bits(meaningful_bits)? as u32;
                let trailing_zeros = 32 - leading_zeros - meaningful_bits;
                prev_leading_zeros = leading_zeros;
                prev_trailing_zeros = trailing_zeros;
                value_bits << trailing_zeros
            };

            let current_value = prev_value ^ xor;
            result.push(current_value as i32);
            prev_value = current_value;
        }
    }

    Ok(result)
}

/// Core decoding logic for u64 wire type
fn decode_gorilla_u64_wire(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }

    if data.len() < 8 {
        return Err(anyhow::anyhow!("Gorilla decode: insufficient data"));
    }

    let first_value = u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let mut result = vec![first_value as i64];

    if count == 1 {
        return Ok(result);
    }

    let mut bit_reader = BitReader::new(&data[8..]);
    let mut prev_value = first_value;
    let mut prev_leading_zeros = 0u8;
    let mut prev_trailing_zeros = 0u8;

    for _ in 1..count {
        let control_bit = bit_reader.read_bit()?;

        if !control_bit {
            result.push(prev_value as i64);
        } else {
            let use_prev_block = !bit_reader.read_bit()?;

            let xor = if use_prev_block {
                let block_size = 64 - prev_leading_zeros - prev_trailing_zeros;
                let value_bits = bit_reader.read_bits(block_size)?;
                value_bits << prev_trailing_zeros
            } else {
                let leading_zeros = bit_reader.read_bits(6)? as u8;
                let meaningful_bits = bit_reader.read_bits(7)? as u8;
                let value_bits = bit_reader.read_bits(meaningful_bits)?;
                let trailing_zeros = 64 - leading_zeros - meaningful_bits;
                prev_leading_zeros = leading_zeros;
                prev_trailing_zeros = trailing_zeros;
                value_bits << trailing_zeros
            };

            let current_value = prev_value ^ xor;
            result.push(current_value as i64);
            prev_value = current_value;
        }
    }

    Ok(result)
}

// ===== Public API (thin wrappers using generic helpers) =====

/// Decode f32 values from Gorilla compressed data
pub fn decode_f32(data: &[u8], count: usize) -> Result<Vec<f32>> {
    helpers::decode_generic::<f32>(data, count, decode_gorilla_u32_wire)
}

/// Decode i64 values from Gorilla compressed data
pub fn decode_i64(data: &[u8], count: usize) -> Result<Vec<i64>> {
    helpers::decode_generic::<i64>(data, count, decode_gorilla_u64_wire)
}

/// Decode i32 values from Gorilla compressed data
pub fn decode_i32(data: &[u8], count: usize) -> Result<Vec<i32>> {
    helpers::decode_generic::<i32>(data, count, decode_gorilla_u32_wire)
}

// ===== Bit-level I/O helpers =====

struct BitWriter {
    bytes: Vec<u8>,
    current_byte: u8,
    bit_pos: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            current_byte: 0,
            bit_pos: 0,
        }
    }

    fn write_bit(&mut self, bit: bool) {
        if bit {
            self.current_byte |= 1 << (7 - self.bit_pos);
        }
        self.bit_pos += 1;

        if self.bit_pos == 8 {
            self.bytes.push(self.current_byte);
            self.current_byte = 0;
            self.bit_pos = 0;
        }
    }

    fn write_bits(&mut self, value: u64, num_bits: u8) {
        for i in (0..num_bits).rev() {
            let bit = (value >> i) & 1 == 1;
            self.write_bit(bit);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit_pos > 0 {
            self.bytes.push(self.current_byte);
        }
        self.bytes
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn read_bit(&mut self) -> Result<bool> {
        if self.byte_pos >= self.data.len() {
            return Err(anyhow::anyhow!("BitReader: unexpected end of data"));
        }

        let bit = (self.data[self.byte_pos] >> (7 - self.bit_pos)) & 1 == 1;
        self.bit_pos += 1;

        if self.bit_pos == 8 {
            self.byte_pos += 1;
            self.bit_pos = 0;
        }

        Ok(bit)
    }

    fn read_bits(&mut self, num_bits: u8) -> Result<u64> {
        let mut value = 0u64;
        for _ in 0..num_bits {
            value = (value << 1) | (self.read_bit()? as u64);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gorilla_identical_values() {
        // All same value - best case for Gorilla
        let values = vec![42.0f32; 100];

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            assert_eq!(orig.to_bits(), dec.to_bits());
        }

        // Should be very small: first value + 100 zero bits
        assert!(encoded.len() < 20);
    }

    #[test]
    fn test_gorilla_similar_values() {
        // Similar values (typical time-series, 32+ values)
        let values: Vec<f32> = (0..32).map(|i| 20.0 + (i as f32 * 0.1)).collect();

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        // Gorilla optimized for floats - should roundtrip exactly
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            let diff = (orig - dec).abs();
            assert!(diff < 0.01, "Expected {}, got {}", orig, dec);
        }
    }

    #[test]
    fn test_gorilla_i64_roundtrip() {
        let values: Vec<i64> = (0..32).map(|i| 1000 + i).collect();

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_gorilla_i32_roundtrip() {
        let values: Vec<i32> = (0..32).map(|i| 100 + i).collect();

        let encoded = encode_i32(&values).unwrap();
        let decoded = decode_i32(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }

    #[test]
    fn test_gorilla_empty() {
        let values: Vec<f32> = vec![];
        let encoded = encode_f32(&values).unwrap();
        assert!(encoded.is_empty());

        let decoded = decode_f32(&encoded, 0).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_gorilla_single_value() {
        let values = vec![42.0f32];

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());
        assert_eq!(values[0].to_bits(), decoded[0].to_bits());
    }

    #[test]
    fn test_gorilla_temperature_sensor() {
        // Simulated temperature sensor data
        let mut values = Vec::new();
        let mut temp = 20.0f32;
        for _ in 0..100 {
            values.push(temp);
            temp += 0.1; // Gradual increase
        }

        let encoded = encode_f32(&values).unwrap();
        let decoded = decode_f32(&encoded, values.len()).unwrap();

        assert_eq!(values.len(), decoded.len());

        // Gorilla optimized for time-series floats
        for (orig, dec) in values.iter().zip(decoded.iter()) {
            let diff = (orig - dec).abs();
            assert!(diff < 0.01, "Expected {}, got {}", orig, dec);
        }
    }

    #[test]
    fn test_gorilla_timestamps() {
        // Simulated timestamps with small increments
        let mut values = Vec::new();
        let mut ts = 1000000i64;
        for _ in 0..100 {
            values.push(ts);
            ts += 1000; // +1 second
        }

        let encoded = encode_i64(&values).unwrap();
        let decoded = decode_i64(&encoded, values.len()).unwrap();

        assert_eq!(values, decoded);
    }
}
