// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Exact clustered frame-of-reference transform for fixed-dimension u8 rows.

use anyhow::{Result, bail};

use super::bitpack::{bitpack_u32, unbitpack_u32};

const MAGIC: &[u8; 4] = b"CFU8";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 17;
const DIRECTORY_ENTRY_LEN: usize = 17;
const CHUNK_RAW: u8 = 0;
const CHUNK_FOR_BITPACK: u8 = 1;

/// One contiguous cluster run in row order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterRun {
    /// First row in the run.
    pub start_row: usize,
    /// Number of rows in the run.
    pub row_count: usize,
}

impl ClusterRun {
    /// Construct a contiguous cluster run.
    pub const fn new(start_row: usize, row_count: usize) -> Self {
        Self {
            start_row,
            row_count,
        }
    }
}

/// Selection and decode-amplification bounds for clustered u8 encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusteredU8Config {
    /// Maximum independently encoded rows per microchunk.
    pub max_rows_per_chunk: usize,
    /// Required byte saving before selecting FOR/bit-pack over raw bytes.
    pub min_savings_bytes: usize,
}

impl Default for ClusteredU8Config {
    fn default() -> Self {
        Self {
            max_rows_per_chunk: 256,
            min_savings_bytes: 8,
        }
    }
}

/// Realized-size accounting for one encoded u8 matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusteredU8Profile {
    /// Row count.
    pub rows: usize,
    /// Fixed row dimension.
    pub dimension: usize,
    /// Input payload bytes.
    pub raw_bytes: usize,
    /// Complete encoded bytes, including header and directory.
    pub encoded_bytes: usize,
    /// Number of independently decodable microchunks.
    pub chunk_count: usize,
    /// Number of chunks using FOR/bit-pack.
    pub transformed_chunks: usize,
    /// Number of chunks stored raw.
    pub raw_chunks: usize,
}

/// Serialized clustered-u8 payload plus realized-size profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedClusteredU8 {
    bytes: Vec<u8>,
    profile: ClusteredU8Profile,
}

impl EncodedClusteredU8 {
    /// Borrow the complete serialized payload.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the result and return the serialized payload.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Return realized byte and chunk accounting.
    pub const fn profile(&self) -> &ClusteredU8Profile {
        &self.profile
    }
}

#[derive(Debug)]
struct EncodedChunk {
    row_start: usize,
    row_count: usize,
    kind: u8,
    payload: Vec<u8>,
}

/// Encode row-major fixed-dimension u8 data using cluster-local FOR/bit-pack.
///
/// The transform is exact. Each cluster run is split into independently
/// described microchunks, and each microchunk falls back to raw row-major bytes
/// unless the transformed payload clears the configured saving threshold.
pub fn encode_u8_rows(
    rows: &[u8],
    dimension: usize,
    runs: &[ClusterRun],
    config: ClusteredU8Config,
) -> Result<EncodedClusteredU8> {
    if dimension == 0 {
        bail!("clustered u8 dimension must be non-zero");
    }
    if config.max_rows_per_chunk == 0 {
        bail!("clustered u8 max_rows_per_chunk must be non-zero");
    }
    if !rows.len().is_multiple_of(dimension) {
        bail!("clustered u8 payload length is not divisible by dimension");
    }

    let row_count = rows.len() / dimension;
    validate_runs(runs, row_count)?;

    let mut chunks = Vec::new();
    for run in runs {
        let run_end = run
            .start_row
            .checked_add(run.row_count)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 run end overflow"))?;
        let mut chunk_start = run.start_row;
        while chunk_start < run_end {
            let chunk_rows = (run_end - chunk_start).min(config.max_rows_per_chunk);
            let raw = row_slice(rows, dimension, chunk_start, chunk_rows)?.to_vec();
            let transformed = encode_for_chunk(&raw, dimension, chunk_rows)?;
            let transformed_with_savings = transformed
                .len()
                .checked_add(config.min_savings_bytes)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 saving threshold overflow"))?;
            let (kind, payload) = if transformed_with_savings <= raw.len() {
                (CHUNK_FOR_BITPACK, transformed)
            } else {
                (CHUNK_RAW, raw)
            };
            chunks.push(EncodedChunk {
                row_start: chunk_start,
                row_count: chunk_rows,
                kind,
                payload,
            });
            chunk_start = chunk_start
                .checked_add(chunk_rows)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 chunk start overflow"))?;
        }
    }

    serialize_chunks(rows.len(), dimension, row_count, chunks)
}

