/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Compression format markers for ProximaDB
//!
//! These markers are used to identify the compression algorithm used
//! in serialized data blocks across all storage engines.

use super::CompressionAlgorithm;

/// Byte marker for uncompressed data blocks
pub const MARKER_UNCOMPRESSED: u8 = 0x02;
/// Byte marker for Zstandard compressed data
pub const MARKER_ZSTD: u8 = 0x03;
/// Byte marker for LZ4 compressed data
pub const MARKER_LZ4: u8 = 0x04;
/// Byte marker for Snappy compressed data
pub const MARKER_SNAPPY: u8 = 0x05;
/// Byte marker for Gzip compressed data
pub const MARKER_GZIP: u8 = 0x06;
/// Byte marker for Brotli compressed data
pub const MARKER_BROTLI: u8 = 0x07;
/// Byte marker for Bzip2 compressed data
pub const MARKER_BZIP2: u8 = 0x08;
/// Byte marker for Deflate compressed data
pub const MARKER_DEFLATE: u8 = 0x09;
/// Byte marker for XZ/LZMA2 compressed data
pub const MARKER_XZ: u8 = 0x0A;
/// Byte marker for Zlib compressed data
pub const MARKER_ZLIB: u8 = 0x0B;
/// Byte marker for LZ4 high-compression data
pub const MARKER_LZ4HC: u8 = 0x0C;
/// Byte marker for LZMA compressed data
pub const MARKER_LZMA: u8 = 0x0D;
/// Byte marker for LZO compressed data
pub const MARKER_LZO: u8 = 0x0E;

/// Get compression marker for algorithm
pub fn compression_marker(algorithm: &CompressionAlgorithm) -> u8 {
    match algorithm {
        CompressionAlgorithm::None => MARKER_UNCOMPRESSED,
        CompressionAlgorithm::Zstd => MARKER_ZSTD,
        CompressionAlgorithm::Lz4 => MARKER_LZ4,
        CompressionAlgorithm::Snappy => MARKER_SNAPPY,
        CompressionAlgorithm::Gzip => MARKER_GZIP,
        CompressionAlgorithm::Brotli => MARKER_BROTLI,
        CompressionAlgorithm::Bzip2 => MARKER_BZIP2,
        CompressionAlgorithm::Deflate => MARKER_DEFLATE,
        CompressionAlgorithm::Xz => MARKER_XZ,
        CompressionAlgorithm::Zlib => MARKER_ZLIB,
        CompressionAlgorithm::Lz4hc => MARKER_LZ4HC,
        CompressionAlgorithm::Lzma => MARKER_LZMA,
        CompressionAlgorithm::Lzo => MARKER_LZO,
        CompressionAlgorithm::Mixed => MARKER_UNCOMPRESSED, // Mixed uses uncompressed marker
    }
}

/// Get unified compression algorithm from marker
pub fn compression_algorithm_from_marker(marker: u8) -> CompressionAlgorithm {
    match marker {
        MARKER_UNCOMPRESSED => CompressionAlgorithm::None,
        MARKER_ZSTD => CompressionAlgorithm::Zstd,
        MARKER_LZ4 => CompressionAlgorithm::Lz4,
        MARKER_SNAPPY => CompressionAlgorithm::Snappy,
        MARKER_GZIP => CompressionAlgorithm::Gzip,
        MARKER_BROTLI => CompressionAlgorithm::Brotli,
        MARKER_BZIP2 => CompressionAlgorithm::Bzip2,
        MARKER_DEFLATE => CompressionAlgorithm::Deflate,
        MARKER_XZ => CompressionAlgorithm::Xz,
        MARKER_ZLIB => CompressionAlgorithm::Zlib,
        MARKER_LZ4HC => CompressionAlgorithm::Lz4hc,
        MARKER_LZMA => CompressionAlgorithm::Lzma,
        MARKER_LZO => CompressionAlgorithm::Lzo,
        _ => CompressionAlgorithm::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_marker_roundtrip() {
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Brotli,
            CompressionAlgorithm::Bzip2,
            CompressionAlgorithm::Deflate,
            CompressionAlgorithm::Xz,
            CompressionAlgorithm::Zlib,
            CompressionAlgorithm::Lz4hc,
            CompressionAlgorithm::Lzma,
            CompressionAlgorithm::Lzo,
        ];

        for algorithm in algorithms {
            let marker = compression_marker(&algorithm);
            let roundtrip = compression_algorithm_from_marker(marker);
            assert_eq!(algorithm, roundtrip, "Roundtrip failed for {:?}", algorithm);
        }
    }

    #[test]
    fn test_marker_uniqueness() {
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Brotli,
            CompressionAlgorithm::Bzip2,
            CompressionAlgorithm::Deflate,
            CompressionAlgorithm::Xz,
            CompressionAlgorithm::Zlib,
            CompressionAlgorithm::Lz4hc,
            CompressionAlgorithm::Lzma,
            CompressionAlgorithm::Lzo,
        ];

        let mut markers = std::collections::HashSet::new();
        for algorithm in algorithms {
            let marker = compression_marker(&algorithm);
            assert!(
                markers.insert(marker),
                "Duplicate marker 0x{:02x} for {:?}",
                marker,
                algorithm
            );
        }
    }
}
