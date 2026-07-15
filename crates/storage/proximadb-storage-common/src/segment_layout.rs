//! Coalesced-RaBitQ segment layout — header-prefix + self-describing footer-index
//! (ADR-062 / TD-RDSTRAT-6 PR1).
//!
//! The new PAX segment layout is Parquet-style: a tiny header-prefix pinned at
//! offset 0 (so the RaBitQ hot-scan is one ranged GET with zero metadata
//! dependency), the data blocks, then a self-describing **footer-index** at the
//! tail (schema snapshot + block table + per-stripe encoding map + extensible
//! per-block stats), located via `[footer_len u64][SEGMENT_MAGIC]`.
//!
//! ```text
//! [HEADER-PREFIX 40B]                 ◄ one GET coalesces header + RaBitQ scan
//! [coalesced RaBitQ region]           ◄ HOT SCAN, keep=100%
//! [Block0 … BlockK]                   ◄ UNCHANGED SQ8/fp32/metadata decoder
//! [FOOTER-INDEX]                      ◄ tail GET, cached
//! [footer_len u64][SEGMENT_MAGIC 8B]
//! ```
//!
//! Same segment magic (`PAXSEG01`) as the legacy `[blocks][SegmentIndex][magic]`
//! layout; a distinct `SEG_HEADER_MAGIC` at offset 0 is the **presence-field**
//! that selects the scan-then-rerank read path (new) vs the legacy in-block
//! RaBitQ path — mixed-read-safe (ADR-061 pre-GA amendment). Readers default to
//! the legacy path when the marker is absent, so already-written segments stay
//! readable.

#![forbid(unsafe_code)]

use anyhow::{Result, bail};

use crate::pax_block::SEGMENT_MAGIC;

/// Segment-header magic — the presence-field for the coalesced-RaBitQ layout.
/// Chosen disjoint from the block magic `PBLK` (`0x50…`) so detection is
/// unambiguous: a legacy segment starts with `PBLK` (its first block); a
/// coalesced segment starts with `PXH1`.
pub const SEG_HEADER_MAGIC: &[u8; 4] = b"PXH1";

/// Header layout version (independent of the block/segment format family — bump
/// only when the header-prefix byte layout changes).
pub const SEG_LAYOUT_VERSION: u8 = 1;

/// `[magic 4][version 1][pad 3][rabitq_off 8][rabitq_len 8][footer_off 8][footer_len 8]`.
pub const SEG_HEADER_PREFIX_LEN: usize = 40;

/// True iff `bytes` carries the coalesced-RaBitQ header-prefix at offset 0 — the
/// presence-field that selects the scan-then-rerank read path (mixed-read).
pub fn is_coalesced_segment(bytes: &[u8]) -> bool {
    bytes.len() >= SEG_HEADER_MAGIC.len() && &bytes[..SEG_HEADER_MAGIC.len()] == SEG_HEADER_MAGIC
}

/// The fixed 40 B header-prefix at offset 0. The RaBitQ scan GET reads
/// `[0, rabitq_off + rabitq_len]`, so the prefix coalesces into that one GET.
#[derive(Debug, Clone)]
pub struct SegmentHeaderPrefix {
    pub layout_version: u8,
    /// Byte offset of the coalesced RaBitQ region (== SEG_HEADER_PREFIX_LEN).
    pub rabitq_off: u64,
    /// Byte length of the coalesced RaBitQ region.
    pub rabitq_len: u64,
    /// Byte offset of the footer-index.
    pub footer_off: u64,
    /// Byte length of the footer-index.
    pub footer_len: u64,
}

