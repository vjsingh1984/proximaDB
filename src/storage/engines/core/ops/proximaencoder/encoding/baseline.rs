// # Baseline Encoding Algorithms
//
// Core encoding algorithms providing fundamental compression schemes.
// These are standalone functions extracted from ProximaEncoder.

use anyhow::Result;

/// Bit-packing with SIMD-friendly layout
/// Uses transposed bit-packing for better auto-vectorization
///
/// # Parameters
/// - `data`: Integer slice to encode
/// - `bits`: Number of bits per value (1-64)
/// - `block_size`: Block size for chunking (typically 128 or 256)
///
/// # Returns
/// Byte vector containing bit-packed data
///
/// # Performance
/// - Compression: 1.5-3x depending on data range
/// - Best for: Integers with known bit width < 64
pub fn bitpack_integers(data: &[i64], bits: u8, block_size: usize) -> Result<Vec<u8>> {
    if bits > 64 {
        return Err(anyhow::anyhow!("Bit width {} exceeds 64", bits));
    }

    let mut encoded = Vec::new();

    // Process in blocks for SIMD efficiency
    for chunk in data.chunks(block_size) {
        // For each value in the chunk, pack its bits
        // Process 8 values at a time to fill bytes
        for value_group in chunk.chunks(8) {
            // For each bit position, collect bits from up to 8 values into a byte
            for bit_pos in 0..bits {
                let mut byte = 0u8;

                for (idx, &value) in value_group.iter().enumerate() {
                    let bit = ((value as u64 >> bit_pos) & 1) as u8;
                    byte |= bit << idx;
                }

                encoded.push(byte);
            }
        }
    }

    Ok(encoded)
}

/// Delta encoding with fixed base
///
/// # Parameters
/// - `data`: Integer slice to encode
/// - `base`: Base value for delta calculation
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [base:i64][bits:u8][bitpacked_deltas]
/// ```
///
/// # Performance
/// - Compression: 2-4x for monotonic sequences
/// - Best for: Sequential or near-sequential data
#[inline(always)] // Encourage auto-vectorization
pub fn delta_encode(data: &[i64], base: i64, block_size: usize) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    // Store base value
    encoded.extend_from_slice(&base.to_le_bytes());

    // Compute deltas using wrapping arithmetic to avoid overflow
    // This is safe because we'll use wrapping_add during decode as well
    let deltas: Vec<i64> = data.iter()
        .map(|&v| v.wrapping_sub(base))
        .collect();

    // Determine optimal bit width for deltas
    // Use unsigned comparison for bit width calculation
    let max_delta = deltas.iter()
        .map(|&d| d.unsigned_abs())
        .max()
        .unwrap_or(0);
    let bits = if max_delta == 0 {
        1
    } else {
        64 - max_delta.leading_zeros() as u8
    };
    encoded.push(bits);

    // Bit-pack the deltas
    let packed = bitpack_integers(&deltas, bits, block_size)?;
    encoded.extend(packed);

    Ok(encoded)
}

/// Frame of Reference (FOR) encoding
///
/// # Parameters
/// - `data`: Integer slice to encode
/// - `reference`: Reference value
/// - `bits`: Bit width for offsets
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [reference:i64][bits:u8][bitpacked_offsets]
/// ```
///
/// # Performance
/// - Compression: 3-6x for normalized data
/// - Best for: Clustered values with small range
pub fn frame_of_reference_encode(data: &[i64], reference: i64, bits: u8, block_size: usize) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    // Store reference value and bit width
    encoded.extend_from_slice(&reference.to_le_bytes());
    encoded.push(bits);

    // Transform to frame of reference (auto-vectorized)
    let transformed: Vec<i64> = data.iter().map(|&v| v - reference).collect();

    // Bit-pack transformed values
    let packed = bitpack_integers(&transformed, bits, block_size)?;
    encoded.extend(packed);

    Ok(encoded)
}

/// Patched base encoding for data with outliers
///
/// # Parameters
/// - `data`: Integer slice to encode
/// - `base`: Base value
/// - `patch_bits`: Bit width for regular values
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [base:i64][patch_bits:u8][num_regular:usize][bitpacked_regular]
/// [num_patches:u32]([position:u32][value:i64])*
/// ```
///
/// # Performance
/// - Compression: 2-5x depending on outlier frequency
/// - Best for: Data with occasional large outliers
pub fn patched_base_encode(data: &[i64], base: i64, patch_bits: u8, block_size: usize) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();
    let threshold = 1i64 << patch_bits;

    // Store base and patch bit width
    encoded.extend_from_slice(&base.to_le_bytes());
    encoded.push(patch_bits);

    // Separate regular values and outliers
    let mut regular_values = Vec::new();
    let mut patches = Vec::new();

    for (idx, &value) in data.iter().enumerate() {
        let delta = value - base;
        if delta.abs() < threshold {
            regular_values.push(delta);
        } else {
            patches.push((idx as u32, value));
        }
    }

    // Encode regular values
    let regular_bits = patch_bits;
    let regular_packed = bitpack_integers(&regular_values, regular_bits, block_size)?;
    encoded.extend_from_slice(&(regular_values.len()).to_le_bytes());
    encoded.extend(regular_packed);

    // Encode patches
    encoded.extend_from_slice(&(patches.len() as u32).to_le_bytes());
    for (idx, value) in patches {
        encoded.extend_from_slice(&idx.to_le_bytes());
        encoded.extend_from_slice(&value.to_le_bytes());
    }

    Ok(encoded)
}

/// Run-length encoding for repeated values
///
/// # Parameters
/// - `data`: Integer slice to encode
///
/// # Wire Format
/// ```
/// [count:u32][value:i64][count:u32][value:i64]...
/// ```
///
/// # Performance
/// - Compression: 10-100x for constant data
/// - Best for: Constant or highly repetitive data
pub fn run_length_encode(data: &[i64]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        return Ok(encoded);
    }

    // RLE format: [count:u32][value:i64][count:u32][value:i64]...
    let mut i = 0;
    while i < data.len() {
        let value = data[i];
        let mut count = 1u32;

        // Count consecutive identical values
        while (i + count as usize) < data.len() && data[i + count as usize] == value {
            count += 1;
            // Limit run length to u32::MAX
            if count == u32::MAX {
                break;
            }
        }

        // Write count and value
        encoded.extend_from_slice(&count.to_le_bytes());
        encoded.extend_from_slice(&value.to_le_bytes());

        i += count as usize;
    }

    Ok(encoded)
}

/// Uncompressed encoding (passthrough)
///
/// # Parameters
/// - `data`: Integer slice to encode
///
/// # Returns
/// Raw byte representation of data
pub fn encode_uncompressed(data: &[i64]) -> Result<Vec<u8>> {
    let mut encoded = Vec::with_capacity(data.len() * 8);
    for &value in data {
        encoded.extend_from_slice(&value.to_le_bytes());
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitpack_integers() {
        let data = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let encoded = bitpack_integers(&data, 3, 128).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_delta_encode() {
        let data = vec![100, 101, 102, 103, 104];
        let encoded = delta_encode(&data, 100, 128).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_frame_of_reference() {
        let data = vec![1000, 1001, 1002, 1003];
        let encoded = frame_of_reference_encode(&data, 1000, 4, 128).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_run_length_encode() {
        let data = vec![5, 5, 5, 5, 7, 7, 9];
        let encoded = run_length_encode(&data).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_uncompressed() {
        let data = vec![1, 2, 3, 4, 5];
        let encoded = encode_uncompressed(&data).unwrap();
        assert_eq!(encoded.len(), data.len() * 8);
    }
}
