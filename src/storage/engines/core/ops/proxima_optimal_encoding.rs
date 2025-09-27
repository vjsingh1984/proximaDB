// Optimal FastLanes Encoding - Minimal redundancy
use anyhow::Result;

/// Encoding markers that indicate whether count is stored
pub mod markers {
    // High bit indicates if count follows marker
    const HAS_COUNT_FLAG: u8 = 0x80;

    // Base markers (without count flag)
    pub const DELTA: u8 = 0x20;
    pub const BITPACKED: u8 = 0x21;
    pub const RLE: u8 = 0x22;
    pub const DICTIONARY: u8 = 0x23;
    pub const RAW: u8 = 0x24;

    // With count flag (for sparse/variable data)
    pub const DELTA_WITH_COUNT: u8 = DELTA | HAS_COUNT_FLAG;        // 0xA0
    pub const BITPACKED_WITH_COUNT: u8 = BITPACKED | HAS_COUNT_FLAG; // 0xA1
    pub const RLE_WITH_COUNT: u8 = RLE | HAS_COUNT_FLAG;            // 0xA2

    #[inline]
    pub fn has_count(marker: u8) -> bool {
        (marker & HAS_COUNT_FLAG) != 0
    }

    #[inline]
    pub fn base_scheme(marker: u8) -> u8 {
        marker & !HAS_COUNT_FLAG
    }
}

impl FastLanesEncoder {
    /// Encode with smart count handling
    pub fn encode_columnar(
        &self,
        data: &[i64],
        expected_count: Option<usize> // Pass file header count
    ) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        // Determine if we need to store count
        let needs_count = match expected_count {
            Some(expected) => data.len() != expected,
            None => true, // No context, must store count
        };

        match self.scheme {
            FastLanesScheme::Delta { base } => {
                if needs_count {
                    encoded.push(markers::DELTA_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::DELTA);
                    // No count needed - decoder will use file header count
                }

                // Delta encoding data
                encoded.extend(&base.to_le_bytes());
                let deltas = compute_deltas(data, base);
                encoded.extend(encode_deltas(&deltas)?);
            },

            FastLanesScheme::RunLength => {
                // RLE often has different output count
                encoded.push(markers::RLE_WITH_COUNT);
                encoded.extend(&(data.len() as u32).to_le_bytes());

                // RLE encoding (run_count, value) pairs
                let runs = compute_runs(data);
                encoded.extend(&(runs.len() as u32).to_le_bytes()); // Number of runs
                for (count, value) in runs {
                    encoded.extend(&count.to_le_bytes());
                    encoded.extend(&value.to_le_bytes());
                }
            },
            // ... other schemes
        }

        Ok(encoded)
    }
}

impl FastLanesDecoder {
    /// Decode with smart count handling
    pub fn decode_columnar(
        &self,
        data: &[u8],
        expected_count: Option<usize> // Pass file header count
    ) -> Result<Vec<i64>> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data"));
        }

        let marker = data[0];
        let mut offset = 1;

        // Determine element count
        let element_count = if markers::has_count(marker) {
            // Count is stored in data
            let count = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
            offset += 4;
            count
        } else {
            // Use expected count from file header
            expected_count.ok_or_else(|| {
                anyhow::anyhow!("No count in data and no expected count provided")
            })?
        };

        // Decode based on scheme
        match markers::base_scheme(marker) {
            markers::DELTA => {
                let base = i64::from_le_bytes(data[offset..offset+8].try_into()?);
                offset += 8;
                let deltas = decode_deltas(&data[offset..], element_count)?;
                Ok(apply_deltas(base, deltas))
            },

            markers::RLE => {
                let run_count = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
                offset += 4;

                let mut result = Vec::with_capacity(element_count);
                for _ in 0..run_count {
                    let count = u32::from_le_bytes(data[offset..offset+4].try_into()?);
                    offset += 4;
                    let value = i64::from_le_bytes(data[offset..offset+8].try_into()?);
                    offset += 8;

                    for _ in 0..count {
                        result.push(value);
                    }
                }
                Ok(result)
            },
            // ... other schemes
            _ => Err(anyhow::anyhow!("Unknown scheme: 0x{:02x}", marker))
        }
    }
}

// Usage in FastLanesDataBlock
impl FastLanesDataBlock {
    pub fn serialize_with_context(&self, record_count: usize) -> Result<Vec<u8>> {
        // ... serialize vectors, IDs, etc.

        // For required fields - no count needed
        let encoded_ids = encoder.encode_columnar(&id_indices, Some(record_count))?;

        // For optional fields - count might be needed
        let non_null_updated_ats: Vec<i64> = /* collect non-null values */;
        let encoded_updated = if non_null_updated_ats.len() == record_count {
            encoder.encode_columnar(&non_null_updated_ats, Some(record_count))?
        } else {
            // Different count, will be stored
            encoder.encode_columnar(&non_null_updated_ats, None)?
        };

        // ...
    }

    pub fn deserialize_with_context(data: &[u8], record_count: usize) -> Result<Self> {
        // For required fields - use file header count
        let id_indices = decoder.decode_columnar(&id_data, Some(record_count))?;

        // For optional fields - count might be in data
        let updated_ats = decoder.decode_columnar(&updated_data, Some(record_count))
            .or_else(|_| decoder.decode_columnar(&updated_data, None))?;

        // ...
    }
}