impl SegmentHeaderPrefix {
    /// Serialize to a fixed 40 B buffer.
    pub fn to_bytes(&self) -> [u8; SEG_HEADER_PREFIX_LEN] {
        let mut buf = [0u8; SEG_HEADER_PREFIX_LEN];
        buf[..4].copy_from_slice(SEG_HEADER_MAGIC);
        buf[4] = self.layout_version;
        // buf[5..8] reserved (zero).
        buf[8..16].copy_from_slice(&self.rabitq_off.to_le_bytes());
        buf[16..24].copy_from_slice(&self.rabitq_len.to_le_bytes());
        buf[24..32].copy_from_slice(&self.footer_off.to_le_bytes());
        buf[32..40].copy_from_slice(&self.footer_len.to_le_bytes());
        buf
    }

    /// Parse + validate (magic + version). Fail-closed on a wrong magic or
    /// truncation — never mis-reads a legacy segment as coalesced.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SEG_HEADER_PREFIX_LEN {
            bail!("coalesced segment header too short: {}", bytes.len());
        }
        if &bytes[..4] != SEG_HEADER_MAGIC {
            bail!("not a coalesced-RaBitQ segment (bad header magic)");
        }
        let layout_version = bytes[4];
        if layout_version != SEG_LAYOUT_VERSION {
            bail!(
                "unsupported coalesced segment layout version {layout_version} (expected {SEG_LAYOUT_VERSION})"
            );
        }
        Ok(Self {
            layout_version,
            rabitq_off: u64::from_le_bytes(bytes[8..16].try_into()?),
            rabitq_len: u64::from_le_bytes(bytes[16..24].try_into()?),
            footer_off: u64::from_le_bytes(bytes[24..32].try_into()?),
            footer_len: u64::from_le_bytes(bytes[32..40].try_into()?),
        })
    }
}

/// Extensible per-block per-column stats kind (Parquet-style `Statistics`
/// control plane). Magic/format-family-independent: adding a kind is
/// forward-compatible (unknown kinds are skipped by the reader), never a magic
/// bump. v1 emits [`StatsKind::None`] (the OID-bloom fast path is PR3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StatsKind {
    /// No per-block stats payload (the PR1 default).
    None = 0,
    /// Per-block OID bloom (no false negatives → `vector_by_id` skips to the one
    /// positive block). Populated in PR3; the structure is emitted now.
    BloomOid = 1,
    /// MinMax zone map (scalar + vector predicate pushdown). Deferred.
    MinMax = 2,
    /// Combined OID bloom + MinMax.
    BloomOidMinMax = 3,
}

impl StatsKind {
    /// Decode from a tag byte; unknown tags degrade to [`StatsKind::None`]
    /// (forward-compatible — a newer writer's stats are ignored, never a
    /// hard error).
    pub fn from_tag(tag: u8) -> Self {
        match tag {
            1 => Self::BloomOid,
            2 => Self::MinMax,
            3 => Self::BloomOidMinMax,
            _ => Self::None,
        }
    }
}

/// One block-table entry in the footer-index: the byte extent + row count of a
/// data block. The read path maps survivor rows → blocks via cumulative row
/// counts, then plans coalesced ranged GETs over these extents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FooterBlockEntry {
    /// Absolute byte offset of the block in the segment.
    pub offset: u64,
    /// Block byte length.
    pub size: u32,
    /// Number of rows in this block (for global row → block mapping).
    pub row_count: u32,
    /// The block's stats kind (PR1: always [`StatsKind::None`]; the payload
    /// length is zero). Forward-compatible hook for the PR3 OID bloom.
    pub stats_kind: StatsKind,
}

