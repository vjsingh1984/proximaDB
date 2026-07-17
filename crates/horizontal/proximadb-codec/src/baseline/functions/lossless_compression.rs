// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Reversible compression of bytes already produced by a value codec.

use anyhow::Result;
use proximadb_compression::{CompressionAlgorithm, CompressionContext};

/// LZ4-compress encoded bytes when the compressed payload clears `min_savings`.
///
/// The returned bytes are the shared compressor's normal size-prefixed LZ4
/// payload. The owning layout records the compressor tag alongside its existing
/// value-encoding tag; no nested envelope or additional format version exists.
pub fn compress_lz4_if_smaller(bytes: &[u8], min_savings: usize) -> Result<Option<Vec<u8>>> {
    let compressed = proximadb_compression::compress(
        bytes,
        CompressionAlgorithm::Lz4,
        1,
        CompressionContext::Column,
    )?;
    let required = compressed
        .len()
        .checked_add(min_savings)
        .ok_or_else(|| anyhow::anyhow!("lossless compression saving threshold overflow"))?;
    Ok((required <= bytes.len()).then_some(compressed))
}

/// Exactly reverse [`compress_lz4_if_smaller`].
pub fn decompress_lz4(bytes: &[u8]) -> Result<Vec<u8>> {
    proximadb_compression::decompress(bytes, CompressionAlgorithm::Lz4, CompressionContext::Column)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::{compress_lz4_if_smaller, decompress_lz4};

    #[test]
    fn lossless_compression_round_trips_exact_encoded_bytes() -> Result<()> {
        let bytes = vec![0xff; 16 * 1024];
        let compressed = compress_lz4_if_smaller(&bytes, 8)?
            .ok_or_else(|| anyhow::anyhow!("repetitive payload did not compress"))?;
        assert!(compressed.len() < bytes.len());
        assert_eq!(decompress_lz4(&compressed)?, bytes);
        Ok(())
    }

    #[test]
    fn lossless_compression_keeps_incompressible_small_payload_flat() -> Result<()> {
        let bytes: Vec<u8> = (0u8..=u8::MAX).collect();
        assert!(compress_lz4_if_smaller(&bytes, 8)?.is_none());
        Ok(())
    }

    #[test]
    fn lossless_compression_corruption_fails_closed() -> Result<()> {
        let bytes = vec![7u8; 4096];
        let mut compressed = compress_lz4_if_smaller(&bytes, 0)?
            .ok_or_else(|| anyhow::anyhow!("repetitive payload did not compress"))?;
        compressed.truncate(compressed.len() / 2);
        assert!(decompress_lz4(&compressed).is_err());
        Ok(())
    }
}
