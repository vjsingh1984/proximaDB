// # Advanced Decoding Algorithms
//
// State-of-the-art decoding algorithms for specialized compression scenarios.
// These are standalone functions extracted from ProximaDecoder.

use anyhow::Result;
use std::collections::HashMap;

// Import baseline functions we depend on
use super::baseline::unpack_integers;

/// **PForDelta Decoder** - Decode Patched Frame of Reference Delta
///
/// # Parameters
/// - `data`: Byte slice containing PForDelta encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [reference:i64][majority_bits:u8][num_values:u32][bitpacked_deltas]
/// [num_exceptions:u32]([position:u32][value:i64])*
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn pfor_delta_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;

    // Read reference value
    if offset + 8 > data.len() {
        return Err(anyhow::anyhow!("PForDelta: insufficient data for reference"));
    }
    let reference = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
    offset += 8;

    // Read majority bit width
    if offset >= data.len() {
        return Err(anyhow::anyhow!("PForDelta: insufficient data for bit width"));
    }
    let majority_bits = data[offset];
    offset += 1;

    // Read number of regular values
    if offset + 4 > data.len() {
        return Err(anyhow::anyhow!("PForDelta: insufficient data for value count"));
    }
    let num_values = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Decode regular values (bitpacked deltas)
    let regular_deltas = unpack_integers(&data[offset..], num_values, majority_bits)?;

    // Calculate offset for exceptions
    let bits_needed = (num_values * majority_bits as usize + 7) / 8;
    offset += bits_needed;

    // Read number of exceptions
    if offset + 4 > data.len() {
        return Err(anyhow::anyhow!("PForDelta: insufficient data for exception count"));
    }
    let num_exceptions = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Read exceptions
    let mut exceptions = HashMap::new();
    for _ in 0..num_exceptions {
        if offset + 12 > data.len() {
            return Err(anyhow::anyhow!("PForDelta: insufficient data for exception"));
        }
        let pos = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;
        exceptions.insert(pos, value);
    }

    // Reconstruct original values
    let mut values = Vec::with_capacity(count);
    for (idx, &delta) in regular_deltas.iter().enumerate() {
        if let Some(&exception_value) = exceptions.get(&idx) {
            // Use exception value
            values.push(exception_value);
        } else {
            // Regular value: reference + delta
            values.push(reference + delta);
        }
    }

    Ok(values)
}

/// **Zigzag Decoder** - Decode zigzag-encoded signed integers
///
/// Reverses zigzag transformation:
/// ```
/// decode(n) = (n >> 1) ^ -(n & 1)
/// ```
/// This converts: [3, 1, 0, 2, 4] → [-2, -1, 0, 1, 2]
///
/// # Parameters
/// - `data`: Byte slice containing zigzag encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [bits:u8][bitpacked_zigzag_values]
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn zigzag_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;

    // Read bit width
    if offset >= data.len() {
        return Err(anyhow::anyhow!("Zigzag: insufficient data for bit width"));
    }
    let bits = data[offset];
    offset += 1;

    // Decode bitpacked zigzag values
    let zigzag_values = unpack_integers(&data[offset..], count, bits)?;

    // Reverse zigzag transformation
    let values: Vec<i64> = zigzag_values.iter()
        .map(|&n| {
            let u = n as u64;
            ((u >> 1) as i64) ^ (-((u & 1) as i64))
        })
        .collect();

    Ok(values)
}

/// **Simple8b Decoder** - Decode variable bit-width 64-bit words
///
/// Each word: [4-bit selector][60 bits of packed data]
/// Selector determines how many values and bits per value.
///
/// # Parameters
/// - `data`: Byte slice containing Simple8b encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [num_words:u32]([selector:u8][packed_data:u64])*
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn simple8b_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;

    // Read number of words
    if offset + 4 > data.len() {
        return Err(anyhow::anyhow!("Simple8b: insufficient data for word count"));
    }
    let num_words = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Simple8b packing configurations (must match encoder)
    const CONFIGS: [(usize, u8); 16] = [
        (60, 1), (30, 2), (20, 3), (15, 4),
        (12, 5), (10, 6), (8, 7), (7, 8),
        (6, 10), (5, 12), (4, 15), (3, 20),
        (2, 30), (1, 60), (1, 60), (1, 60),
    ];

    let mut values = Vec::with_capacity(count);

    // Decode each word
    for _ in 0..num_words {
        if offset + 8 > data.len() {
            break;
        }
        let word = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        // Extract selector (top 4 bits)
        let selector = (word >> 60) as usize;
        if selector >= 16 {
            return Err(anyhow::anyhow!("Simple8b: invalid selector {}", selector));
        }

        let (values_in_word, bits) = CONFIGS[selector];

        // Extract values from word
        for idx in 0..values_in_word {
            if values.len() >= count {
                break;
            }
            let shift = idx * bits as usize;
            let mask = (1u64 << bits) - 1;
            let value = (word >> shift) & mask;
            values.push(value as i64);
        }

        if values.len() >= count {
            break;
        }
    }

    // Ensure we have exactly count values
    values.truncate(count);
    while values.len() < count {
        values.push(0);
    }

    Ok(values)
}