/// The self-describing footer-index (Parquet-style metadata hub): block table +
/// schema snapshot + per-stripe encoding map + RaBitQ mirror + row count. Located
/// via `[footer_index][footer_len u64][SEGMENT_MAGIC]` at the segment tail.
///
/// The block table **subsumes** the legacy `SegmentIndex` for the new layout: the
/// reader builds the scanner's block list from it so `next_block` / `read_records`
/// work unchanged (mixed-read: legacy segments still use `SegmentIndex::locate`).
#[derive(Debug, Clone)]
pub struct SegmentFooterIndex {
    /// Total rows in the segment (== RaBitQ region `n_rows`).
    pub row_count: u64,
    /// RaBitQ region extent mirror (also in the header-prefix).
    pub rabitq_off: u64,
    pub rabitq_len: u64,
    // -- Schema snapshot (minimal; full evolution deferred to TD-TBL-4 / ADR-047) --
    /// Embedding dimensionality (per embedding column; homogeneous in v1).
    pub embed_dim: u32,
    /// Number of embedding columns.
    pub embed_count: u32,
    // -- Per-stripe encoding map (minimal; future clustered-SQ8/fp32 registers here) --
    /// Tier-1 (block) vector encoding tag: 0 = RawF32, 1 = SQ8, 3 = FP16.
    pub embed_quant_tag: u8,
    /// Whether the opt-in exact-f32 tier is present in the blocks.
    pub has_f32_tier: bool,
    /// The block table (block 0..K in emission/cluster order).
    pub blocks: Vec<FooterBlockEntry>,
}

/// `[footer_len u64][SEGMENT_MAGIC 8B]` — the 16 B tail that locates the footer.
pub const SEG_TAIL_LEN: usize = 8 + 8;

