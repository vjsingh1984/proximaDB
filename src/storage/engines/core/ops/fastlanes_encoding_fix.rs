// Optimized FastLanes Encoding Fix
// This demonstrates the recommended fix for the element count issue

use anyhow::Result;

/// Fixed encoder that stores element count
pub fn encode_i64_fixed(encoder: &FastLanesEncoder, data: &[i64]) -> Result<Vec<u8>> {
    let mut result = Vec::new();

    // 1. Type marker for i64
    result.push(0x82);

    // 2. CRITICAL: Store the element count (4 bytes for up to 4B elements)
    result.extend_from_slice(&(data.len() as u32).to_le_bytes());

    // 3. Encode using the chosen scheme
    let encoded = encoder.encode_integers(data)?;
    result.extend(encoded);

    Ok(result)
}

/// Fixed decoder that reads element count
pub fn decode_i64_fixed(decoder: &FastLanesDecoder, data: &[u8]) -> Result<Vec<i64>> {
    if data.len() < 5 {
        return Err(anyhow::anyhow!("Invalid i64 data: too short"));
    }

    // 1. Check type marker
    if data[0] != 0x82 {
        return Err(anyhow::anyhow!("Invalid i64 marker"));
    }

    // 2. CRITICAL: Read the element count
    let count = u32::from_le_bytes(data[1..5].try_into()?) as usize;

    // 3. Decode exactly 'count' elements
    decoder.decode_integers(&data[5..], count)
}

/// Alternative: Use length-prefixed encoding at the scheme level
pub fn delta_encode_with_count(data: &[i64], base: i64) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    // Store element count first
    encoded.extend_from_slice(&(data.len() as u32).to_le_bytes());

    // Store base value
    encoded.extend_from_slice(&base.to_le_bytes());

    // Compute and store deltas
    let deltas: Vec<i64> = data.iter()
        .map(|&v| v.wrapping_sub(base))
        .collect();

    // Determine optimal bit width
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
    // ... bitpacking logic

    Ok(encoded)
}

pub fn delta_decode_with_count(data: &[u8]) -> Result<Vec<i64>> {
    if data.len() < 13 { // 4 (count) + 8 (base) + 1 (bits)
        return Err(anyhow::anyhow!("Invalid delta-encoded data"));
    }

    // Read element count
    let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;

    // Read base value
    let base = i64::from_le_bytes(data[4..12].try_into()?);

    // Read bit width
    let bits = data[12];

    // Decode exactly 'count' deltas
    let deltas = unpack_integers(&data[13..], count, bits)?;

    // Apply deltas
    let values: Vec<i64> = deltas.iter()
        .map(|&delta| base.wrapping_add(delta))
        .collect();

    Ok(values)
}

// For the FastLanesDataBlock use case
impl FastLanesDataBlock {
    pub fn serialize_column_with_count(&self, column_data: &[i64]) -> Result<Vec<u8>> {
        let mut result = Vec::new();

        // Always prefix with element count
        result.extend_from_slice(&(column_data.len() as u32).to_le_bytes());

        // Then encode the data
        let encoder = FastLanesEncoder::new(self.select_optimal_scheme(column_data));
        let encoded = encoder.encode_integers(column_data)?;
        result.extend(encoded);

        Ok(result)
    }

    pub fn deserialize_column_with_count(data: &[u8]) -> Result<Vec<i64>> {
        if data.len() < 4 {
            return Err(anyhow::anyhow!("Missing element count"));
        }

        // Read element count
        let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;

        // Decode exactly that many elements
        let decoder = FastLanesDecoder::new_from_data(&data[4..]);
        decoder.decode_integers(&data[4..], count)
    }
}