/// Decode a complete payload produced by [`encode_u8_rows`].
pub fn decode_u8_rows(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < HEADER_LEN {
        bail!("clustered u8 payload shorter than header");
    }
    if &data[..4] != MAGIC {
        bail!("clustered u8 payload has invalid magic");
    }
    if data[4] != VERSION {
        bail!("clustered u8 payload has unsupported version {}", data[4]);
    }

    let mut pos = 5;
    let dimension = read_u32(data, &mut pos)? as usize;
    let row_count = read_u32(data, &mut pos)? as usize;
    let chunk_count = read_u32(data, &mut pos)? as usize;
    if dimension == 0 {
        bail!("clustered u8 payload declares zero dimension");
    }

    let directory_bytes = chunk_count
        .checked_mul(DIRECTORY_ENTRY_LEN)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 directory length overflow"))?;
    let payload_start = HEADER_LEN
        .checked_add(directory_bytes)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 payload offset overflow"))?;
    ensure_remaining(
        data,
        HEADER_LEN,
        directory_bytes,
        "clustered u8 truncated directory",
    )?;

    let output_len = row_count
        .checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 output length overflow"))?;
    let mut output = vec![0u8; output_len];
    let mut expected_row = 0usize;
    let mut expected_payload_offset = 0usize;

    for _ in 0..chunk_count {
        let row_start = read_u32(data, &mut pos)? as usize;
        let chunk_rows = read_u32(data, &mut pos)? as usize;
        let kind = read_u8(data, &mut pos)?;
        let payload_offset = read_u32(data, &mut pos)? as usize;
        let payload_len = read_u32(data, &mut pos)? as usize;

        if row_start != expected_row || chunk_rows == 0 {
            bail!("clustered u8 directory has non-contiguous or empty chunk");
        }
        if payload_offset != expected_payload_offset {
            bail!("clustered u8 directory has non-contiguous payload offsets");
        }
        let chunk_end = row_start
            .checked_add(chunk_rows)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 chunk row overflow"))?;
        if chunk_end > row_count {
            bail!("clustered u8 chunk exceeds declared row count");
        }
        let absolute_payload = payload_start
            .checked_add(payload_offset)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 absolute payload offset overflow"))?;
        ensure_remaining(
            data,
            absolute_payload,
            payload_len,
            "clustered u8 chunk payload is truncated",
        )?;
        let payload = &data[absolute_payload..absolute_payload + payload_len];
        let decoded = match kind {
            CHUNK_RAW => decode_raw_chunk(payload, dimension, chunk_rows)?,
            CHUNK_FOR_BITPACK => decode_for_chunk(payload, dimension, chunk_rows)?,
            other => bail!("clustered u8 unknown chunk kind {other}"),
        };
        let output_start = row_start
            .checked_mul(dimension)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 output start overflow"))?;
        let output_end = chunk_end
            .checked_mul(dimension)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 output end overflow"))?;
        output[output_start..output_end].copy_from_slice(&decoded);

        expected_row = chunk_end;
        expected_payload_offset = expected_payload_offset
            .checked_add(payload_len)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 payload length overflow"))?;
    }

    if expected_row != row_count {
        bail!("clustered u8 directory does not cover every row");
    }
    let expected_end = payload_start
        .checked_add(expected_payload_offset)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 final payload offset overflow"))?;
    if expected_end != data.len() {
        bail!("clustered u8 payload has trailing bytes");
    }
    Ok(output)
}