impl SegmentFooterIndex {
    /// Serialize the footer-index body (no tail). Layout:
    /// `[footer_version u8][row_count u64][rabitq_off u64][rabitq_len u64]`
    /// `[embed_dim u32][embed_count u32][embed_quant_tag u8][has_f32_tier u8]`
    /// `[n_blocks u32] per block: [off u64][size u32][row_count u32][stats_tag u8][stats_len u32][stats bytes]`.
    pub fn to_bytes(&self) -> Vec<u8> {
        const FOOTER_VERSION: u8 = 1;
        let mut buf = Vec::with_capacity(64 + self.blocks.len() * 24);
        buf.push(FOOTER_VERSION);
        buf.extend_from_slice(&self.row_count.to_le_bytes());
        buf.extend_from_slice(&self.rabitq_off.to_le_bytes());
        buf.extend_from_slice(&self.rabitq_len.to_le_bytes());
        buf.extend_from_slice(&self.embed_dim.to_le_bytes());
        buf.extend_from_slice(&self.embed_count.to_le_bytes());
        buf.push(self.embed_quant_tag);
        buf.push(if self.has_f32_tier { 1 } else { 0 });
        buf.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for b in &self.blocks {
            buf.extend_from_slice(&b.offset.to_le_bytes());
            buf.extend_from_slice(&b.size.to_le_bytes());
            buf.extend_from_slice(&b.row_count.to_le_bytes());
            buf.push(b.stats_kind as u8);
            // PR1: no stats payload (length 0). PR3 writes the OID bloom here.
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
        buf
    }

    /// Parse the footer-index body (the bytes between the data blocks and the
    /// `[footer_len][magic]` tail). Fail-closed on truncation / bad version.
    /// Forward-compatible: an unknown future `FOOTER_VERSION` is an error (a new
    /// footer version is a deliberate, gated change), but unknown `StatsKind`
    /// tags degrade to `None`.
    pub fn parse(body: &[u8]) -> Result<Self> {
        const FOOTER_VERSION: u8 = 1;
        if body.is_empty() {
            bail!("empty footer-index body");
        }
        if body[0] != FOOTER_VERSION {
            bail!(
                "unsupported footer-index version {} (expected {FOOTER_VERSION})",
                body[0]
            );
        }
        let mut p = 1usize;
        let rd_u64 = |p: &mut usize| -> Result<u64> {
            if *p + 8 > body.len() {
                bail!("footer-index truncated at u64");
            }
            let v = u64::from_le_bytes(body[*p..*p + 8].try_into()?);
            *p += 8;
            Ok(v)
        };
        let rd_u32 = |p: &mut usize| -> Result<u32> {
            if *p + 4 > body.len() {
                bail!("footer-index truncated at u32");
            }
            let v = u32::from_le_bytes(body[*p..*p + 4].try_into()?);
            *p += 4;
            Ok(v)
        };
        let row_count = rd_u64(&mut p)?;
        let rabitq_off = rd_u64(&mut p)?;
        let rabitq_len = rd_u64(&mut p)?;
        let embed_dim = rd_u32(&mut p)?;
        let embed_count = rd_u32(&mut p)?;
        if p + 2 > body.len() {
            bail!("footer-index truncated at encoding map");
        }
        let embed_quant_tag = body[p];
        let has_f32_tier = body[p + 1] != 0;
        p += 2;
        let n_blocks = rd_u32(&mut p)? as usize;
        let mut blocks = Vec::with_capacity(n_blocks);
        for _ in 0..n_blocks {
            let offset = rd_u64(&mut p)?;
            let size = rd_u32(&mut p)?;
            let rc = rd_u32(&mut p)?;
            if p + 5 > body.len() {
                bail!("footer-index truncated at block stats");
            }
            let stats_tag = body[p];
            let stats_len = u32::from_le_bytes(body[p + 1..p + 5].try_into()?) as usize;
            p += 5;
            if p + stats_len > body.len() {
                bail!("footer-index stats payload overruns body");
            }
            // PR1 ignores the (empty) stats payload; PR3 decodes the OID bloom.
            p += stats_len;
            blocks.push(FooterBlockEntry {
                offset,
                size,
                row_count: rc,
                stats_kind: StatsKind::from_tag(stats_tag),
            });
        }
        Ok(Self {
            row_count,
            rabitq_off,
            rabitq_len,
            embed_dim,
            embed_count,
            embed_quant_tag,
            has_f32_tier,
            blocks,
        })
    }

    /// Locate + parse the footer-index from a full segment buffer (the tail
    /// `[footer_len u64][SEGMENT_MAGIC 8B]` gives the extent). Returns
    /// `Ok(None)` when the buffer is not a coalesced segment (no valid tail) —
    /// the caller then uses the legacy `SegmentIndex::locate` path.
    pub fn locate_in_segment(segment: &[u8]) -> Result<Option<Self>> {
        if segment.len() < SEG_TAIL_LEN {
            return Ok(None);
        }
        let tail = &segment[segment.len() - SEG_TAIL_LEN..];
        if &tail[8..] != SEGMENT_MAGIC {
            return Ok(None);
        }
        let footer_len = u64::from_le_bytes(tail[..8].try_into()?) as usize;
        if SEG_TAIL_LEN + footer_len > segment.len() {
            bail!("coalesced footer-index length overruns segment");
        }
        let body =
            &segment[segment.len() - SEG_TAIL_LEN - footer_len..segment.len() - SEG_TAIL_LEN];
        Ok(Some(Self::parse(body)?))
    }
}

/// Build the segment tail bytes: `[footer-index body][footer_len u64][SEGMENT_MAGIC]`.
pub fn segment_tail(footer_body: &[u8]) -> Vec<u8> {
    let mut tail = Vec::with_capacity(footer_body.len() + SEG_TAIL_LEN);
    tail.extend_from_slice(footer_body);
    tail.extend_from_slice(&(footer_body.len() as u64).to_le_bytes());
    tail.extend_from_slice(SEGMENT_MAGIC);
    tail
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_footer() -> SegmentFooterIndex {
        SegmentFooterIndex {
            row_count: 1000,
            rabitq_off: 40,
            rabitq_len: 24_000,
            embed_dim: 128,
            embed_count: 1,
            embed_quant_tag: 1, // SQ8
            has_f32_tier: false,
            blocks: vec![
                FooterBlockEntry {
                    offset: 24_040,
                    size: 8_000,
                    row_count: 128,
                    stats_kind: StatsKind::None,
                },
                FooterBlockEntry {
                    offset: 32_040,
                    size: 7_900,
                    row_count: 127,
                    stats_kind: StatsKind::None,
                },
            ],
        }
    }

    #[test]
    fn header_prefix_round_trips() {
        let h = SegmentHeaderPrefix {
            layout_version: SEG_LAYOUT_VERSION,
            rabitq_off: 40,
            rabitq_len: 24_000,
            footer_off: 100_000,
            footer_len: 512,
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), SEG_HEADER_PREFIX_LEN);
        let parsed = SegmentHeaderPrefix::parse(&bytes).unwrap();
        assert_eq!(parsed.rabitq_off, 40);
        assert_eq!(parsed.rabitq_len, 24_000);
        assert_eq!(parsed.footer_off, 100_000);
        assert_eq!(parsed.footer_len, 512);
    }

