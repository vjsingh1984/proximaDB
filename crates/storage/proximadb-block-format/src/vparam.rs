// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! `VectorParamBlock` — the footer-resident side region describing each vector
//! column's quantization.
//!
//! In v2, f32 vector stripes are fixed-stride: there is no per-row dimension
//! prefix. The dimension, the quantization kind, and (for SQ8) the affine
//! scale/offset live here once per column instead of being repeated per row.
//! A reader fetches this block (via the `vparam_offset`/`vparam_len` pointers in
//! the [`crate::writer::BlockFooter`]) right after the column footer, then knows
//! how to slice and decode each vector stripe — including seeking directly to an
//! arbitrary row, which is what enables row-group range reads.
//!
//! Layout (little-endian):
//! ```text
//! [0..4]  n_vec_cols  u32
//! per entry (ENTRY_SIZE = 28 bytes):
//!   [0..4]   column_id   i32
//!   [4..8]   dim         u32
//!   [8]      quant_kind  u8   (0 = raw f32, 1 = SQ8, 2 = reserved RaBitQ)
//!   [9..12]  _pad        [u8;3]
//!   [12..16] scale       f32  (SQ8 affine step; 0 for raw)
//!   [16..20] offset      f32  (SQ8 affine offset; 0 for raw)
//!   [20..24] vmin        f32  (exact column min)
//!   [24..28] vmax        f32  (exact column max)
//! ```

use anyhow::{Result, bail};
use proximadb_codec::Sq8Params;

/// Vector stripe is stored as raw fixed-stride f32 (no quantization).
pub const QUANT_RAW_F32: u8 = 0;
/// Vector stripe is SQ8-quantized (1 byte/value).
pub const QUANT_SQ8: u8 = 1;
/// Vector stripe is RaBitQ binary-quantized: per-row sign bits + corrective
/// scalars live in the stripe, with the centroid and rotation seed in the
/// [`RaBitQColumn`] trailer. The writer emits this id under the
/// `PROXIMADB_VECTOR_RABITQ_ENABLE` gate (default-OFF until recall-baked); readers key
/// off it to run the RaBitQ candidate/rerank cascade. (Formerly mislabeled
/// `QUANT_RABITQ_RESERVED` "not yet implemented" while the writer already
/// emitted it — the id and its on-wire meaning are unchanged; value stays 2.)
pub const QUANT_RABITQ: u8 = 2;
/// Vector stripe is stored as FP16 (2 bytes/value, near-lossless).
pub const QUANT_FP16: u8 = 3;

/// Exact clustered frame-of-reference/bit-pack transform over SQ8 code bytes.
pub const TRANSFORM_CLUSTERED_FOR_U8: u8 = 1;

/// Bytes per [`VectorParamEntry`] on the wire.
pub const ENTRY_SIZE: usize = 28;

/// Per-column vector quantization parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorParamEntry {
    pub column_id: i32,
    /// Fixed dimensionality of every vector in this column.
    pub dim: u32,
    /// One of `QUANT_*`.
    pub quant_kind: u8,
    /// SQ8 affine parameters + exact bounds (`scale`/`offset` are 0 for raw).
    pub params: Sq8Params,
}

