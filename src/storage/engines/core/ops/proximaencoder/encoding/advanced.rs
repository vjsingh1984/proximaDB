// # Advanced Encoding Algorithms
//
// State-of-the-art encoding algorithms for specialized compression scenarios.
// These are standalone functions extracted from ProximaEncoder.

use anyhow::Result;

// Import baseline functions we depend on
use super::baseline::bitpack_integers;

/// **PForDelta Encoding** - Patched Frame of Reference with Delta
///
/// Hybrid approach combining Frame of Reference (FOR) with delta encoding
/// and exception handling for outliers.
///
/// # Parameters
/// - `data`: Integer slice to encode
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [reference:i64][majority_bits:u8][num_values:u32][bitpacked_deltas]
/// [num_exceptions:u32]([position:u32][value:i64])*
/// ```
///
/// # Performance
/// - Compression: 4-8x for near-sequential data with occasional outliers
/// - Best case: 90%+ values within small delta range
pub fn pfor_delta_encode(data: &[i64], block_size: usize) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        return Ok(encoded);
    }

    // Find reference (minimum value)
    let reference = *data.iter().min().unwrap();
    encoded.extend_from_slice(&reference.to_le_bytes());

    // Compute deltas from reference
    let deltas: Vec<i64> = data.iter().map(|&v| v - reference).collect();

    // Find majority bit width (covering 90% of values)
    let mut sorted_deltas: Vec<i64> = deltas.clone();
    sorted_deltas.sort_unstable();
    let p90_index = (deltas.len() * 90) / 100;
    let p90_value = sorted_deltas.get(p90_index).copied().unwrap_or(0);

    let majority_bits = if p90_value == 0 {
        1
    } else {
        64 - (p90_value as u64).leading_zeros() as u8
    };
    encoded.push(majority_bits);

    // Separate regular values and exceptions
    let max_regular = (1i64 << majority_bits) - 1;
    let mut regular_values = Vec::with_capacity(data.len());
    let mut exceptions = Vec::new();

    for (idx, &delta) in deltas.iter().enumerate() {
        if delta <= max_regular {
            regular_values.push(delta);
        } else {
            // Store original value for exception
            exceptions.push((idx as u32, data[idx]));
            // Use max_regular as placeholder in regular stream
            regular_values.push(max_regular);
        }
    }

    // Encode regular values with bitpacking
    encoded.extend_from_slice(&(regular_values.len() as u32).to_le_bytes());
    let packed = bitpack_integers(&regular_values, majority_bits, block_size)?;
    encoded.extend(packed);

    // Encode exceptions
    encoded.extend_from_slice(&(exceptions.len() as u32).to_le_bytes());
    for (pos, value) in exceptions {
        encoded.extend_from_slice(&pos.to_le_bytes());
        encoded.extend_from_slice(&value.to_le_bytes());
    }

    Ok(encoded)
}

/// **Zigzag Encoding** - Signed integer interleaving
///
/// Maps signed integers to unsigned integers by interleaving positive
/// and negative values: 0→0, -1→1, 1→2, -2→3, 2→4, etc.
///
/// # Parameters
/// - `data`: Signed integer slice to encode
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [bits:u8][bitpacked_zigzag_values]
/// ```
///
/// # Performance
/// - Compression: 2-4x for small signed integers near zero
/// - Best case: Values in range [-128, 127] → 1-8 bits
pub fn zigzag_encode(data: &[i64], block_size: usize) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        return Ok(encoded);
    }

    // Apply zigzag transformation
    let zigzag: Vec<i64> = data.iter()
        .map(|&n| ((n << 1) ^ (n >> 63)) as i64)
        .collect();

    // Find max zigzag value to determine bit width
    let max_zigzag = zigzag.iter()
        .map(|&z| z as u64)
        .max()
        .unwrap_or(0);

    let bits = if max_zigzag == 0 {
        1
    } else {
        64 - max_zigzag.leading_zeros() as u8
    };
    encoded.push(bits);

    // Bitpack zigzag values
    let packed = bitpack_integers(&zigzag, bits, block_size)?;
    encoded.extend(packed);

    Ok(encoded)
}