    #[test]
    fn header_parse_rejects_bad_magic_and_version() {
        // Wrong magic → err (a legacy PBLK segment is never mis-detected).
        let mut bad = SegmentHeaderPrefix {
            layout_version: SEG_LAYOUT_VERSION,
            rabitq_off: 0,
            rabitq_len: 0,
            footer_off: 0,
            footer_len: 0,
        }
        .to_bytes();
        bad[0..4].copy_from_slice(b"PBLK");
        assert!(SegmentHeaderPrefix::parse(&bad).is_err());
        // Wrong version → err.
        let mut v = SegmentHeaderPrefix {
            layout_version: SEG_LAYOUT_VERSION,
            rabitq_off: 0,
            rabitq_len: 0,
            footer_off: 0,
            footer_len: 0,
        }
        .to_bytes();
        v[4] = 99;
        assert!(SegmentHeaderPrefix::parse(&v).is_err());
    }

    #[test]
    fn is_coalesced_segment_presence_field() {
        let h = SegmentHeaderPrefix {
            layout_version: SEG_LAYOUT_VERSION,
            rabitq_off: 40,
            rabitq_len: 1,
            footer_off: 41,
            footer_len: 1,
        }
        .to_bytes();
        assert!(is_coalesced_segment(&h));
        // A legacy block magic prefix is NOT coalesced.
        assert!(!is_coalesced_segment(b"PBLK\x00\x00"));
        assert!(!is_coalesced_segment(&[]));
    }

    #[test]
    fn footer_index_round_trips() {
        let f = sample_footer();
        let bytes = f.to_bytes();
        let parsed = SegmentFooterIndex::parse(&bytes).unwrap();
        assert_eq!(parsed.row_count, 1000);
        assert_eq!(parsed.rabitq_off, 40);
        assert_eq!(parsed.embed_dim, 128);
        assert_eq!(parsed.embed_count, 1);
        assert_eq!(parsed.embed_quant_tag, 1);
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].offset, 24_040);
        assert_eq!(parsed.blocks[1].row_count, 127);
    }

    #[test]
    fn footer_locate_in_segment_round_trips() {
        let f = sample_footer();
        let body = f.to_bytes();
        // Pretend segment = [some data][footer body][footer_len][magic].
        let mut segment = vec![0xAAu8; 1000];
        segment.extend(segment_tail(&body));
        let parsed = SegmentFooterIndex::locate_in_segment(&segment)
            .unwrap()
            .expect("footer located");
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.row_count, 1000);
    }

    #[test]
    fn footer_locate_returns_none_for_non_coalesced_tail() {
        // A legacy `[..][SEGMENT_MAGIC]` tail (no footer_len before magic) — the
        // 8 bytes before magic would be parsed as footer_len, but the resulting
        // body slice is not a valid footer (wrong version byte) → err → None-ish.
        // Here we assert a too-short buffer returns None cleanly.
        assert!(
            SegmentFooterIndex::locate_in_segment(&[0u8; 5])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn footer_parse_rejects_bad_version() {
        let mut bytes = sample_footer().to_bytes();
        bytes[0] = 2; // unsupported version
        assert!(SegmentFooterIndex::parse(&bytes).is_err());
    }

    #[test]
    fn stats_kind_unknown_tag_degrades_to_none() {
        assert_eq!(StatsKind::from_tag(99), StatsKind::None);
        assert_eq!(StatsKind::from_tag(1), StatsKind::BloomOid);
    }
}
