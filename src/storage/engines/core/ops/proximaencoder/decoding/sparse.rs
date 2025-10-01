// # Sparse Decoding Algorithms
//
// Specialized decoders for sparse data (vectors with many zeros).

use anyhow::Result;

/// **SparseBitmap Decoder** - Decode bitmap-based sparse vectors
///
/// Reverses sparse_bitmap_encode to reconstruct full vector.
///
/// # Parameters
/// - `data`: Byte slice containing SparseBitmap encoded data
/// - `count`: Total number of values in original vector
///
/// # Wire Format
/// ```
/// [bitmap_size:u32][non_zero_count:u32][bitmap_bytes][non_zero_values:i64*]
/// ```
///
/// # Algorithm
/// 1. Read bitmap size and non-zero count
/// 2. Read bitmap bytes (1 bit per position)
/// 3. Read non-zero values (i64 each)
/// 4. Reconstruct vector by scanning bitmap and inserting values
///
/// # Returns
/// Vector of decoded i64 values (zeros inserted where bitmap bit is 0)
///
/// # Efficiency
/// Best for 70-95% sparse data. For >95% sparse, use SparseCOO instead.
pub fn sparse_bitmap_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;

    // Read bitmap size
    if offset + 4 > data.len() {
        return Err(anyhow::anyhow!("SparseBitmap: insufficient data for bitmap size"));
    }
    let bitmap_size = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Read non-zero count
    if offset + 4 > data.len() {
        return Err(anyhow::anyhow!("SparseBitmap: insufficient data for non-zero count"));
    }
    let non_zero_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Read bitmap
    if offset + bitmap_size > data.len() {
        return Err(anyhow::anyhow!("SparseBitmap: insufficient data for bitmap"));
    }
    let bitmap = &data[offset..offset + bitmap_size];
    offset += bitmap_size;

    // Read non-zero values
    if offset + non_zero_count * 8 > data.len() {
        return Err(anyhow::anyhow!("SparseBitmap: insufficient data for values"));
    }

    let mut non_zero_values = Vec::with_capacity(non_zero_count);
    for _ in 0..non_zero_count {
        let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;
        non_zero_values.push(value);
    }

    // Reconstruct full vector
    let mut values = vec![0i64; count];
    let mut non_zero_idx = 0;

    for i in 0..count {
        let bit_is_set = (bitmap[i / 8] & (1u8 << (i % 8))) != 0;
        if bit_is_set {
            if non_zero_idx < non_zero_values.len() {
                values[i] = non_zero_values[non_zero_idx];
                non_zero_idx += 1;
            }
        }
    }

    Ok(values)
}

/// **SparseCOO Decoder** - Decode coordinate-format sparse vectors
///
/// Reverses sparse_coo_encode to reconstruct full vector.
///
/// # Parameters
/// - `data`: Byte slice containing SparseCOO encoded data
/// - `count`: Total number of values in original vector
///
/// # Wire Format
/// ```
/// [num_entries:u32]([index:u16][value:i64])*
/// ```
///
/// # Algorithm
/// 1. Read count of (index, value) pairs
/// 2. Read each pair
/// 3. Reconstruct vector by inserting values at specified indices
///
/// # Returns
/// Vector of decoded i64 values (zeros at unspecified indices)
///
/// # Efficiency
/// Best for >95% sparse data. For 70-95% sparse, use SparseBitmap instead.
pub fn sparse_coo_decode(data: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut offset = 0;

    // Read number of non-zero entries
    if offset + 4 > data.len() {
        return Err(anyhow::anyhow!("SparseCOO: insufficient data for entry count"));
    }
    let num_entries = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
    offset += 4;

    // Read (index, value) pairs
    let mut values = vec![0i64; count];

    for _ in 0..num_entries {
        if offset + 10 > data.len() {
            return Err(anyhow::anyhow!("SparseCOO: insufficient data for entry"));
        }

        let idx = u16::from_le_bytes(data[offset..offset + 2].try_into()?) as usize;
        offset += 2;

        let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        if idx < count {
            values[idx] = value;
        }
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::ops::proximaencoder::encoding::sparse::*;

    #[test]
    fn test_sparse_bitmap_roundtrip() {
        // 80% sparse data with 32 elements (20% non-zero = ~6 non-zero values)
        let mut data = vec![0i64; 32];
        data[2] = 42;
        data[5] = 100;
        data[10] = 7;
        data[18] = 200;
        data[25] = 15;
        data[30] = 300;
        let encoded = sparse_bitmap_encode(&data).unwrap();
        let decoded = sparse_bitmap_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_sparse_coo_roundtrip() {
        // 97% sparse data with 32 elements (3% non-zero = 1 non-zero value)
        let mut data = vec![0i64; 32];
        data[16] = 42;
        let encoded = sparse_coo_encode(&data).unwrap();
        let decoded = sparse_coo_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_sparse_bitmap_all_zeros() {
        let data = vec![0i64; 100];
        let encoded = sparse_bitmap_encode(&data).unwrap();
        let decoded = sparse_bitmap_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_sparse_coo_all_zeros() {
        let data = vec![0i64; 100];
        let encoded = sparse_coo_encode(&data).unwrap();
        let decoded = sparse_coo_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_sparse_bitmap_single_value() {
        // Single non-zero value in 32 elements
        let mut data = vec![0i64; 32];
        data[15] = 42;
        let encoded = sparse_bitmap_encode(&data).unwrap();
        let decoded = sparse_bitmap_decode(&encoded, data.len()).unwrap();
        assert_eq!(decoded, data);
    }
}
