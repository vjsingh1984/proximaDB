// Clean FastLanes Encoding Design
// One marker per method, self-describing format

use anyhow::Result;

// Single unified marker system
pub mod markers {
    // Data type markers (first byte tells us everything)
    pub const I64_DELTA: u8 = 0x20;
    pub const I64_BITPACKED: u8 = 0x21;
    pub const I64_FRAME_OF_REF: u8 = 0x22;
    pub const I64_RUN_LENGTH: u8 = 0x23;
    pub const I64_DICTIONARY: u8 = 0x24;
    pub const I64_RAW: u8 = 0x25;

    pub const F32_DELTA: u8 = 0x30;
    pub const F32_BITPACKED: u8 = 0x31;
    pub const F32_QUANTIZED: u8 = 0x32;

    pub const F64_DELTA: u8 = 0x40;
    pub const F64_BITPACKED: u8 = 0x41;

    // Specialized types
    pub const BINARY_VECTOR: u8 = 0x50;
    pub const PQ4_CODES: u8 = 0x51;
    pub const PQ8_CODES: u8 = 0x52;
}

// Clean encoder - ONE method for each data type
impl FastLanesEncoder {
    /// Encode any i64 data - marker tells decoder everything
    pub fn encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        match self.scheme {
            FastLanesScheme::Delta { base } => {
                encoded.push(markers::I64_DELTA);
                // Store element count for proper decoding
                encoded.extend(&(data.len() as u32).to_le_bytes());
                encoded.extend(&base.to_le_bytes());
                // ... delta encoding logic
            },
            FastLanesScheme::BitPacked { bits } => {
                encoded.push(markers::I64_BITPACKED);
                encoded.extend(&(data.len() as u32).to_le_bytes());
                encoded.push(bits);
                // ... bitpacking logic
            },
            // ... other schemes
        }

        Ok(encoded)
    }

    // No need for separate encode_i64, encode_integers, etc!
    // The scheme determines the encoding, not the method name
}

// Clean decoder - ONE method that reads marker and decodes appropriately
impl FastLanesDecoder {
    /// Decode any FastLanes data - marker tells us how
    pub fn decode(&self, data: &[u8]) -> Result<DecodedData> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data"));
        }

        let marker = data[0];
        let data = &data[1..];

        match marker {
            markers::I64_DELTA => {
                // Read count
                let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;
                let base = i64::from_le_bytes(data[4..12].try_into()?);
                let deltas = self.decode_deltas(&data[12..], count)?;
                Ok(DecodedData::I64(apply_deltas(base, deltas)))
            },
            markers::I64_BITPACKED => {
                let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;
                let bits = data[4];
                let values = self.unpack_bits(&data[5..], count, bits)?;
                Ok(DecodedData::I64(values))
            },
            markers::F32_DELTA => {
                // Similar but converts back to f32
                let count = u32::from_le_bytes(data[0..4].try_into()?) as usize;
                // ... decode and convert
                Ok(DecodedData::F32(values))
            },
            // ... handle all markers
            _ => Err(anyhow::anyhow!("Unknown marker: 0x{:02x}", marker))
        }
    }
}

pub enum DecodedData {
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Binary(Vec<u8>),
    // ... other types
}

// For the block structures, it becomes super clean:
impl FastLanesDataBlock {
    pub fn encode_column(&self, data: &[i64]) -> Result<Vec<u8>> {
        let encoder = FastLanesEncoder::new(self.select_optimal_scheme(data));
        encoder.encode(data) // That's it! Marker included, self-describing
    }

    pub fn decode_column(&self, data: &[u8]) -> Result<Vec<i64>> {
        let decoder = FastLanesDecoder::new();
        match decoder.decode(data)? {
            DecodedData::I64(values) => Ok(values),
            _ => Err(anyhow::anyhow!("Expected i64 data"))
        }
    }
}