/// **VByte Decoder** - Decode variable-byte encoded integers
///
/// Each byte: [continuation_bit:1][data:7]
/// Continuation bit = 1 means more bytes follow.
///
/// # Parameters
/// - `data`: Byte slice containing VByte encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// ([continuation_bit:1][data:7])*
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn vbyte_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut values = Vec::with_capacity(count);
    let mut offset = 0;

    while values.len() < count && offset < data.len() {
        let mut value = 0u64;
        let mut shift = 0;

        loop {
            if offset >= data.len() {
                return Err(anyhow::anyhow!("VByte: unexpected end of data"));
            }

            let byte = data[offset];
            offset += 1;

            // Extract 7 bits of data
            value |= ((byte & 0x7F) as u64) << shift;
            shift += 7;

            // Check continuation bit
            if (byte & 0x80) == 0 {
                // Last byte for this value
                break;
            }

            if shift >= 64 {
                return Err(anyhow::anyhow!("VByte: value overflow"));
            }
        }

        values.push(value as i64);
    }

    // Ensure we have exactly count values
    if values.len() != count {
        return Err(anyhow::anyhow!(
            "VByte: expected {} values, got {}",
            count,
            values.len()
        ));
    }

    Ok(values)
}

/// **DoubleDelta Decoder** - Decode delta-of-deltas encoding
///
/// Reverses double_delta_encode:
/// 1. Read first value
/// 2. Read first delta
/// 3. Read double deltas (bitpacked)
/// 4. Reconstruct deltas by accumulating double deltas
/// 5. Reconstruct values by accumulating deltas
///
/// # Parameters
/// - `data`: Byte slice containing DoubleDelta encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [first_value:i64][first_delta:i64][ddelta_bits:u8][bitpacked_ddeltas]
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn double_delta_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;
    let mut values = Vec::with_capacity(count);

    if count == 0 {
        return Ok(values);
    }

    // Read first value
    if offset + 8 > data.len() {
        return Err(anyhow::anyhow!("DoubleDelta: insufficient data for first value"));
    }
    let first_value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
    offset += 8;
    values.push(first_value);

    if count == 1 {
        return Ok(values);
    }

    // Read first delta
    if offset + 8 > data.len() {
        return Err(anyhow::anyhow!("DoubleDelta: insufficient data for first delta"));
    }
    let first_delta = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
    offset += 8;
    values.push(first_value + first_delta);

    if count == 2 {
        return Ok(values);
    }

    // Read bit width for double deltas
    if offset >= data.len() {
        return Err(anyhow::anyhow!("DoubleDelta: insufficient data for bit width"));
    }
    let bits = data[offset];
    offset += 1;

    // Decode double deltas
    let num_ddeltas = count - 2;
    let double_deltas = unpack_integers(&data[offset..], num_ddeltas, bits)?;

    // Reconstruct deltas and values
    let mut prev_delta = first_delta;
    let mut prev_value = values[1];

    for &ddelta in &double_deltas {
        let delta = prev_delta + ddelta;
        let value = prev_value + delta;
        values.push(value);
        prev_delta = delta;
        prev_value = value;
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::ops::proximaencoder::encoding::advanced::*;

    #[test]
    fn test_pfor_delta_roundtrip() {
        // PForDelta with 32 values and outliers
        let mut data: Vec<i64> = (100..130).collect();
        data.push(500); // Outlier
        data.push(131); // Regular
        let encoded = pfor_delta_encode(&data, 128).unwrap();
        let decoded = pfor_delta_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_zigzag_roundtrip() {
        // Zigzag with 32 mixed signed values
        let data: Vec<i64> = (-16..16).collect();
        let encoded = zigzag_encode(&data, 128).unwrap();
        let decoded = zigzag_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_simple8b_roundtrip() {
        // Simple8b with 32 small values
        let data: Vec<i64> = (1..33).collect();
        let encoded = simple8b_encode(&data).unwrap();
        let decoded = simple8b_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_vbyte_roundtrip() {
        // VByte with 32 varying size values
        let data = vec![
            1, 2, 10, 50, 100, 127, 128, 255,
            256, 1000, 5000, 10000, 16383, 16384, 32767, 32768,
            65535, 65536, 100000, 200000, 500000, 1000000, 2000000, 5000000,
            10000000, 20000000, 50000000, 100000000, 200000000, 500000000, 1000000000, 2000000000,
        ];
        let encoded = vbyte_encode(&data).unwrap();
        let decoded = vbyte_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_double_delta_roundtrip() {
        // DoubleDelta with 32 values (constant deltas)
        let data: Vec<i64> = (0..32).collect();
        let encoded = double_delta_encode(&data, 128).unwrap();
        let decoded = double_delta_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }
}
