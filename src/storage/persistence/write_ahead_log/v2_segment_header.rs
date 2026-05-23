//! WAL segment header v2 — embedding-precision rollout PR 4.
//!
//! Every precision-aware WAL segment begins with this header so the reader
//! can fast-path buffer sizing on the segment's canonical default precision
//! before scanning per-record tags. The header is the authoritative source
//! for "is this a v2 segment?"; per-record `schema_version` byte is stamped
//! from this header by the dispatching reader (PR 2 plumbing).
//!
//! Layout (all multi-byte fields little-endian — enforced by the LE compile
//! assert in `apps/proximadb-server/src/main.rs`):
//!
//! ```text
//! | magic (4 B = b"PWAL")
//! | version (2 B = 2)
//! | header_len (2 B = total bytes of this header)
//! | flags (4 B = reserved, 0 today)
//! | segment_id (16 B = u128 ULID)
//! | created_at_ns (8 B = i64 wall-clock)
//! | canonical_default_precision (1 B = EmbeddingScalarType discriminant)
//! | precision_epoch (8 B = u64)
//! | policy_id_len (2 B = u16)
//! | policy_id (UTF-8 bytes, policy_id_len bytes)
//! | policy_version (8 B = u64)
//! | reserved (8 B = zero, future use)
//! ```
//!
//! Spec: `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc`
//! §"WAL segment header (Q5)" and §"PR 4 — WAL segment header v2".

use anyhow::{Result, anyhow, bail};
use proximadb_records::EmbeddingScalarType;

/// File-format magic: 4 bytes `b"PWAL"`. Same magic across v1 and v2 — the
/// 2-byte version that follows is what distinguishes them.
pub const PWAL_MAGIC: &[u8; 4] = b"PWAL";

/// Header version emitted by the precision-aware writer.
pub const SEGMENT_HEADER_VERSION_V2: u16 = 2;

/// Header version for the pre-precision (legacy) segment format.
pub const SEGMENT_HEADER_VERSION_V1: u16 = 1;

/// Minimum bytes the reader needs before it can decide v1 vs v2 dispatch.
/// (`magic` + `version` = 6 bytes.)
pub const PWAL_PEEK_LEN: usize = 6;

/// Fixed-size portion of the v2 header (everything except the variable
/// `policy_id` UTF-8 bytes).
///
/// `magic(4) + version(2) + header_len(2) + flags(4) + segment_id(16) +
/// created_at_ns(8) + precision(1) + precision_epoch(8) + policy_id_len(2) +
/// policy_version(8) + reserved(8) = 63`.
pub const V2_HEADER_FIXED_LEN: usize = 4 + 2 + 2 + 4 + 16 + 8 + 1 + 8 + 2 + 8 + 8;

/// Parsed v2 segment header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2SegmentHeader {
    /// Bitfield reserved for future use. Writers emit 0.
    pub flags: u32,
    /// ULID identifying the segment.
    pub segment_id: u128,
    /// Wall-clock timestamp at segment creation (nanoseconds since UNIX epoch).
    pub created_at_ns: i64,
    /// Canonical precision the writer used by default. Per-record tags MAY
    /// override this to support mixed-precision segments during epoch
    /// transitions without forcing compaction first.
    pub canonical_default_precision: EmbeddingScalarType,
    /// Precision-policy epoch this segment was opened under (catalog-managed).
    pub precision_epoch: u64,
    /// Catalog policy id (UTF-8 string, may be empty when policy is unnamed).
    pub policy_id: String,
    /// Monotonic policy version (bumped on each policy change in the catalog).
    pub policy_version: u64,
}

/// Result of peeking the first 6 bytes of a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekedSegmentVersion {
    /// Pre-precision segment — caller dispatches to the legacy reader and
    /// treats every embedding as `Fp32`.
    V1,
    /// Precision-aware segment — caller continues parsing the v2 header.
    V2,
}

/// Inspect just the magic + version bytes at the start of a segment.
///
/// Returns the version variant or an error if the magic doesn't match (the
/// caller is reading something that isn't a WAL segment).
pub fn peek_segment_version(data: &[u8]) -> Result<PeekedSegmentVersion> {
    if data.len() < PWAL_PEEK_LEN {
        bail!(
            "WAL segment too short for header peek: need {} bytes, got {}",
            PWAL_PEEK_LEN,
            data.len()
        );
    }
    if &data[..4] != PWAL_MAGIC {
        bail!(
            "WAL segment magic mismatch: expected {:?}, got {:?}",
            PWAL_MAGIC,
            &data[..4]
        );
    }
    let version = u16::from_le_bytes([data[4], data[5]]);
    match version {
        SEGMENT_HEADER_VERSION_V1 => Ok(PeekedSegmentVersion::V1),
        SEGMENT_HEADER_VERSION_V2 => Ok(PeekedSegmentVersion::V2),
        other => bail!(
            "unsupported WAL segment header version: {} (expected {} or {})",
            other,
            SEGMENT_HEADER_VERSION_V1,
            SEGMENT_HEADER_VERSION_V2
        ),
    }
}

