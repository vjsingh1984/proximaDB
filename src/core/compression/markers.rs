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

// Compression format markers - used in all storage engines
pub const MARKER_UNCOMPRESSED: u8 = 0x02;
pub const MARKER_ZSTD: u8 = 0x03;
pub const MARKER_LZ4: u8 = 0x04;
pub const MARKER_SNAPPY: u8 = 0x05;
pub const MARKER_GZIP: u8 = 0x06;
pub const MARKER_BROTLI: u8 = 0x07;
pub const MARKER_BZIP2: u8 = 0x08;
pub const MARKER_DEFLATE: u8 = 0x09;
pub const MARKER_XZ: u8 = 0x0A;
pub const MARKER_ZLIB: u8 = 0x0B;
pub const MARKER_LZ4HC: u8 = 0x0C;
pub const MARKER_LZMA: u8 = 0x0D;
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
            let marker = get_compression_marker(&algorithm);
            assert!(
                markers.insert(marker),
                "Duplicate marker 0x{:02x} for {:?}",
                marker,
                algorithm
            );
        }
    }
}
