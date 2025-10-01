// # Baseline Decoding Algorithms
//
// Core decoding algorithms for fundamental compression schemes.
// These are standalone functions extracted from ProximaDecoder.

use anyhow::Result;

/// Unpack bit-packed integers
///
/// # Parameters
/// - `data`: Byte slice containing packed data
/// - `count`: Number of integers to extract
/// - `bits`: Number of bits per integer (1-64)
///
/// # Returns
/// Vector of unpacked i64 values
pub fn unpack_integers(data: &[u8], count: usize, bits: u8) -> Result<Vec<i64>> {
    let mut values = Vec::with_capacity(count);
    let mut offset = 0;

    // Process values 8 at a time (matching the packing)
    while values.len() < count {
        let remaining = count - values.len();
        let values_in_group = remaining.min(8);

        // Extract values from this group
        for value_idx in 0..values_in_group {
            let mut value = 0u64;

            for bit_pos in 0..bits {
                let byte_idx = offset + bit_pos as usize;
                if byte_idx >= data.len() {
                    break;
                }

                let byte = data[byte_idx];
                let bit = ((byte >> value_idx) & 1) as u64;
                value |= bit << bit_pos;
            }

            values.push(value as i64);
        }

        offset += bits as usize;
    }

    values.truncate(count);
    Ok(values)
}

/// Decode delta-encoded data
///
/// # Parameters
/// - `data`: Byte slice containing delta-encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [base:i64][bits:u8][bitpacked_deltas]
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn delta_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if data.len() < 9 {
        return Err(anyhow::anyhow!("Invalid delta-encoded data"));
    }

    // Read base value
    let base = i64::from_le_bytes(data[0..8].try_into()?);
    let bits = data[8];

    // Decode deltas
    let deltas = unpack_integers(&data[9..], count, bits)?;

    // Apply deltas using wrapping arithmetic to match encoder
    let values: Vec<i64> = deltas.iter()
        .map(|&delta| base.wrapping_add(delta))
        .collect();

    Ok(values)
}

/// Decode frame of reference data
///
/// # Parameters
/// - `data`: Byte slice containing FOR-encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [reference:i64][bits:u8][bitpacked_offsets]
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn frame_of_reference_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if data.len() < 9 {
        return Err(anyhow::anyhow!("Invalid FOR-encoded data"));
    }

    // Read reference and bit width
    let reference = i64::from_le_bytes(data[0..8].try_into()?);
    let bits = data[8];

    // Decode transformed values
    let transformed = unpack_integers(&data[9..], count, bits)?;

    // Apply reference (auto-vectorized)
    let values: Vec<i64> = transformed.iter().map(|&v| reference + v).collect();

    Ok(values)
}

/// Decode patched base data
///
/// # Parameters
/// - `data`: Byte slice containing patched base encoded data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [base:i64][patch_bits:u8][num_regular:usize][bitpacked_regular]
/// [num_patches:u32]([position:u32][value:i64])*
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn patched_base_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;

    // Read base and patch bits
    let base = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
    offset += 8;
    let patch_bits = data[offset];
    offset += 1;

    // Read regular values count
    let regular_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Decode regular values
    let regular_data = &data[offset..];
    let regular_values = unpack_integers(regular_data, regular_count, patch_bits)?;
    offset += (regular_count * patch_bits as usize + 7) / 8;

    // Read patches count
    let patch_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Build result with patches
    let mut values = vec![0i64; count];
    let mut regular_idx = 0;

    // Apply regular values
    for i in 0..count {
        if regular_idx < regular_values.len() {
            values[i] = base + regular_values[regular_idx];
            regular_idx += 1;
        }
    }

    // Apply patches
    for _ in 0..patch_count {
        let idx = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        if idx < values.len() {
            values[idx] = value;
        }
    }

    Ok(values)
}

/// Run-length decode
///
/// # Parameters
/// - `data`: Byte slice containing RLE data
/// - `count`: Number of values to decode
///
/// # Wire Format
/// ```
/// [count:u32][value:i64][count:u32][value:i64]...
/// ```
///
/// # Returns
/// Vector of decoded i64 values
pub fn run_length_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut values = Vec::with_capacity(count);
    let mut offset = 0;

    // RLE format: [count:u32][value:i64][count:u32][value:i64]...
    while values.len() < count && offset < data.len() {
        // Read run count
        if offset + 4 > data.len() {
            break;
        }
        let run_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Read value
        if offset + 8 > data.len() {
            break;
        }
        let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        // Expand the run
        for _ in 0..run_count.min(count - values.len()) {
            values.push(value);
        }
    }

    // If we didn't get enough values, pad with zeros (shouldn't happen with valid data)
    while values.len() < count {
        values.push(0);
    }

    Ok(values)
}

/// Decode uncompressed data
///
/// # Parameters
/// - `data`: Byte slice containing raw i64 values
/// - `count`: Number of values to decode
///
/// # Returns
/// Vector of decoded i64 values
pub fn decode_uncompressed(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut values = Vec::with_capacity(count);

    for i in 0..count {
        let offset = i * 8;
        if offset + 8 > data.len() {
            break;
        }
        let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        values.push(value);
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::ops::proximaencoder::encoding::baseline::*;

    #[test]
    fn test_unpack_integers() {
        // Pack then unpack (32 values)
        let data: Vec<i64> = (0..32).collect();
        let packed = bitpack_integers(&data, 6, 128).unwrap();
        let unpacked = unpack_integers(&packed, data.len(), 6).unwrap();
        assert_eq!(unpacked, data);
    }

    #[test]
    fn test_delta_roundtrip() {
        // Delta encoding with 32 values
        let data: Vec<i64> = (100..132).collect();
        let encoded = delta_encode(&data, 100, 128).unwrap();
        let decoded = delta_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_frame_of_reference_roundtrip() {
        // Frame of reference with 32 values
        let data: Vec<i64> = (1000..1032).collect();
        let encoded = frame_of_reference_encode(&data, 1000, 6, 128).unwrap();
        let decoded = frame_of_reference_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_run_length_roundtrip() {
        // RLE with 32 values (repeated pattern)
        let mut data = vec![5; 10];
        data.extend(vec![7; 12]);
        data.extend(vec![9; 10]);
        let encoded = run_length_encode(&data).unwrap();
        let decoded = run_length_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_uncompressed_roundtrip() {
        // Uncompressed with 32 values
        let data: Vec<i64> = (1..33).collect();
        let encoded = encode_uncompressed(&data).unwrap();
        let decoded = decode_uncompressed(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }
}