fn validate_runs(runs: &[ClusterRun], row_count: usize) -> Result<()> {
    if row_count == 0 {
        if runs.is_empty() {
            return Ok(());
        }
        bail!("clustered u8 empty input must not declare runs");
    }
    let mut expected_start = 0usize;
    for run in runs {
        if run.row_count == 0 || run.start_row != expected_start {
            bail!("clustered u8 runs must be non-empty and exactly contiguous");
        }
        expected_start = expected_start
            .checked_add(run.row_count)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 run coverage overflow"))?;
        if expected_start > row_count {
            bail!("clustered u8 runs exceed row count");
        }
    }
    if expected_start != row_count {
        bail!("clustered u8 runs do not cover every row");
    }
    Ok(())
}

fn row_slice(rows: &[u8], dimension: usize, row_start: usize, row_count: usize) -> Result<&[u8]> {
    let start = row_start
        .checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 row offset overflow"))?;
    let len = row_count
        .checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 row length overflow"))?;
    ensure_remaining(rows, start, len, "clustered u8 row slice exceeds input")?;
    Ok(&rows[start..start + len])
}

fn encode_for_chunk(raw: &[u8], dimension: usize, row_count: usize) -> Result<Vec<u8>> {
    let metadata_len = dimension
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 metadata length overflow"))?;
    let mut metadata = Vec::with_capacity(metadata_len);
    let mut lanes = Vec::new();

    for dimension_index in 0..dimension {
        let mut minimum = u8::MAX;
        let mut maximum = u8::MIN;
        for row_index in 0..row_count {
            let value = raw[row_index * dimension + dimension_index];
            minimum = minimum.min(value);
            maximum = maximum.max(value);
        }
        let max_delta = maximum - minimum;
        let width = if max_delta == 0 {
            0
        } else {
            (u8::BITS - max_delta.leading_zeros()) as u8
        };
        metadata.push(minimum);
        metadata.push(width);

        let deltas: Vec<u32> = (0..row_count)
            .map(|row_index| u32::from(raw[row_index * dimension + dimension_index] - minimum))
            .collect();
        lanes.extend_from_slice(&bitpack_u32(&deltas, width)?);
    }
    metadata.extend_from_slice(&lanes);
    Ok(metadata)
}

fn decode_raw_chunk(payload: &[u8], dimension: usize, row_count: usize) -> Result<Vec<u8>> {
    let expected = row_count
        .checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 raw chunk length overflow"))?;
    if payload.len() != expected {
        bail!("clustered u8 raw chunk has invalid length");
    }
    Ok(payload.to_vec())
}

fn decode_for_chunk(payload: &[u8], dimension: usize, row_count: usize) -> Result<Vec<u8>> {
    let metadata_len = dimension
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 metadata length overflow"))?;
    ensure_remaining(
        payload,
        0,
        metadata_len,
        "clustered u8 transformed chunk lacks lane metadata",
    )?;
    let output_len = row_count
        .checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 transformed output overflow"))?;
    let mut output = vec![0u8; output_len];
    let mut lane_pos = metadata_len;

    for dimension_index in 0..dimension {
        let minimum = payload[dimension_index * 2];
        let width = payload[dimension_index * 2 + 1];
        if width > 8 {
            bail!("clustered u8 lane bit width exceeds 8");
        }
        let lane_bits = row_count
            .checked_mul(width as usize)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 lane bit length overflow"))?;
        let lane_len = lane_bits.div_ceil(8);
        ensure_remaining(
            payload,
            lane_pos,
            lane_len,
            "clustered u8 transformed lane is truncated",
        )?;
        let deltas = unbitpack_u32(&payload[lane_pos..lane_pos + lane_len], width, row_count)?;
        for (row_index, delta) in deltas.into_iter().enumerate() {
            let value = u32::from(minimum)
                .checked_add(delta)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 decoded value overflow"))?;
            let value = u8::try_from(value)
                .map_err(|_| anyhow::anyhow!("clustered u8 decoded value exceeds u8"))?;
            output[row_index * dimension + dimension_index] = value;
        }
        lane_pos = lane_pos
            .checked_add(lane_len)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 lane offset overflow"))?;
    }
    if lane_pos != payload.len() {
        bail!("clustered u8 transformed chunk has trailing bytes");
    }
    Ok(output)
}

