// Optimal Proxima Encoding - Minimal redundancy
use anyhow::Result;

/// Encoding markers for Proxima format
pub mod markers {
    // Count mode constants (second byte)
    pub const COUNT_MODE_NONE: u8 = 0x00;
    pub const COUNT_MODE_U8: u8 = 0x01;
    pub const COUNT_MODE_U16: u8 = 0x02;
    pub const COUNT_MODE_U32: u8 = 0x03;

    // Scheme markers (first byte)
    #[allow(dead_code)]
    pub const DELTA: u8 = 0x20;
    #[allow(dead_code)]
    pub const BITPACKED: u8 = 0x10;
    pub const RLE: u8 = 0x60;
    #[allow(dead_code)]
    pub const DICTIONARY: u8 = 0x50;
    #[allow(dead_code)]
    pub const RAW: u8 = 0x00;
}

impl ProximaEncoder {
    /// Encode with smart count handling using Proxima format
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

        // Helper to write Proxima format header
        let write_header = |buf: &mut Vec<u8>, scheme: u8, count: usize, store_count: bool| {
            buf.push(scheme); // First byte: scheme
            if store_count {
                match count {
                    0..=255 => {
                        buf.push(markers::COUNT_MODE_U8);
                        buf.push(count as u8);
                    },
                    256..=65535 => {
                        buf.push(markers::COUNT_MODE_U16);
                        buf.extend(&(count as u16).to_le_bytes());
                    },
                    _ => {
                        buf.push(markers::COUNT_MODE_U32);
                        buf.extend(&(count as u32).to_le_bytes());
                    }
                }
            } else {
                buf.push(markers::COUNT_MODE_NONE);
            }
        };

        match self.scheme {
            ProximaScheme::Delta { base } => {
                write_header(&mut encoded, markers::DELTA, data.len(), needs_count);

                // Delta encoding data
                encoded.extend(&base.to_le_bytes());
                let deltas = compute_deltas(data, base);
                encoded.extend(encode_deltas(&deltas)?);
            },

            ProximaScheme::RunLength => {
                write_header(&mut encoded, markers::RLE, data.len(), true);

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

impl ProximaDecoder {
    /// Decode with smart count handling using Proxima format
    pub fn decode_columnar(
        &self,
        data: &[u8],
        expected_count: Option<usize> // Pass file header count
    ) -> Result<Vec<i64>> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data"));
        }

        if data.len() < 2 {
            return Err(anyhow::anyhow!("Insufficient data for Proxima format"));
        }

        let scheme_marker = data[0];
        let count_mode = data[1];
        let mut offset = 2;

        // Extract count based on count mode
        let element_count = match count_mode {
            markers::COUNT_MODE_NONE => {
                expected_count.ok_or_else(|| anyhow::anyhow!("No count in data and no expected count provided"))?
            },
            markers::COUNT_MODE_U8 => {
                let c = data[offset] as usize;
                offset += 1;
                c
            },
            markers::COUNT_MODE_U16 => {
                let c = u16::from_le_bytes(data[offset..offset+2].try_into()?) as usize;
                offset += 2;
                c
            },
            markers::COUNT_MODE_U32 => {
                let c = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
                offset += 4;
                c
            },
            _ => {
                return Err(anyhow::anyhow!("Invalid count mode: 0x{:02x}", count_mode));
            }
        };

        // Decode based on scheme
        match scheme_marker {
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
            _ => Err(anyhow::anyhow!("Unknown scheme: 0x{:02x}", scheme_marker))
        }
    }
}

// Usage in ProximaDataBlock
impl ProximaDataBlock {
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