/// **Simple8b Encoding** - Variable bit-width in 64-bit words
///
/// Packs multiple small integers into 64-bit words using 16 different
/// selector codes. Each word stores: [4-bit selector][60 bits of data].
///
/// # Parameters
/// - `data`: Integer slice to encode
///
/// # Wire Format
/// ```
/// [num_words:u32]([selector:u8][packed_data:u64])*
/// ```
///
/// # Performance
/// - Compression: 2-6x for uniformly small positive integers
/// - Best case: 60x compression for binary data
pub fn simple8b_encode(data: &[i64]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        encoded.extend_from_slice(&0u32.to_le_bytes());
        return Ok(encoded);
    }

    // Simple8b packing configurations: (values_per_word, bits_per_value)
    const CONFIGS: [(usize, u8); 16] = [
        (60, 1), (30, 2), (20, 3), (15, 4),
        (12, 5), (10, 6), (8, 7), (7, 8),
        (6, 10), (5, 12), (4, 15), (3, 20),
        (2, 30), (1, 60), (1, 60), (1, 60),
    ];

    let mut words = Vec::new();
    let mut i = 0;

    while i < data.len() {
        // Find best selector for current window
        let remaining = data.len() - i;
        let mut best_selector = 15; // Default to single 60-bit value
        let mut best_count = 1;

        for (selector, &(count, bits)) in CONFIGS.iter().enumerate() {
            if count > remaining {
                continue;
            }

            // Check if all values in window fit in 'bits' bits
            let window = &data[i..i + count.min(remaining)];
            let max_val = window.iter()
                .map(|&v| v as u64)
                .max()
                .unwrap_or(0);

            if max_val < (1u64 << bits) {
                best_selector = selector;
                best_count = count;
                break; // Found first valid selector (most efficient)
            }
        }

        // Pack values into 64-bit word
        let (count, bits) = CONFIGS[best_selector];
        let window = &data[i..i + best_count.min(remaining)];

        let mut word = (best_selector as u64) << 60;
        for (idx, &value) in window.iter().enumerate() {
            let shift = idx * bits as usize;
            word |= (value as u64) << shift;
        }

        words.push(word);
        i += best_count;
    }

    // Write number of words
    encoded.extend_from_slice(&(words.len() as u32).to_le_bytes());

    // Write packed words
    for word in words {
        encoded.extend_from_slice(&word.to_le_bytes());
    }

    Ok(encoded)
}

/// **VByte Encoding** - Variable-byte encoding
///
/// Each byte stores 7 bits of data plus a continuation bit.
/// Continuation bit = 1 means more bytes follow.
///
/// # Parameters
/// - `data`: Integer slice to encode
///
/// # Wire Format
/// ```
/// ([continuation_bit:1][data:7])*
/// ```
///
/// # Performance
/// - Compression: 1-8 bytes per value
/// - Best case: All values < 128 → 1 byte per value
pub fn vbyte_encode(data: &[i64]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    for &value in data {
        let mut n = value as u64;

        loop {
            let mut byte = (n & 0x7F) as u8;
            n >>= 7;

            if n != 0 {
                byte |= 0x80; // Set continuation bit
                encoded.push(byte);
            } else {
                encoded.push(byte);
                break;
            }
        }
    }

    Ok(encoded)
}

/// **DoubleDelta Encoding** - Delta of deltas
///
/// Computes deltas, then computes deltas of those deltas.
/// Optimal for time-series data with constant or linearly changing rates.
///
/// # Parameters
/// - `data`: Integer slice to encode
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [first_value:i64][first_delta:i64][ddelta_bits:u8][bitpacked_ddeltas]
/// ```
///
/// # Performance
/// - Compression: 4-10x for linear or nearly-linear sequences
/// - Best case: Constant deltas → all double deltas = 0
pub fn double_delta_encode(data: &[i64], block_size: usize) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        return Ok(encoded);
    }

    if data.len() == 1 {
        encoded.extend_from_slice(&data[0].to_le_bytes());
        return Ok(encoded);
    }

    // Store first value
    encoded.extend_from_slice(&data[0].to_le_bytes());

    // Compute first delta
    let first_delta = data[1] - data[0];
    encoded.extend_from_slice(&first_delta.to_le_bytes());

    if data.len() == 2 {
        return Ok(encoded);
    }

    // Compute double deltas
    let mut prev_delta = first_delta;
    let mut double_deltas = Vec::with_capacity(data.len() - 2);

    for i in 2..data.len() {
        let delta = data[i] - data[i - 1];
        let ddelta = delta - prev_delta;
        double_deltas.push(ddelta);
        prev_delta = delta;
    }

    // Find bit width for double deltas (may be negative)
    let max_abs = double_deltas.iter()
        .map(|&d| d.unsigned_abs())
        .max()
        .unwrap_or(0);

    let bits = if max_abs == 0 {
        1
    } else {
        64 - max_abs.leading_zeros() as u8
    };
    encoded.push(bits);

    // Bitpack double deltas
    let packed = bitpack_integers(&double_deltas, bits, block_size)?;
    encoded.extend(packed);

    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pfor_delta_encode() {
        let data = vec![100, 101, 102, 103, 150]; // One outlier
        let encoded = pfor_delta_encode(&data, 128).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_zigzag_encode() {
        let data = vec![-2, -1, 0, 1, 2];
        let encoded = zigzag_encode(&data, 128).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_simple8b_encode() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let encoded = simple8b_encode(&data).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_vbyte_encode() {
        let data = vec![1, 127, 128, 16383, 16384];
        let encoded = vbyte_encode(&data).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_double_delta_encode() {
        let data = vec![0, 1, 2, 3, 4]; // Constant deltas
        let encoded = double_delta_encode(&data, 128).unwrap();
        assert!(!encoded.is_empty());
    }
}
