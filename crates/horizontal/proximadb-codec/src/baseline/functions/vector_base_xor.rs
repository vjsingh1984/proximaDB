// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Exact base-XOR codec for co-located f32 vector rows.
//!
//! This is a pilot codec for PCX-005. It stores the first vector as an exact
//! base, XORs every subsequent f32 bit pattern against that base dimension, and
//! run-codes zero and literal XOR runs. It is intentionally not assigned a
//! durable `ProximaScheme` marker yet; PAX keeps raw f32 vectors until cataloged
//! profiling proves this candidate is beneficial for a layout family.

use anyhow::{Result, bail};

const MAGIC: &[u8; 4] = b"VBX1";
const HEADER_LEN: usize = 12;
const TAG_ZERO_RUN: u8 = 0;
const TAG_LITERAL_RUN: u8 = 1;
const MAX_LITERAL_RUN_WORDS: usize = 4096;

/// Compression profile produced while evaluating the base-XOR candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorBaseXorProfile {
    /// Number of vector rows encoded.
    pub rows: usize,
    /// Fixed vector dimension.
    pub dimension: usize,
    /// Raw f32 payload bytes, excluding row/null headers used by container formats.
    pub raw_bytes: usize,
    /// Encoded payload bytes for this codec.
    pub encoded_bytes: usize,
    /// Number of zero XOR words after comparing rows to the base vector.
    pub zero_words: usize,
    /// Number of non-zero XOR words.
    pub literal_words: usize,
    /// Number of zero runs emitted.
    pub zero_runs: usize,
    /// Number of literal runs emitted.
    pub literal_runs: usize,
    /// `raw_bytes / encoded_bytes`; greater than 1.0 means the codec saved space.
    pub compression_ratio: f64,
    /// Encoded bytes per f32 value.
    pub bytes_per_value: f64,
}

impl VectorBaseXorProfile {
    /// Returns true when the candidate is smaller than the raw f32 payload.
    pub fn saves_space(&self) -> bool {
        self.raw_bytes > 0 && self.encoded_bytes < self.raw_bytes
    }

    /// Conservative selection predicate for profiling and future strategy hooks.
    pub fn should_use(&self, min_compression_ratio: f64) -> bool {
        self.saves_space() && self.compression_ratio >= min_compression_ratio
    }
}

#[derive(Debug, Default)]
struct RunStats {
    zero_words: usize,
    literal_words: usize,
    zero_runs: usize,
    literal_runs: usize,
}

/// Encode fixed-dimension f32 vectors with the base-XOR pilot codec.
pub fn encode_f32_vectors(rows: &[&[f32]]) -> Result<Vec<u8>> {
    encode_f32_vectors_with_profile(rows).map(|(encoded, _)| encoded)
}

/// Encode fixed-dimension f32 vectors and return the candidate profile.
pub fn encode_f32_vectors_with_profile(rows: &[&[f32]]) -> Result<(Vec<u8>, VectorBaseXorProfile)> {
    let dimension = validate_rows(rows)?;
    let row_count = rows.len();
    let raw_bytes = raw_payload_bytes(row_count, dimension)?;

    let row_count_u32 = u32::try_from(row_count)
        .map_err(|_| anyhow::anyhow!("vector base-XOR row count exceeds u32"))?;
    let dimension_u32 = u32::try_from(dimension)
        .map_err(|_| anyhow::anyhow!("vector base-XOR dimension exceeds u32"))?;

    let base_bytes = checked_bytes(dimension)?;
    let mut encoded = Vec::with_capacity(HEADER_LEN.saturating_add(base_bytes));
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(&row_count_u32.to_le_bytes());
    encoded.extend_from_slice(&dimension_u32.to_le_bytes());

    if row_count == 0 {
        let profile = make_profile(
            row_count,
            dimension,
            raw_bytes,
            encoded.len(),
            RunStats::default(),
        );
        return Ok((encoded, profile));
    }

    for value in rows[0] {
        encoded.extend_from_slice(&value.to_bits().to_le_bytes());
    }

    let stats = encode_xor_runs(rows, dimension, &mut encoded)?;
    let profile = make_profile(row_count, dimension, raw_bytes, encoded.len(), stats);
    Ok((encoded, profile))
}

