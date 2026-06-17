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
/// Reserved for a future binary RaBitQ scheme (not yet implemented).
pub const QUANT_RABITQ_RESERVED: u8 = 2;

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
        Ok(Self { entries, rabitq })
    }

    /// Find the entry for `column_id`, if present.
    pub fn get(&self, column_id: i32) -> Option<&VectorParamEntry> {
        self.entries.iter().find(|e| e.column_id == column_id)
    }

    /// Find the RaBitQ side data for `column_id`, if present.
    pub fn rabitq_column(&self, column_id: i32) -> Option<&RaBitQColumn> {
        self.rabitq.iter().find(|r| r.column_id == column_id)
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
        };
        let bytes = block.to_bytes();
        // entries + the (empty) RaBitQ trailer count.
        assert_eq!(bytes.len(), 4 + 2 * ENTRY_SIZE + 4);
        let back = VectorParamBlock::from_bytes(&bytes).unwrap();
        assert_eq!(back, block);
        assert_eq!(back.get(20).unwrap().dim, 384);
        assert_eq!(back.get(20).unwrap().quant_kind, QUANT_SQ8);
        assert_eq!(back.get(21).unwrap().quant_kind, QUANT_RAW_F32);
        assert!(back.get(99).is_none());
    }

    #[test]
    fn vparam_block_with_rabitq_trailer_round_trips() {
        let block = VectorParamBlock {
            entries: vec![VectorParamEntry {
                column_id: 20,
                dim: 4,
                quant_kind: QUANT_RABITQ_RESERVED,
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
}
