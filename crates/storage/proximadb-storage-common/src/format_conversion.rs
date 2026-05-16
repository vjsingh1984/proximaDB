#![allow(dead_code)]
//! Format conversion utilities for storage format and quantization format interop.

use anyhow::Result;
use proximadb_runtime_common::pool::VectorMemoryPool;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Storage formats supported by the universal adapter
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageFormat {
    FP32,
    FP16,
    QuantizedINT8 {
        scale: f32,
        zero_point: i32,
    },
    QuantizedPQ {
        segments: usize,
        bits: usize,
    },
    Binary,
    Custom {
        format_name: String,
        metadata: HashMap<String, String>,
    },
}

/// Quantization formats for computation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuantizedFormat {
    INT8 {
        scale: f32,
        zero_point: i32,
    },
    PQ {
        segments: usize,
        bits: usize,
        codebook: Option<Vec<Vec<Vec<f32>>>>,
    },
    Binary {
        threshold: f32,
    },
    Scalar {
        min_value: f32,
        max_value: f32,
        levels: usize,
    },
}

/// Compression formats for storage
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CompressionFormat {
    None,
    ZSTD {
        level: i32,
    },
    LZ4,
    Snappy,
    Custom {
        algorithm: String,
        parameters: HashMap<String, String>,
    },
}

/// Format converter for storage and quantization formats
pub struct FormatConverter {
    conversion_cache: HashMap<String, Vec<u8>>,
    conversion_stats: ConversionStatistics,
    memory_pool: Arc<VectorMemoryPool>,
}

impl std::fmt::Debug for FormatConverter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormatConverter")
            .field("conversion_cache_size", &self.conversion_cache.len())
            .field("conversion_stats", &self.conversion_stats)
            .field("memory_pool", &"<VectorMemoryPool>")
            .finish()
    }
}

/// Statistics for format conversions
#[derive(Debug, Clone, Default)]
pub struct ConversionStatistics {
    pub total_conversions: u64,
    pub conversions_per_format: HashMap<String, u64>,
    pub average_conversion_time_us: u64,
    pub cache_hit_rate: f32,
    pub total_conversion_time_us: u64,
}

/// Error types for format conversion
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("Unsupported conversion from {from} to {to}")]
    UnsupportedConversion { from: String, to: String },

    #[error("Invalid format parameters: {0}")]
    InvalidParameters(String),

    #[error("Data size mismatch: expected {expected}, got {actual}")]
    DataSizeMismatch { expected: usize, actual: usize },

    #[error("Quantization error: {0}")]
    QuantizationError(String),

    #[error("Compression error: {0}")]
    CompressionError(String),

    #[error("Internal conversion error: {0}")]
    Internal(String),
}

/// Result type for conversion operations
pub type ConversionResult<T> = Result<T, ConversionError>;

impl FormatConverter {
    pub async fn new() -> ConversionResult<Self> {
        Ok(Self {
            conversion_cache: HashMap::new(),
            conversion_stats: ConversionStatistics::default(),
            memory_pool: Arc::new(VectorMemoryPool::new()),
        })
    }

    pub async fn with_memory_pool(memory_pool: Arc<VectorMemoryPool>) -> ConversionResult<Self> {
        Ok(Self {
            conversion_cache: HashMap::new(),
            conversion_stats: ConversionStatistics::default(),
            memory_pool,
        })
    }

    pub async fn to_int8(&self, data: &[u8]) -> ConversionResult<Vec<i8>> {
        let start_time = std::time::Instant::now();
        trace!("Converting {} bytes to INT8 format", data.len());

        let result = if data.len().is_multiple_of(4) {
            self.fp32_to_int8(data).await?
        } else {
            data.iter().map(|&b| b as i8).collect()
        };

        let conversion_time = start_time.elapsed().as_micros() as u64;
        debug!("INT8 conversion completed in {}μs", conversion_time);
        Ok(result)
    }