/// Profile the base-XOR candidate without exposing the encoded payload.
pub fn profile_f32_vectors(rows: &[&[f32]]) -> Result<VectorBaseXorProfile> {
    encode_f32_vectors_with_profile(rows).map(|(_, profile)| profile)
}

/// Decode a payload produced by [`encode_f32_vectors`].
pub fn decode_f32_vectors(data: &[u8]) -> Result<Vec<Vec<f32>>> {
    if data.len() < HEADER_LEN {
        bail!("vector base-XOR payload shorter than header");
    }
    if &data[..4] != MAGIC {
        bail!("vector base-XOR payload has invalid magic");
    }

    let mut pos = 4;
    let row_count = read_u32(data, &mut pos)? as usize;
    let dimension = read_u32(data, &mut pos)? as usize;

    if row_count == 0 {
        if dimension != 0 {
            bail!("vector base-XOR empty payload declares non-zero dimension");
        }
        if pos != data.len() {
            bail!("vector base-XOR empty payload has trailing bytes");
        }
        return Ok(Vec::new());
    }

    if dimension == 0 {
        if pos != data.len() {
            bail!("vector base-XOR zero-dimension payload has trailing bytes");
        }
        return Ok(vec![Vec::new(); row_count]);
    }

    let base_bytes = checked_bytes(dimension)?;
    ensure_remaining(
        data,
        pos,
        base_bytes,
        "vector base-XOR payload ended inside base vector",
    )?;

    let mut base_bits = Vec::with_capacity(dimension);
    for _ in 0..dimension {
        base_bits.push(read_u32(data, &mut pos)?);
    }

    let total_words = checked_words(row_count, dimension)?;
    let expected_xor_words = checked_words(row_count - 1, dimension)?;
    let mut flat_bits = Vec::with_capacity(total_words);
    flat_bits.extend_from_slice(&base_bits);

    let mut decoded_xor_words = 0usize;
    while decoded_xor_words < expected_xor_words {
        if pos >= data.len() {
            bail!("vector base-XOR payload ended inside XOR stream");
        }
        let tag = data[pos];
        pos += 1;
        let run_len = read_u32(data, &mut pos)? as usize;
        if run_len == 0 {
            bail!("vector base-XOR run length must be non-zero");
        }
        let next_decoded = decoded_xor_words
            .checked_add(run_len)
            .ok_or_else(|| anyhow::anyhow!("vector base-XOR decoded word count overflow"))?;
        if next_decoded > expected_xor_words {
            bail!("vector base-XOR run exceeds expected word count");
        }

        match tag {
            TAG_ZERO_RUN => {
                for _ in 0..run_len {
                    let base_idx = decoded_xor_words % dimension;
                    flat_bits.push(base_bits[base_idx]);
                    decoded_xor_words += 1;
                }
            }
            TAG_LITERAL_RUN => {
                let literal_bytes = checked_bytes(run_len)?;
                ensure_remaining(
                    data,
                    pos,
                    literal_bytes,
                    "vector base-XOR payload ended inside literal run",
                )?;
                for _ in 0..run_len {
                    let xor = read_u32(data, &mut pos)?;
                    let base_idx = decoded_xor_words % dimension;
                    flat_bits.push(base_bits[base_idx] ^ xor);
                    decoded_xor_words += 1;
                }
            }
            other => bail!("vector base-XOR unknown run tag: {other}"),
        }
    }

    if pos != data.len() {
        bail!("vector base-XOR payload has trailing bytes");
    }

    let mut rows = Vec::with_capacity(row_count);
    for chunk in flat_bits.chunks_exact(dimension) {
        rows.push(chunk.iter().map(|bits| f32::from_bits(*bits)).collect());
    }
    Ok(rows)
}

fn validate_rows(rows: &[&[f32]]) -> Result<usize> {
    let Some(first) = rows.first() else {
        return Ok(0);
    };
    let dimension = first.len();
    for (idx, row) in rows.iter().enumerate().skip(1) {
        if row.len() != dimension {
            bail!(
                "vector base-XOR requires fixed dimension: row 0 has {}, row {idx} has {}",
                dimension,
                row.len()
            );
        }
    }
    Ok(dimension)
}

