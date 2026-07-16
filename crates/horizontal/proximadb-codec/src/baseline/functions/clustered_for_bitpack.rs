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

#[derive(Debug, Clone, Copy)]
struct ChunkDirectoryEntry {
    row_start: usize,
    row_count: usize,
    kind: u8,
    payload_start: usize,
    payload_len: usize,
}

struct ParsedClusteredU8 {
    dimension: usize,
    row_count: usize,
    chunks: Vec<ChunkDirectoryEntry>,
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
    let parsed = parse_clustered_u8(data)?;
    let output_len = parsed
        .row_count
        .checked_mul(parsed.dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 output length overflow"))?;
    let mut output = vec![0u8; output_len];
    for chunk in &parsed.chunks {
        let decoded = decode_chunk(data, &parsed, chunk)?;
        let output_start = chunk
            .row_start
            .checked_mul(parsed.dimension)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 output start overflow"))?;
        let output_end = output_start
            .checked_add(decoded.len())
            .ok_or_else(|| anyhow::anyhow!("clustered u8 output end overflow"))?;
        output[output_start..output_end].copy_from_slice(&decoded);
    }
    Ok(output)
}

/// Decode only requested rows, preserving `row_indices` order and duplicates.
///
/// The directory is fully validated, but only microchunks containing requested
/// rows run their inverse transform. This bounds rerank CPU by the survivor
/// chunks rather than by the complete fetched stripe.
pub fn decode_u8_rows_selected(data: &[u8], row_indices: &[usize]) -> Result<Vec<Vec<u8>>> {
    let parsed = parse_clustered_u8(data)?;
    for &row in row_indices {
        if row >= parsed.row_count {
            bail!("clustered u8 selected row exceeds declared row count");
        }
    }
    let mut output = vec![Vec::new(); row_indices.len()];
    for chunk in &parsed.chunks {
        let chunk_end = chunk
            .row_start
            .checked_add(chunk.row_count)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 chunk row overflow"))?;
        if !row_indices
            .iter()
            .any(|&row| row >= chunk.row_start && row < chunk_end)
        {
            continue;
        }
        let decoded = decode_chunk(data, &parsed, chunk)?;
        for (output_index, &row) in row_indices.iter().enumerate() {
            if row < chunk.row_start || row >= chunk_end {
                continue;
            }
            let local_row = row - chunk.row_start;
            let start = local_row
                .checked_mul(parsed.dimension)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 selected row offset overflow"))?;
            let end = start
                .checked_add(parsed.dimension)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 selected row end overflow"))?;
            output[output_index].extend_from_slice(&decoded[start..end]);
        }
    }
    if output.iter().any(|row| row.len() != parsed.dimension) {
        bail!("clustered u8 selected rows were not covered by directory");
    }
    Ok(output)
}

fn parse_clustered_u8(data: &[u8]) -> Result<ParsedClusteredU8> {
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

    let mut expected_row = 0usize;
    let mut expected_payload_offset = 0usize;
    let mut chunks = Vec::with_capacity(chunk_count);

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
        if !matches!(kind, CHUNK_RAW | CHUNK_FOR_BITPACK) {
            bail!("clustered u8 unknown chunk kind {kind}");
        }
        chunks.push(ChunkDirectoryEntry {
            row_start,
            row_count: chunk_rows,
            kind,
            payload_start: absolute_payload,
            payload_len,
        });

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
    Ok(ParsedClusteredU8 {
        dimension,
        row_count,
        chunks,
    })
}

fn decode_chunk(
    data: &[u8],
    parsed: &ParsedClusteredU8,
    chunk: &ChunkDirectoryEntry,
) -> Result<Vec<u8>> {
    let payload_end = chunk
        .payload_start
        .checked_add(chunk.payload_len)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 chunk payload end overflow"))?;
    let payload = &data[chunk.payload_start..payload_end];
    match chunk.kind {
        CHUNK_RAW => decode_raw_chunk(payload, parsed.dimension, chunk.row_count),
        CHUNK_FOR_BITPACK => decode_for_chunk(payload, parsed.dimension, chunk.row_count),
        other => bail!("clustered u8 unknown chunk kind {other}"),
    }
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
    const LANE_HEADER_LEN: usize = 6;
    const EXCEPTION_LEN: usize = 5;
    let mut output = Vec::new();

    for dimension_index in 0..dimension {
        let mut minimum = u8::MAX;
        for row_index in 0..row_count {
            let value = raw[row_index * dimension + dimension_index];
            minimum = minimum.min(value);
        }
        let deltas: Vec<u32> = (0..row_count)
            .map(|row_index| u32::from(raw[row_index * dimension + dimension_index] - minimum))
            .collect();

        // PFOR-style selection: one outlier must not force every value in the
        // lane to width 8. Choose the exact width whose packed bytes plus sparse
        // `(row, delta)` exceptions are smallest.
        let mut best_width = 8u8;
        let mut best_size = LANE_HEADER_LEN
            .checked_add(row_count)
            .ok_or_else(|| anyhow::anyhow!("clustered u8 lane size overflow"))?;
        for width in 0u8..=8 {
            let threshold = if width == 8 { 256 } else { 1u32 << width };
            let exception_count = deltas.iter().filter(|&&delta| delta >= threshold).count();
            let packed_len = row_count
                .checked_mul(width as usize)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 packed length overflow"))?
                .div_ceil(8);
            let candidate_size = LANE_HEADER_LEN
                .checked_add(packed_len)
                .and_then(|size| {
                    exception_count
                        .checked_mul(EXCEPTION_LEN)
                        .and_then(|exceptions| size.checked_add(exceptions))
                })
                .ok_or_else(|| anyhow::anyhow!("clustered u8 exception size overflow"))?;
            if candidate_size < best_size {
                best_size = candidate_size;
                best_width = width;
            }
        }

        let threshold = if best_width == 8 {
            256
        } else {
            1u32 << best_width
        };
        let mut packed_deltas = Vec::with_capacity(deltas.len());
        let mut exceptions = Vec::new();
        for (row_index, &delta) in deltas.iter().enumerate() {
            if delta >= threshold {
                packed_deltas.push(0);
                exceptions.push((row_index, delta));
            } else {
                packed_deltas.push(delta);
            }
        }
        let exception_count = u32::try_from(exceptions.len())
            .map_err(|_| anyhow::anyhow!("clustered u8 exception count exceeds u32"))?;
        output.push(minimum);
        output.push(best_width);
        output.extend_from_slice(&exception_count.to_le_bytes());
        output.extend_from_slice(&bitpack_u32(&packed_deltas, best_width)?);
        for (row_index, delta) in exceptions {
            let row_index = u32::try_from(row_index)
                .map_err(|_| anyhow::anyhow!("clustered u8 exception row exceeds u32"))?;
            let delta = u8::try_from(delta)
                .map_err(|_| anyhow::anyhow!("clustered u8 exception delta exceeds u8"))?;
            output.extend_from_slice(&row_index.to_le_bytes());
            output.push(delta);
        }
    }
    Ok(output)
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
    let output_len = row_count
        .checked_mul(dimension)
        .ok_or_else(|| anyhow::anyhow!("clustered u8 transformed output overflow"))?;
    let mut output = vec![0u8; output_len];
    let mut lane_pos = 0usize;

    for dimension_index in 0..dimension {
        let minimum = read_u8(payload, &mut lane_pos)?;
        let width = read_u8(payload, &mut lane_pos)?;
        let exception_count = read_u32(payload, &mut lane_pos)? as usize;
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
        if exception_count > payload.len().saturating_sub(lane_pos) / 5 {
            bail!("clustered u8 exception count exceeds lane payload");
        }
        let mut exception_rows = vec![false; row_count];
        let threshold = if width == 8 { 256 } else { 1u32 << width };
        for _ in 0..exception_count {
            let exception_row = read_u32(payload, &mut lane_pos)? as usize;
            let delta = read_u8(payload, &mut lane_pos)?;
            if exception_row >= row_count {
                bail!("clustered u8 exception row exceeds chunk");
            }
            if exception_rows[exception_row] {
                bail!("clustered u8 duplicate exception row");
            }
            if u32::from(delta) < threshold {
                bail!("clustered u8 exception fits declared lane width");
            }
            exception_rows[exception_row] = true;
            let value = minimum
                .checked_add(delta)
                .ok_or_else(|| anyhow::anyhow!("clustered u8 exception value overflow"))?;
            output[exception_row * dimension + dimension_index] = value;
        }
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

    use super::{
        ClusterRun, ClusteredU8Config, decode_u8_rows, decode_u8_rows_selected, encode_u8_rows,
    };

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
    fn clustered_u8_sparse_outliers_do_not_force_eight_bit_lanes() -> Result<()> {
        let dimension = 16usize;
        let row_count = 128usize;
        let mut rows = Vec::with_capacity(dimension * row_count);
        for row in 0..row_count {
            for lane in 0..dimension {
                let value = if row == lane {
                    255
                } else {
                    100 + ((row + lane) % 4) as u8
                };
                rows.push(value);
            }
        }
        let encoded = encode_u8_rows(
            &rows,
            dimension,
            &[ClusterRun::new(0, row_count)],
            ClusteredU8Config {
                max_rows_per_chunk: row_count,
                min_savings_bytes: 0,
            },
        )?;

        assert!(encoded.profile().encoded_bytes < rows.len() / 2);
        assert_eq!(decode_u8_rows(encoded.as_bytes())?, rows);
        Ok(())
    }

    #[test]
    fn clustered_u8_selected_decode_preserves_order_and_duplicates() -> Result<()> {
        let dimension = 8usize;
        let row_count = 24usize;
        let rows: Vec<u8> = (0..row_count)
            .flat_map(|row| (0..dimension).map(move |lane| (40 + row + lane) as u8))
            .collect();
        let encoded = encode_u8_rows(
            &rows,
            dimension,
            &[ClusterRun::new(0, 12), ClusterRun::new(12, 12)],
            ClusteredU8Config {
                max_rows_per_chunk: 6,
                min_savings_bytes: 0,
            },
        )?;
        let selected = [19usize, 2, 19, 7];
        let decoded = decode_u8_rows_selected(encoded.as_bytes(), &selected)?;
        let expected: Vec<Vec<u8>> = selected
            .iter()
            .map(|&row| rows[row * dimension..(row + 1) * dimension].to_vec())
            .collect();
        assert_eq!(decoded, expected);
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