fn serialize_chunks(
    raw_bytes: usize,
    dimension: usize,
    row_count: usize,
    chunks: Vec<EncodedChunk>,
) -> Result<EncodedClusteredU8> {
    let chunk_count = chunks.len();
    let directory_bytes = chunk_count
        .checked_mul(DIRECTORY_ENTRY_LEN)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 directory length overflow"))?;
    let payload_bytes = chunks.iter().try_fold(0usize, |total, chunk| {
        total
            .checked_add(chunk.payload.len())
            .ok_or_else(|| anyhow::anyhow!("clustered u8 payload length overflow"))
    })?;
    let encoded_bytes = HEADER_LEN
        .checked_add(directory_bytes)
        .and_then(|value| value.checked_add(payload_bytes))
        .ok_or_else(|| anyhow::anyhow!("clustered u8 encoded length overflow"))?;

    let dimension_u32 = u32::try_from(dimension)
        .map_err(|_| anyhow::anyhow!("clustered u8 dimension exceeds u32"))?;
    let row_count_u32 = u32::try_from(row_count)
        .map_err(|_| anyhow::anyhow!("clustered u8 row count exceeds u32"))?;
    let chunk_count_u32 = u32::try_from(chunk_count)
        .map_err(|_| anyhow::anyhow!("clustered u8 chunk count exceeds u32"))?;

    let mut bytes = Vec::with_capacity(encoded_bytes);
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    bytes.extend_from_slice(&dimension_u32.to_le_bytes());
    bytes.extend_from_slice(&row_count_u32.to_le_bytes());
    bytes.extend_from_slice(&chunk_count_u32.to_le_bytes());

    let mut payload_offset = 0usize;
    for chunk in &chunks {
        let row_start = u32::try_from(chunk.row_start)
            .map_err(|_| anyhow::anyhow!("clustered u8 chunk row start exceeds u32"))?;
        let chunk_rows = u32::try_from(chunk.row_count)
            .map_err(|_| anyhow::anyhow!("clustered u8 chunk row count exceeds u32"))?;
        let offset = u32::try_from(payload_offset)
            .map_err(|_| anyhow::anyhow!("clustered u8 payload offset exceeds u32"))?;
        let len = u32::try_from(chunk.payload.len())
            .map_err(|_| anyhow::anyhow!("clustered u8 chunk payload exceeds u32"))?;
        bytes.extend_from_slice(&row_start.to_le_bytes());
        bytes.extend_from_slice(&chunk_rows.to_le_bytes());
        bytes.push(chunk.kind);
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&len.to_le_bytes());
        payload_offset = payload_offset
            .checked_add(chunk.payload.len())
            .ok_or_else(|| anyhow::anyhow!("clustered u8 payload offset overflow"))?;
    }
    for chunk in &chunks {
        bytes.extend_from_slice(&chunk.payload);
    }

    let transformed_chunks = chunks
        .iter()
        .filter(|chunk| chunk.kind == CHUNK_FOR_BITPACK)
        .count();
    Ok(EncodedClusteredU8 {
        bytes,
        profile: ClusteredU8Profile {
            rows: row_count,
            dimension,
            raw_bytes,
            encoded_bytes,
            chunk_count,
            transformed_chunks,
            raw_chunks: chunk_count - transformed_chunks,
        },
    })
}

fn ensure_remaining(data: &[u8], start: usize, len: usize, message: &str) -> Result<()> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("{message}: length overflow"))?;
    if end > data.len() {
        bail!("{message}");
    }
    Ok(())
}

fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8> {
    ensure_remaining(data, *pos, 1, "clustered u8 payload ended while reading u8")?;
    let value = data[*pos];
    *pos += 1;
    Ok(value)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32> {
    ensure_remaining(
        data,
        *pos,
        4,
        "clustered u8 payload ended while reading u32",
    )?;
    let end = *pos + 4;
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&data[*pos..end]);
    *pos = end;
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::{ClusterRun, ClusteredU8Config, decode_u8_rows, encode_u8_rows};

    #[test]
    fn clustered_u8_round_trip_is_byte_exact() -> Result<()> {
        let rows = vec![
            100, 101, 102, 103, // cluster 0
            101, 100, 103, 102, // cluster 0
            99, 102, 101, 104, // cluster 0
            200, 202, 201, 203, // cluster 1
            201, 203, 200, 202, // cluster 1
            202, 201, 203, 200, // cluster 1
        ];
        let runs = [ClusterRun::new(0, 3), ClusterRun::new(3, 3)];
        let encoded = encode_u8_rows(
            &rows,
            4,
            &runs,
            ClusteredU8Config {
                max_rows_per_chunk: 2,
                min_savings_bytes: 0,
            },
        )?;

        assert_eq!(encoded.profile().rows, 6);
        assert_eq!(encoded.profile().dimension, 4);
        assert_eq!(encoded.profile().chunk_count, 4);
        assert_eq!(decode_u8_rows(encoded.as_bytes())?, rows);
        Ok(())
    }

    #[test]
    fn clustered_u8_handles_zero_and_eight_bit_widths() -> Result<()> {
        let rows = vec![
            7, 0, 255, // width 0, 0, 0 for first row
            7, 1, 0, // width 0, 1, 8 across rows
            7, 0, 128,
        ];
        let encoded = encode_u8_rows(
            &rows,
            3,
            &[ClusterRun::new(0, 3)],
            ClusteredU8Config {
                max_rows_per_chunk: 3,
                min_savings_bytes: 0,
            },
        )?;

        assert_eq!(decode_u8_rows(encoded.as_bytes())?, rows);
        Ok(())
    }

    #[test]
    fn clustered_u8_selects_transform_for_tight_long_runs() -> Result<()> {
        let dimension = 16;
        let row_count = 128;
        let mut rows = Vec::with_capacity(dimension * row_count);
        for row in 0..row_count {
            for lane in 0..dimension {
                rows.push(96 + ((row + lane) % 4) as u8);
            }
        }
        let encoded = encode_u8_rows(
            &rows,
            dimension,
            &[ClusterRun::new(0, row_count)],
            ClusteredU8Config {
                max_rows_per_chunk: row_count,
                min_savings_bytes: 8,
            },
        )?;

        assert_eq!(encoded.profile().transformed_chunks, 1);
        assert_eq!(encoded.profile().raw_chunks, 0);
        assert!(encoded.profile().encoded_bytes < encoded.profile().raw_bytes);
        assert_eq!(decode_u8_rows(encoded.as_bytes())?, rows);
        Ok(())
    }

    #[test]
    fn clustered_u8_every_byte_value_round_trips() -> Result<()> {
        let rows: Vec<u8> = (u8::MIN..=u8::MAX).collect();
        let encoded = encode_u8_rows(
            &rows,
            8,
            &[ClusterRun::new(0, rows.len() / 8)],
            ClusteredU8Config {
                max_rows_per_chunk: 7,
                min_savings_bytes: 0,
            },
        )?;

        assert_eq!(decode_u8_rows(encoded.as_bytes())?, rows);
        Ok(())
    }

    #[test]
    fn clustered_u8_rejects_incomplete_or_overlapping_runs() {
        let rows = vec![1, 2, 3, 4, 5, 6];
        let config = ClusteredU8Config::default();

        assert!(encode_u8_rows(&rows, 2, &[ClusterRun::new(0, 2)], config).is_err());
        assert!(
            encode_u8_rows(
                &rows,
                2,
                &[ClusterRun::new(0, 2), ClusterRun::new(1, 2)],
                config,
            )
            .is_err()
        );
    }

    #[test]
    fn clustered_u8_corruption_fails_closed() -> Result<()> {
        let rows = vec![10, 11, 12, 13, 14, 15];
        let encoded = encode_u8_rows(
            &rows,
            2,
            &[ClusterRun::new(0, 3)],
            ClusteredU8Config::default(),
        )?;
        let mut corrupt = encoded.into_bytes();
        if let Some(version) = corrupt.get_mut(4) {
            *version = version.saturating_add(1);
        }

        assert!(decode_u8_rows(&corrupt).is_err());
        Ok(())
    }
}