    pub async fn to_pq(
        &self,
        data: &[u8],
        segments: usize,
        bits: usize,
    ) -> ConversionResult<Vec<u8>> {
        let start_time = std::time::Instant::now();
        trace!(
            "Converting {} bytes to PQ format (segments: {}, bits: {})",
            data.len(),
            segments,
            bits
        );

        let float_data = if data.len().is_multiple_of(4) {
            self.bytes_to_fp32(data)?
        } else {
            return Err(ConversionError::InvalidParameters(
                "PQ conversion requires FP32 input data".to_string(),
            ));
        };

        let pq_codes = self.quantize_to_pq(&float_data, segments, bits).await?;
        let conversion_time = start_time.elapsed().as_micros() as u64;
        debug!("PQ conversion completed in {}μs", conversion_time);
        Ok(pq_codes)
    }

    pub async fn to_binary(&self, data: &[u8]) -> ConversionResult<Vec<u8>> {
        let start_time = std::time::Instant::now();
        trace!("Converting {} bytes to binary format", data.len());

        let float_data = if data.len().is_multiple_of(4) {
            self.bytes_to_fp32(data)?
        } else {
            return Err(ConversionError::InvalidParameters(
                "Binary conversion requires FP32 input data".to_string(),
            ));
        };

        let binary_data = self.quantize_to_binary(&float_data, 0.0).await?;
        let conversion_time = start_time.elapsed().as_micros() as u64;
        debug!("Binary conversion completed in {}μs", conversion_time);
        Ok(binary_data)
    }

    pub async fn convert_storage_format(
        &self,
        data: &[u8],
        from_format: &StorageFormat,
        to_format: &StorageFormat,
    ) -> ConversionResult<Vec<u8>> {
        let start_time = std::time::Instant::now();
        trace!("Converting from {:?} to {:?}", from_format, to_format);

        if from_format == to_format {
            return Ok(data.to_vec());
        }

        let result = match (from_format, to_format) {
            (StorageFormat::FP32, StorageFormat::QuantizedINT8 { scale, zero_point }) => {
                let float_data = self.bytes_to_fp32(data)?;
                let int8_data = self.fp32_to_int8_with_params(&float_data, *scale, *zero_point)?;
                int8_data.into_iter().map(|i| i as u8).collect()
            }
            (StorageFormat::FP32, StorageFormat::QuantizedPQ { segments, bits }) => {
                let float_data = self.bytes_to_fp32(data)?;
                self.quantize_to_pq(&float_data, *segments, *bits).await?
            }
            (StorageFormat::FP32, StorageFormat::Binary) => {
                let float_data = self.bytes_to_fp32(data)?;
                self.quantize_to_binary(&float_data, 0.0).await?
            }
            (StorageFormat::QuantizedINT8 { scale, zero_point }, StorageFormat::FP32) => {
                let int8_data: Vec<i8> = data.iter().map(|&b| b as i8).collect();
                let float_data = self.int8_to_fp32(&int8_data, *scale, *zero_point)?;
                self.fp32_to_bytes(&float_data)?
            }
            _ => {
                return Err(ConversionError::UnsupportedConversion {
                    from: format!("{:?}", from_format),
                    to: format!("{:?}", to_format),
                });
            }
        };

        let conversion_time = start_time.elapsed().as_micros() as u64;
        debug!(
            "Storage format conversion completed in {}μs",
            conversion_time
        );
        Ok(result)
    }

    pub async fn compress_data(
        &self,
        data: &[u8],
        format: &CompressionFormat,
    ) -> ConversionResult<Vec<u8>> {
        let start_time = std::time::Instant::now();
        trace!("Compressing {} bytes using {:?}", data.len(), format);

        let result = match format {
            CompressionFormat::None => data.to_vec(),
            CompressionFormat::ZSTD { level } => self.compress_zstd_pooled(data, *level).await?,
            CompressionFormat::LZ4 => self.compress_lz4_pooled(data).await?,
            CompressionFormat::Snappy => self.compress_snappy_pooled(data).await?,
            CompressionFormat::Custom { algorithm, .. } => {
                warn!(
                    "Custom compression algorithm '{}' not implemented, using no compression",
                    algorithm
                );
                data.to_vec()
            }
        };

        let compression_time = start_time.elapsed().as_micros() as u64;
        debug!(
            "Compression completed in {}μs, ratio: {:.2}",
            compression_time,
            data.len() as f32 / result.len() as f32
        );
        Ok(result)
    }