impl VectorParamEntry {
    pub fn to_bytes(self) -> [u8; ENTRY_SIZE] {
        let mut b = [0u8; ENTRY_SIZE];
        b[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        b[4..8].copy_from_slice(&self.dim.to_le_bytes());
        b[8] = self.quant_kind;
        // [9..12] pad — zeros
        b[12..16].copy_from_slice(&self.params.scale.to_le_bytes());
        b[16..20].copy_from_slice(&self.params.offset.to_le_bytes());
        b[20..24].copy_from_slice(&self.params.vmin.to_le_bytes());
        b[24..28].copy_from_slice(&self.params.vmax.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < ENTRY_SIZE {
            bail!("VectorParamEntry slice too short: {}", b.len());
        }
        Ok(Self {
            column_id: i32::from_le_bytes(b[0..4].try_into()?),
            dim: u32::from_le_bytes(b[4..8].try_into()?),
            quant_kind: b[8],
            params: Sq8Params {
                scale: f32::from_le_bytes(b[12..16].try_into()?),
                offset: f32::from_le_bytes(b[16..20].try_into()?),
                vmin: f32::from_le_bytes(b[20..24].try_into()?),
                vmax: f32::from_le_bytes(b[24..28].try_into()?),
            },
        })
    }
}

/// Per-column RaBitQ side data: the centroid all vectors are centered by and the
/// `u64` seed that regenerates the orthonormal rotation. Stored in the
/// [`VectorParamBlock`] trailer for `quant_kind == QUANT_RABITQ` columns (the
/// per-row sign bits + corrective scalars live in the stripe, not here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RaBitQColumn {
    pub column_id: i32,
    pub seed: u64,
    pub centroid: Vec<f32>,
}

/// Block-local reversible transform applied after the value codec.
///
/// This optional trailer is absent from legacy `VectorParamBlock` payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorTransformColumn {
    pub column_id: i32,
    pub transform_kind: u8,
    pub transform_version: u8,
}

impl VectorTransformColumn {
    const SIZE: usize = 8;

    fn to_bytes(self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..4].copy_from_slice(&self.column_id.to_le_bytes());
        bytes[4] = self.transform_kind;
        bytes[5] = self.transform_version;
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < Self::SIZE {
            bail!("VectorTransformColumn slice too short: {}", bytes.len());
        }
        let transform = Self {
            column_id: i32::from_le_bytes(bytes[0..4].try_into()?),
            transform_kind: bytes[4],
            transform_version: bytes[5],
        };
        if transform.transform_kind != TRANSFORM_CLUSTERED_FOR_U8 {
            bail!(
                "unknown vector transform kind 0x{:02x}",
                transform.transform_kind
            );
        }
        if transform.transform_version != 1 {
            bail!(
                "unsupported vector transform version {}",
                transform.transform_version
            );
        }
        Ok(transform)
    }
}

impl RaBitQColumn {
    fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16 + self.centroid.len() * 4);
        b.extend_from_slice(&self.column_id.to_le_bytes());
        b.extend_from_slice(&self.seed.to_le_bytes());
        b.extend_from_slice(&(self.centroid.len() as u32).to_le_bytes());
        for &c in &self.centroid {
            b.extend_from_slice(&c.to_le_bytes());
        }
        b
    }

    /// Parse one entry, returning it and the number of bytes consumed.
    fn from_bytes(b: &[u8]) -> Result<(Self, usize)> {
        if b.len() < 16 {
            bail!("RaBitQColumn header too short: {}", b.len());
        }
        let column_id = i32::from_le_bytes(b[0..4].try_into()?);
        let seed = u64::from_le_bytes(b[4..12].try_into()?);
        let dim = u32::from_le_bytes(b[12..16].try_into()?) as usize;
        let need = 16 + dim * 4;
        if b.len() < need {
            bail!("RaBitQColumn truncated: have {}, need {need}", b.len());
        }
        let mut centroid = Vec::with_capacity(dim);
        for i in 0..dim {
            let off = 16 + i * 4;
            centroid.push(f32::from_le_bytes(b[off..off + 4].try_into()?));
        }
        Ok((
            Self {
                column_id,
                seed,
                centroid,
            },
            need,
        ))
    }
}

/// The full block — one entry per vector column, plus RaBitQ side data for any
/// binary-quantized columns (a trailer after the fixed entries array).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorParamBlock {
    pub entries: Vec<VectorParamEntry>,
    pub rabitq: Vec<RaBitQColumn>,
    pub transforms: Vec<VectorTransformColumn>,
}