impl V2SegmentHeader {
    /// Serialize this header into bytes ready to write at the start of a
    /// segment file. Patches the `header_len` field after layout so callers
    /// don't have to predict the policy_id length.
    pub fn encode(&self) -> Vec<u8> {
        let policy_id_bytes = self.policy_id.as_bytes();
        let total = V2_HEADER_FIXED_LEN + policy_id_bytes.len();
        let header_len = u16::try_from(total).unwrap_or(u16::MAX);

        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(PWAL_MAGIC);
        buf.extend_from_slice(&SEGMENT_HEADER_VERSION_V2.to_le_bytes());
        buf.extend_from_slice(&header_len.to_le_bytes());
        buf.extend_from_slice(&self.flags.to_le_bytes());
        buf.extend_from_slice(&self.segment_id.to_le_bytes());
        buf.extend_from_slice(&self.created_at_ns.to_le_bytes());
        buf.push(self.canonical_default_precision as u8);
        buf.extend_from_slice(&self.precision_epoch.to_le_bytes());
        buf.extend_from_slice(
            &u16::try_from(policy_id_bytes.len())
                .unwrap_or(u16::MAX)
                .to_le_bytes(),
        );
        buf.extend_from_slice(policy_id_bytes);
        buf.extend_from_slice(&self.policy_version.to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]); // reserved