    pub async fn compress_data_into(
        &self,
        data: &[u8],
        format: &CompressionFormat,
        output: &mut Vec<u8>,
    ) -> ConversionResult<usize> {
        output.clear();
        match format {
            CompressionFormat::None => {
                output.extend_from_slice(data);
                Ok(data.len())
            }
            _ => {
                let compressed = self.compress_data(data, format).await?;
                let size = compressed.len();
                output.extend_from_slice(&compressed);
                Ok(size)
            }
        }
    }

    pub async fn decompress_data(
        &self,
        data: &[u8],
        format: &CompressionFormat,
    ) -> ConversionResult<Vec<u8>> {
        let start_time = std::time::Instant::now();
        trace!("Decompressing {} bytes using {:?}", data.len(), format);

        let result = match format {
            CompressionFormat::None => data.to_vec(),
            CompressionFormat::ZSTD { level: _ } => self.decompress_zstd_pooled(data).await?,
            CompressionFormat::LZ4 => self.decompress_lz4_pooled(data).await?,
            CompressionFormat::Snappy => self.decompress_snappy_pooled(data).await?,
            CompressionFormat::Custom { algorithm, .. } => {
                warn!(
                    "Custom compression algorithm '{}' not implemented",
                    algorithm
                );
                return Err(ConversionError::CompressionError(format!(
                    "Unsupported compression algorithm: {}",
                    algorithm
                )));
            }
        };

        let decompression_time = start_time.elapsed().as_micros() as u64;
        debug!("Decompression completed in {}μs", decompression_time);
        Ok(result)
    }

    pub fn get_statistics(&self) -> &ConversionStatistics {
        &self.conversion_stats
    }

    async fn fp32_to_int8(&self, data: &[u8]) -> ConversionResult<Vec<i8>> {
        let float_data = self.bytes_to_fp32(data)?;
        let min_val = float_data.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_val = float_data.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let scale = if max_val > min_val {
            (max_val - min_val) / 255.0
        } else {
            1.0
        };
        let zero_point = (-min_val / scale).round() as i32 - 128;
        self.fp32_to_int8_with_params(&float_data, scale, zero_point)
    }

    fn fp32_to_int8_with_params(
        &self,
        data: &[f32],
        scale: f32,
        zero_point: i32,
    ) -> ConversionResult<Vec<i8>> {
        let result = data
            .iter()
            .map(|&value| {
                let quantized = (value / scale + zero_point as f32).round();
                quantized.clamp(-128.0, 127.0) as i8
            })
            .collect();
        Ok(result)
    }

    fn int8_to_fp32(&self, data: &[i8], scale: f32, zero_point: i32) -> ConversionResult<Vec<f32>> {
        let result = data
            .iter()
            .map(|&value| (value as f32 - zero_point as f32) * scale)
            .collect();
        Ok(result)
    }

    async fn quantize_to_pq(
        &self,
        data: &[f32],
        segments: usize,
        bits: usize,
    ) -> ConversionResult<Vec<u8>> {
        if !data.len().is_multiple_of(segments) {
            return Err(ConversionError::InvalidParameters(format!(
                "Vector dimension {} not divisible by segments {}",
                data.len(),
                segments
            )));
        }

        let segment_size = data.len() / segments;
        let centroids_per_segment = 1 << bits;
        let mut result = Vec::with_capacity(segments);

        for segment_idx in 0..segments {
            let segment_start = segment_idx * segment_size;
            let segment_end = segment_start + segment_size;
            let segment = &data[segment_start..segment_end];
            let mean = segment.iter().sum::<f32>() / segment.len() as f32;
            let quantized_index = ((mean + 1.0) / 2.0 * centroids_per_segment as f32)
                .clamp(0.0, (centroids_per_segment - 1) as f32)
                as u8;
            result.push(quantized_index);
        }

        Ok(result)
    }