impl VectorParamBlock {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + self.entries.len() * ENTRY_SIZE + 4);
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            buf.extend_from_slice(&e.to_bytes());
        }
        // RaBitQ trailer (always written; count 0 when none).
        buf.extend_from_slice(&(self.rabitq.len() as u32).to_le_bytes());
        for r in &self.rabitq {
            buf.extend_from_slice(&r.to_bytes());
        }
        // Optional lossless-transform trailer. Omit it when empty so default-OFF
        // writers retain the legacy VectorParamBlock bytes exactly.
        if !self.transforms.is_empty() {
            buf.extend_from_slice(&(self.transforms.len() as u32).to_le_bytes());
            for transform in &self.transforms {
                buf.extend_from_slice(&transform.to_bytes());
            }
        }
        buf
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < 4 {
            bail!("VectorParamBlock too short: {}", b.len());
        }
        let n = u32::from_le_bytes(b[0..4].try_into()?) as usize;
        let need = 4 + n * ENTRY_SIZE;
        if b.len() < need {
            bail!("VectorParamBlock truncated: have {}, need {need}", b.len());
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let off = 4 + i * ENTRY_SIZE;
            entries.push(VectorParamEntry::from_bytes(&b[off..off + ENTRY_SIZE])?);
        }
        // Parse the RaBitQ trailer if present (clean-break v2 always writes it,
        // but tolerate its absence for forward-compatible callers).
        let mut rabitq = Vec::new();
        let mut cur = need;
        if b.len() >= cur + 4 {
            let nr = u32::from_le_bytes(b[cur..cur + 4].try_into()?) as usize;
            cur += 4;
            for _ in 0..nr {
                let (col, consumed) = RaBitQColumn::from_bytes(&b[cur..])?;
                rabitq.push(col);
                cur += consumed;
            }
        }
        let mut transforms = Vec::new();
        if cur < b.len() {
            if b.len() - cur < 4 {
                bail!("VectorParamBlock truncated at transform count");
            }
            let transform_count = u32::from_le_bytes(b[cur..cur + 4].try_into()?) as usize;
            cur += 4;
            if transform_count > b.len().saturating_sub(cur) / VectorTransformColumn::SIZE {
                bail!("VectorParamBlock transform count exceeds remaining bytes");
            }
            transforms.reserve(transform_count);
            for _ in 0..transform_count {
                let end = cur + VectorTransformColumn::SIZE;
                let transform = VectorTransformColumn::from_bytes(&b[cur..end])?;
                let Some(entry) = entries
                    .iter()
                    .find(|entry| entry.column_id == transform.column_id)
                else {
                    bail!("vector transform references unknown column");
                };
                if entry.quant_kind != QUANT_SQ8 {
                    bail!("clustered u8 transform requires an SQ8 vector column");
                }
                transforms.push(transform);
                cur = end;
            }
        }
        if cur != b.len() {
            bail!("VectorParamBlock has trailing bytes");
        }
        Ok(Self {
            entries,
            rabitq,
            transforms,
        })
    }

    /// Find the entry for `column_id`, if present.
    pub fn get(&self, column_id: i32) -> Option<&VectorParamEntry> {
        self.entries.iter().find(|e| e.column_id == column_id)
    }

    /// Find the RaBitQ side data for `column_id`, if present.
    pub fn rabitq_column(&self, column_id: i32) -> Option<&RaBitQColumn> {
        self.rabitq.iter().find(|r| r.column_id == column_id)
    }

    /// Find a reversible transform declaration for `column_id`.
    pub fn transform(&self, column_id: i32) -> Option<&VectorTransformColumn> {
        self.transforms
            .iter()
            .find(|transform| transform.column_id == column_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vparam_block_round_trip() {
        let block = VectorParamBlock {
            entries: vec![
                VectorParamEntry {
                    column_id: 20,
                    dim: 384,
                    quant_kind: QUANT_SQ8,
                    params: Sq8Params {
                        scale: 0.0125,
                        offset: -1.0,
                        vmin: -1.0,
                        vmax: 2.1875,
                    },
                },
                VectorParamEntry {
                    column_id: 21,
                    dim: 64,
                    quant_kind: QUANT_RAW_F32,
                    params: Sq8Params {
                        scale: 0.0,
                        offset: 0.0,
                        vmin: -0.5,
                        vmax: 0.5,
                    },
                },
            ],
            rabitq: Vec::new(),
            transforms: vec![VectorTransformColumn {
                column_id: 20,
                transform_kind: TRANSFORM_CLUSTERED_FOR_U8,
                transform_version: 1,
            }],
        };
        let bytes = block.to_bytes();
        // Entries + empty RaBitQ count + one transform-trailer entry.
        assert_eq!(
            bytes.len(),
            4 + 2 * ENTRY_SIZE + 4 + 4 + VectorTransformColumn::SIZE
        );
        let back = VectorParamBlock::from_bytes(&bytes).unwrap();
        assert_eq!(back, block);
        assert_eq!(back.get(20).unwrap().dim, 384);
        assert_eq!(back.get(20).unwrap().quant_kind, QUANT_SQ8);
        assert_eq!(back.get(21).unwrap().quant_kind, QUANT_RAW_F32);
        assert_eq!(
            back.transform(20).map(|transform| transform.transform_kind),
            Some(TRANSFORM_CLUSTERED_FOR_U8)
        );
        assert!(back.get(99).is_none());
    }

    #[test]
    fn vparam_block_with_rabitq_trailer_round_trips() {
        let block = VectorParamBlock {
            entries: vec![VectorParamEntry {
                column_id: 20,
                dim: 4,
                quant_kind: QUANT_RABITQ,
                params: Sq8Params {
                    scale: 0.0,
                    offset: 0.0,
                    vmin: -1.0,
                    vmax: 1.0,
                },
            }],
            rabitq: vec![RaBitQColumn {
                column_id: 20,
                seed: 0xDEAD_BEEF_1234_5678,
                centroid: vec![0.1, -0.2, 0.3, -0.4],
            }],
            transforms: Vec::new(),
        };
        let back = VectorParamBlock::from_bytes(&block.to_bytes()).unwrap();
        assert_eq!(back, block);
        let rq = back.rabitq_column(20).unwrap();
        assert_eq!(rq.seed, 0xDEAD_BEEF_1234_5678);
        assert_eq!(rq.centroid, vec![0.1, -0.2, 0.3, -0.4]);
        assert!(back.rabitq_column(99).is_none());
    }

    #[test]
    fn empty_vparam_block_round_trips() {
        let block = VectorParamBlock::default();
        let bytes = block.to_bytes();
        // entries count (0) + RaBitQ trailer count (0).
        assert_eq!(bytes.len(), 8);
        assert!(VectorParamBlock::from_bytes(&bytes).unwrap().is_empty());
    }

    #[test]
    fn legacy_vparam_without_transform_trailer_remains_readable() -> Result<()> {
        let entry = VectorParamEntry {
            column_id: 20,
            dim: 4,
            quant_kind: QUANT_SQ8,
            params: Sq8Params {
                scale: 1.0,
                offset: 0.0,
                vmin: 0.0,
                vmax: 255.0,
            },
        };
        let mut legacy = Vec::new();
        legacy.extend_from_slice(&1u32.to_le_bytes());
        legacy.extend_from_slice(&entry.to_bytes());
        legacy.extend_from_slice(&0u32.to_le_bytes());

        let parsed = VectorParamBlock::from_bytes(&legacy)?;
        assert!(parsed.transforms.is_empty());
        Ok(())
    }

    #[test]
    fn unknown_vector_transform_fails_closed() {
        let block = VectorParamBlock {
            entries: Vec::new(),
            rabitq: Vec::new(),
            transforms: vec![VectorTransformColumn {
                column_id: 20,
                transform_kind: TRANSFORM_CLUSTERED_FOR_U8,
                transform_version: 1,
            }],
        };
        let mut bytes = block.to_bytes();
        let transform_kind_offset = 4 + 4 + 4 + 4;
        if let Some(kind) = bytes.get_mut(transform_kind_offset) {
            *kind = 0xfe;
        }
        assert!(VectorParamBlock::from_bytes(&bytes).is_err());
    }
}