        debug_assert_eq!(buf.len(), total, "encode total length mismatch");
        buf
    }

    /// Parse a v2 segment header from the start of `data`.
    ///
    /// Returns the header and the number of bytes consumed (so the caller can
    /// continue reading records from that offset).
    ///
    /// Fails if the magic, version, declared length, precision discriminant,
    /// or UTF-8 policy id are invalid. v1 segments must be routed to the
    /// legacy reader via [`peek_segment_version`] before calling this.
    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        match peek_segment_version(data)? {
            PeekedSegmentVersion::V1 => {
                bail!(
                    "decode called on a v1 segment; route via peek_segment_version first"
                );
            }
            PeekedSegmentVersion::V2 => {}
        }

        if data.len() < V2_HEADER_FIXED_LEN {
            bail!(
                "WAL v2 header truncated: need at least {} bytes, got {}",
                V2_HEADER_FIXED_LEN,
                data.len()
            );
        }

        let header_len = u16::from_le_bytes([data[6], data[7]]) as usize;
        if header_len < V2_HEADER_FIXED_LEN {
            bail!(
                "WAL v2 header_len {} is smaller than fixed minimum {}",
                header_len,
                V2_HEADER_FIXED_LEN
            );
        }
        if data.len() < header_len {
            bail!(
                "WAL v2 header truncated: declared {} bytes, only {} present",
                header_len,
                data.len()
            );
        }

        let mut off = 8;
        let flags = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        off += 4;
        let segment_id = u128::from_le_bytes(data[off..off + 16].try_into().unwrap());
        off += 16;
        let created_at_ns = i64::from_le_bytes(data[off..off + 8].try_into().unwrap());
        off += 8;
        let canonical_default_precision = match data[off] {
            0x01 => EmbeddingScalarType::Fp32,
            0x02 => EmbeddingScalarType::Fp16,
            0x03 => EmbeddingScalarType::Bf16,
            0x04 => EmbeddingScalarType::Int8Scalar,
            0x05 => EmbeddingScalarType::UInt8Scalar,
            other => {
                bail!(
                    "unknown canonical_default_precision discriminant 0x{:02x}",
                    other
                );
            }
        };
        off += 1;
        let precision_epoch = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
        off += 8;
        let policy_id_len = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
        off += 2;

        // The fixed portion accounts for everything *except* the policy_id
        // bytes. So the declared header_len must equal V2_HEADER_FIXED_LEN +
        // policy_id_len, otherwise we'd misalign on the next fields.
        if header_len != V2_HEADER_FIXED_LEN + policy_id_len {
            bail!(
                "WAL v2 header_len {} doesn't match fixed {} + policy_id_len {}",
                header_len,
                V2_HEADER_FIXED_LEN,
                policy_id_len
            );
        }

        let policy_id = std::str::from_utf8(&data[off..off + policy_id_len])
            .map_err(|e| anyhow!("policy_id is not valid UTF-8: {e}"))?
            .to_string();
        off += policy_id_len;
        let policy_version = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
        off += 8;
        // reserved (8 bytes) — skipped, currently must be zero by convention
        // but we don't reject non-zero so forward-compat upgrades stay easy.
        off += 8;

        debug_assert_eq!(off, header_len, "decode consumed length mismatch");

        Ok((
            Self {
                flags,
                segment_id,
                created_at_ns,
                canonical_default_precision,
                precision_epoch,
                policy_id,
                policy_version,
            },
            header_len,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> V2SegmentHeader {
        V2SegmentHeader {
            flags: 0,
            segment_id: 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210u128,
            created_at_ns: 1_700_000_000_000_000_000,
            canonical_default_precision: EmbeddingScalarType::Fp16,
            precision_epoch: 42,
            policy_id: "tenant-acme/precision-policy".to_string(),
            policy_version: 7,
        }
    }

    #[test]
    fn magic_constant_is_pwal_ascii() {
        assert_eq!(PWAL_MAGIC, b"PWAL");
    }

    #[test]
    fn version_constants_match_lld() {
        assert_eq!(SEGMENT_HEADER_VERSION_V1, 1);
        assert_eq!(SEGMENT_HEADER_VERSION_V2, 2);
    }

    #[test]
    fn peek_dispatches_v1_and_v2() {
        let mut v1 = Vec::new();
        v1.extend_from_slice(PWAL_MAGIC);
        v1.extend_from_slice(&1u16.to_le_bytes());
        assert_eq!(peek_segment_version(&v1).unwrap(), PeekedSegmentVersion::V1);

        let mut v2 = Vec::new();
        v2.extend_from_slice(PWAL_MAGIC);
        v2.extend_from_slice(&2u16.to_le_bytes());
        assert_eq!(peek_segment_version(&v2).unwrap(), PeekedSegmentVersion::V2);
    }

    #[test]
    fn peek_rejects_unknown_version() {
        let mut bad = Vec::new();
        bad.extend_from_slice(PWAL_MAGIC);
        bad.extend_from_slice(&99u16.to_le_bytes());
        let err = peek_segment_version(&bad).unwrap_err().to_string();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    #[test]
    fn peek_rejects_wrong_magic() {
        let mut bad = Vec::new();
        bad.extend_from_slice(b"SOMETHING_ELSE");
        let err = peek_segment_version(&bad).unwrap_err().to_string();
        assert!(err.contains("magic mismatch"), "got: {err}");
    }

    #[test]
    fn peek_rejects_short_buffer() {
        assert!(peek_segment_version(&[]).is_err());
        assert!(peek_segment_version(b"PWA").is_err());
    }

    #[test]
    fn encode_decode_round_trip() {
        let header = sample_header();
        let bytes = header.encode();
        let (back, consumed) = V2SegmentHeader::decode(&bytes).unwrap();
        assert_eq!(back, header);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn encode_decode_round_trip_empty_policy_id() {
        let mut header = sample_header();
        header.policy_id = String::new();
        let bytes = header.encode();
        assert_eq!(bytes.len(), V2_HEADER_FIXED_LEN);
        let (back, consumed) = V2SegmentHeader::decode(&bytes).unwrap();
        assert_eq!(back, header);
        assert_eq!(consumed, V2_HEADER_FIXED_LEN);
    }

    #[test]
    fn encode_round_trip_every_precision_variant() {
        for prec in [
            EmbeddingScalarType::Fp32,
            EmbeddingScalarType::Fp16,
            EmbeddingScalarType::Bf16,
            EmbeddingScalarType::Int8Scalar,
            EmbeddingScalarType::UInt8Scalar,
        ] {
            let mut header = sample_header();
            header.canonical_default_precision = prec;
            let bytes = header.encode();
            let (back, _) = V2SegmentHeader::decode(&bytes).unwrap();
            assert_eq!(back.canonical_default_precision, prec, "round-trip {prec:?}");
        }
    }

    #[test]
    fn decode_rejects_v1_segment() {
        let mut v1 = Vec::new();
        v1.extend_from_slice(PWAL_MAGIC);
        v1.extend_from_slice(&1u16.to_le_bytes());
        v1.extend_from_slice(&[0u8; 64]); // pad so decode reaches the version check
        let err = V2SegmentHeader::decode(&v1).unwrap_err().to_string();
        assert!(err.contains("v1 segment"), "got: {err}");
    }

    #[test]
    fn decode_rejects_unknown_precision_discriminant() {
        let mut bytes = sample_header().encode();
        // Locate the precision byte: 4+2+2+4+16+8 = 36
        bytes[36] = 0x99;
        let err = V2SegmentHeader::decode(&bytes).unwrap_err().to_string();
        assert!(err.contains("0x99"), "got: {err}");
    }

    #[test]
    fn decode_rejects_truncated_buffer() {
        let bytes = sample_header().encode();
        for cut in 0..bytes.len() {
            assert!(
                V2SegmentHeader::decode(&bytes[..cut]).is_err(),
                "decode succeeded on truncated buffer (cut at {cut})"
            );
        }
    }

    #[test]
    fn decode_rejects_inconsistent_header_len() {
        let mut bytes = sample_header().encode();
        // Bump header_len (offset 6..8) by 4 — now declared > actual policy_id room.
        let bad = (bytes.len() as u16).wrapping_add(4).to_le_bytes();
        bytes[6..8].copy_from_slice(&bad);
        assert!(V2SegmentHeader::decode(&bytes).is_err());
    }

    #[test]
    fn encode_layout_matches_lld_byte_offsets() {
        let header = sample_header();
        let bytes = header.encode();
        // magic
        assert_eq!(&bytes[0..4], PWAL_MAGIC);
        // version
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2);
        // header_len
        assert_eq!(
            u16::from_le_bytes([bytes[6], bytes[7]]) as usize,
            bytes.len()
        );
        // flags
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 0);
        // canonical_default_precision at offset 36
        assert_eq!(bytes[36], EmbeddingScalarType::Fp16 as u8);
    }
}