    async fn quantize_to_binary(&self, data: &[f32], threshold: f32) -> ConversionResult<Vec<u8>> {
        let mut result = Vec::new();
        for chunk in data.chunks(8) {
            let mut byte = 0u8;
            for (i, &value) in chunk.iter().enumerate() {
                if value > threshold {
                    byte |= 1 << i;
                }
            }
            result.push(byte);
        }
        Ok(result)
    }

    fn bytes_to_fp32(&self, data: &[u8]) -> ConversionResult<Vec<f32>> {
        if !data.len().is_multiple_of(4) {
            return Err(ConversionError::DataSizeMismatch {
                expected: data.len() - (data.len() % 4),
                actual: data.len(),
            });
        }
        let mut result = Vec::new();
        for chunk in data.chunks(4) {
            let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
            result.push(f32::from_le_bytes(bytes));
        }
        Ok(result)
    }

    fn fp32_to_bytes(&self, data: &[f32]) -> ConversionResult<Vec<u8>> {
        let mut result = Vec::new();
        for &value in data {
            result.extend_from_slice(&value.to_le_bytes());
        }
        Ok(result)
    }

    async fn compress_zstd_pooled(&self, data: &[u8], _level: i32) -> ConversionResult<Vec<u8>> {
        let mut pooled_buffer = self.memory_pool.compression_buffers.acquire();
        let buffer = &mut *pooled_buffer;
        buffer.clear();
        buffer.reserve(data.len());
        buffer.extend_from_slice(data);
        Ok(buffer.clone())
    }

    async fn decompress_zstd_pooled(&self, data: &[u8]) -> ConversionResult<Vec<u8>> {
        let mut pooled_buffer = self.memory_pool.compression_buffers.acquire();
        let buffer = &mut *pooled_buffer;
        buffer.clear();
        buffer.reserve(data.len() * 2);
        buffer.extend_from_slice(data);
        Ok(buffer.clone())
    }

    async fn compress_lz4_pooled(&self, data: &[u8]) -> ConversionResult<Vec<u8>> {
        let mut pooled_buffer = self.memory_pool.compression_buffers.acquire();
        let buffer = &mut *pooled_buffer;
        buffer.clear();
        buffer.reserve(data.len());
        buffer.extend_from_slice(data);
        Ok(buffer.clone())
    }

    async fn decompress_lz4_pooled(&self, data: &[u8]) -> ConversionResult<Vec<u8>> {
        let mut pooled_buffer = self.memory_pool.compression_buffers.acquire();
        let buffer = &mut *pooled_buffer;
        buffer.clear();
        buffer.reserve(data.len() * 2);
        buffer.extend_from_slice(data);
        Ok(buffer.clone())
    }

    async fn compress_snappy_pooled(&self, data: &[u8]) -> ConversionResult<Vec<u8>> {
        let mut pooled_buffer = self.memory_pool.compression_buffers.acquire();
        let buffer = &mut *pooled_buffer;
        buffer.clear();
        buffer.reserve(data.len());
        buffer.extend_from_slice(data);
        Ok(buffer.clone())
    }

    async fn decompress_snappy_pooled(&self, data: &[u8]) -> ConversionResult<Vec<u8>> {
        let mut pooled_buffer = self.memory_pool.compression_buffers.acquire();
        let buffer = &mut *pooled_buffer;
        buffer.clear();
        buffer.reserve(data.len() * 2);
        buffer.extend_from_slice(data);
        Ok(buffer.clone())
    }
}

impl StorageFormat {
    pub fn data_size_per_vector(&self, dimension: usize) -> usize {
        match self {
            StorageFormat::FP32 => dimension * 4,
            StorageFormat::FP16 => dimension * 2,
            StorageFormat::QuantizedINT8 { .. } => dimension,
            StorageFormat::QuantizedPQ { segments, .. } => *segments,
            StorageFormat::Binary => dimension.div_ceil(8),
            StorageFormat::Custom { .. } => dimension * 4,
        }
    }

    pub fn supports_hardware_acceleration(&self) -> bool {
        matches!(
            self,
            StorageFormat::FP32 | StorageFormat::QuantizedINT8 { .. } | StorageFormat::Binary
        )
    }
}