fn encode_xor_runs(rows: &[&[f32]], dimension: usize, encoded: &mut Vec<u8>) -> Result<RunStats> {
    let mut stats = RunStats::default();
    if rows.len() <= 1 || dimension == 0 {
        return Ok(stats);
    }

    let base = rows[0];
    let mut zero_run = 0u32;
    let mut literals = Vec::new();

    for row in rows.iter().skip(1) {
        for (dim_idx, value) in row.iter().enumerate() {
            let xor = value.to_bits() ^ base[dim_idx].to_bits();
            if xor == 0 {
                flush_literal_run(encoded, &mut literals, &mut stats)?;
                zero_run = zero_run
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("vector base-XOR zero run exceeds u32"))?;
                stats.zero_words += 1;
                if zero_run == u32::MAX {
                    flush_zero_run(encoded, &mut zero_run, &mut stats);
                }
            } else {
                flush_zero_run(encoded, &mut zero_run, &mut stats);
                literals.push(xor);
                stats.literal_words += 1;
                if literals.len() == MAX_LITERAL_RUN_WORDS {
                    flush_literal_run(encoded, &mut literals, &mut stats)?;
                }
            }
        }
    }

    flush_zero_run(encoded, &mut zero_run, &mut stats);
    flush_literal_run(encoded, &mut literals, &mut stats)?;
    Ok(stats)
}

fn flush_zero_run(encoded: &mut Vec<u8>, len: &mut u32, stats: &mut RunStats) {
    if *len == 0 {
        return;
    }
    encoded.push(TAG_ZERO_RUN);
    encoded.extend_from_slice(&len.to_le_bytes());
    stats.zero_runs += 1;
    *len = 0;
}

fn flush_literal_run(
    encoded: &mut Vec<u8>,
    literals: &mut Vec<u32>,
    stats: &mut RunStats,
) -> Result<()> {
    if literals.is_empty() {
        return Ok(());
    }
    let len = u32::try_from(literals.len())
        .map_err(|_| anyhow::anyhow!("vector base-XOR literal run exceeds u32"))?;
    encoded.push(TAG_LITERAL_RUN);
    encoded.extend_from_slice(&len.to_le_bytes());
    for word in literals.drain(..) {
        encoded.extend_from_slice(&word.to_le_bytes());
    }
    stats.literal_runs += 1;
    Ok(())
}

fn make_profile(
    rows: usize,
    dimension: usize,
    raw_bytes: usize,
    encoded_bytes: usize,
    stats: RunStats,
) -> VectorBaseXorProfile {
    let value_count = rows.saturating_mul(dimension);
    VectorBaseXorProfile {
        rows,
        dimension,
        raw_bytes,
        encoded_bytes,
        zero_words: stats.zero_words,
        literal_words: stats.literal_words,
        zero_runs: stats.zero_runs,
        literal_runs: stats.literal_runs,
        compression_ratio: if raw_bytes == 0 || encoded_bytes == 0 {
            0.0
        } else {
            raw_bytes as f64 / encoded_bytes as f64
        },
        bytes_per_value: if value_count == 0 {
            0.0
        } else {
            encoded_bytes as f64 / value_count as f64
        },
    }
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32> {
    ensure_remaining(data, *pos, 4, "vector base-XOR payload ended before u32")?;
    let value = u32::from_le_bytes(data[*pos..*pos + 4].try_into()?);
    *pos += 4;
    Ok(value)
}

fn ensure_remaining(data: &[u8], pos: usize, len: usize, message: &'static str) -> Result<()> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("vector base-XOR offset overflow"))?;
    if end > data.len() {
        bail!(message);
    }
    Ok(())
}

fn raw_payload_bytes(rows: usize, dimension: usize) -> Result<usize> {
    checked_words(rows, dimension).and_then(checked_bytes)
}

fn checked_words(rows: usize, dimension: usize) -> Result<usize> {
    rows.checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("vector base-XOR word count overflow"))
}

fn checked_bytes(words: usize) -> Result<usize> {
    words
        .checked_mul(4)
        .ok_or_else(|| anyhow::anyhow!("vector base-XOR byte count overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_base_xor_preserves_exact_f32_bits() {
        let rows = vec![
            vec![
                f32::from_bits(0x0000_0000),
                f32::from_bits(0x8000_0000),
                f32::from_bits(0x7fc0_0123),
                f32::INFINITY,
            ],
            vec![
                f32::from_bits(0x0000_0001),
                f32::from_bits(0x8000_0000),
                f32::from_bits(0x7fc0_0123),
                f32::NEG_INFINITY,
            ],
            vec![
                f32::from_bits(0x0000_0000),
                f32::from_bits(0x8000_0001),
                f32::from_bits(0x7fa0_0001),
                42.25,
            ],
        ];
        let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();

        let encoded = encode_f32_vectors(&refs).unwrap();
        let decoded = decode_f32_vectors(&encoded).unwrap();

        assert_bitwise_eq(&rows, &decoded);
    }

    #[test]
    fn vector_base_xor_compresses_co_located_repeated_rows() {
        let base: Vec<f32> = (0..64).map(|idx| idx as f32 / 64.0).collect();
        let rows = vec![base; 128];
        let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();

        let profile = profile_f32_vectors(&refs).unwrap();

        assert!(profile.saves_space());
        assert!(profile.should_use(1.10));
        assert_eq!(profile.zero_words, 127 * 64);
        assert_eq!(profile.literal_words, 0);
    }

    #[test]
    fn vector_base_xor_profile_rejects_high_entropy_rows() {
        let rows = high_entropy_rows(64, 32);
        let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();

        let profile = profile_f32_vectors(&refs).unwrap();

        assert!(!profile.saves_space());
        assert!(!profile.should_use(1.01));
        assert!(profile.literal_words > profile.zero_words);
    }

    #[test]
    fn vector_base_xor_profile_selects_sparse_drift_rows() {
        let rows = sparse_drift_rows(128, 64);
        let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();

        let profile = profile_f32_vectors(&refs).unwrap();

        assert!(profile.should_use(1.10), "{profile:?}");
        assert!(profile.zero_words > profile.literal_words);
    }

    #[test]
    fn vector_base_xor_rejects_mixed_dimensions() {
        let first = vec![1.0, 2.0, 3.0];
        let second = vec![1.0, 2.0];
        let err = encode_f32_vectors(&[first.as_slice(), second.as_slice()]).unwrap_err();

        assert!(err.to_string().contains("requires fixed dimension"));
    }

    #[test]
    fn vector_base_xor_roundtrips_empty_input() {
        let encoded = encode_f32_vectors(&[]).unwrap();
        let decoded = decode_f32_vectors(&encoded).unwrap();

        assert!(decoded.is_empty());
    }

    fn high_entropy_rows(rows: usize, dimension: usize) -> Vec<Vec<f32>> {
        let mut state = 0x1234_5678u32;
        (0..rows)
            .map(|_| {
                (0..dimension)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        f32::from_bits(state)
                    })
                    .collect()
            })
            .collect()
    }

    fn sparse_drift_rows(rows: usize, dimension: usize) -> Vec<Vec<f32>> {
        let base: Vec<f32> = (0..dimension)
            .map(|idx| idx as f32 / dimension as f32)
            .collect();
        let mut vectors = Vec::with_capacity(rows);
        vectors.push(base.clone());
        for row_idx in 1..rows {
            let mut row = base.clone();
            for lane in 0..4 {
                let dim_idx = (row_idx * 31 + lane * 17) % dimension;
                row[dim_idx] = f32::from_bits(row[dim_idx].to_bits() ^ ((lane as u32 + 1) << 8));
            }
            vectors.push(row);
        }
        vectors
    }

    fn assert_bitwise_eq(expected: &[Vec<f32>], actual: &[Vec<f32>]) {
        assert_eq!(expected.len(), actual.len());
        for (expected_row, actual_row) in expected.iter().zip(actual) {
            assert_eq!(expected_row.len(), actual_row.len());
            for (expected, actual) in expected_row.iter().zip(actual_row) {
                assert_eq!(expected.to_bits(), actual.to_bits());
            }
        }
    }
}
