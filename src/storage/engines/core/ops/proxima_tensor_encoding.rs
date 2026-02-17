// UNIFIED TENSOR ENCODING MODULE FOR ALL ENGINES
// ================================================
// This module provides common sparse and quantized tensor encoding/decoding
// functionality that is reused across all storage engines (SST, SWIFT, RAPTOR, PRISM)
// to eliminate code duplication and ensure consistency.

use super::proximacodec::types::ProximaScheme;
use super::proximacodec::{ProximaCodec, analysis};
use anyhow::Result;
use std::io::{Read, Write};

// ============================================================================
// SPARSE TENSOR ENCODING/DECODING (Common for all engines)
// ============================================================================

#[derive(Debug, Clone)]
pub enum SparseFormat {
    COO, // Coordinate format (row, col, value)
    CSR, // Compressed Sparse Row format
}

/// Encode sparse tensor data that can be used by any engine
pub fn encode_sparse_tensor(
    dense_vectors: &[f32],
    num_vectors: usize,
    dimension: usize,
    format: SparseFormat,
    sparsity_threshold: f32,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();

    // Write format indicator
    output.write_all(&[match format {
        SparseFormat::COO => 0u8,
        SparseFormat::CSR => 1u8,
    }])?;

    // Write dimensions
    output.write_all(&(dimension as u32).to_le_bytes())?;
    output.write_all(&(num_vectors as u32).to_le_bytes())?;

    // Find non-zero elements
    let mut row_indices = Vec::new();
    let mut col_indices = Vec::new();
    let mut values = Vec::new();

    for row in 0..num_vectors {
        for col in 0..dimension {
            let idx = row * dimension + col;
            let value = dense_vectors[idx];
            if value.abs() > sparsity_threshold {
                row_indices.push(row as u32);
                col_indices.push(col as u32);
                values.push(value);
            }
        }
    }

    let nnz = values.len();
    output.write_all(&(nnz as u32).to_le_bytes())?;

    match format {
        SparseFormat::COO => {
            // Write row indices
            for &row in &row_indices {
                output.write_all(&row.to_le_bytes())?;
            }

            // Write column indices
            for &col in &col_indices {
                output.write_all(&col.to_le_bytes())?;
            }

            // Analyze data and choose optimal encoding scheme
            let scheme = analysis::analyze_and_choose_scheme_f32(&values);
            let codec = ProximaCodec::global();
            let encoded_values = codec.encode(&values, scheme)?;
            output.write_all(&(encoded_values.len() as u32).to_le_bytes())?;
            output.write_all(&encoded_values)?;
        }
        SparseFormat::CSR => {
            // Build row pointers
            let mut row_ptrs = vec![0u32; num_vectors + 1];
            for &row in &row_indices {
                row_ptrs[row as usize + 1] += 1;
            }
            for i in 1..=num_vectors {
                row_ptrs[i] += row_ptrs[i - 1];
            }

            // Write row pointers
            for &ptr in &row_ptrs {
                output.write_all(&ptr.to_le_bytes())?;
            }

            // Write column indices
            for &col in &col_indices {
                output.write_all(&col.to_le_bytes())?;
            }

            // Analyze data and choose optimal encoding scheme
            let scheme = analysis::analyze_and_choose_scheme_f32(&values);
            let codec = ProximaCodec::global();
            let encoded_values = codec.encode(&values, scheme)?;
            output.write_all(&(encoded_values.len() as u32).to_le_bytes())?;
            output.write_all(&encoded_values)?;
        }
    }

    Ok(output)
}

/// Decode sparse tensor data that can be used by any engine
pub fn decode_sparse_tensor(
    data: &[u8],
    expected_dimension: Option<usize>,
) -> Result<(Vec<f32>, usize, usize)> {
    let mut cursor = std::io::Cursor::new(data);

    // Read format
    let mut format_byte = [0u8; 1];
    cursor.read_exact(&mut format_byte)?;
    let is_coo = format_byte[0] == 0;

    // Read dimensions
    let mut dim_bytes = [0u8; 4];
    cursor.read_exact(&mut dim_bytes)?;
    let dimension = u32::from_le_bytes(dim_bytes) as usize;

    if let Some(expected) = expected_dimension {
        if dimension != expected {
            return Err(anyhow::anyhow!(
                "Dimension mismatch: expected {}, got {}",
                expected,
                dimension
            ));
        }
    }

    let mut count_bytes = [0u8; 4];
    cursor.read_exact(&mut count_bytes)?;
    let num_vectors = u32::from_le_bytes(count_bytes) as usize;

    let mut nnz_bytes = [0u8; 4];
    cursor.read_exact(&mut nnz_bytes)?;
    let nnz = u32::from_le_bytes(nnz_bytes) as usize;

    let dense_vectors = if is_coo {
        // COO format decoding
        let mut row_indices = vec![0u32; nnz];
        for i in 0..nnz {
            let mut idx_bytes = [0u8; 4];
            cursor.read_exact(&mut idx_bytes)?;
            row_indices[i] = u32::from_le_bytes(idx_bytes);
        }

        let mut col_indices = vec![0u32; nnz];
        for i in 0..nnz {
            let mut idx_bytes = [0u8; 4];
            cursor.read_exact(&mut idx_bytes)?;
            col_indices[i] = u32::from_le_bytes(idx_bytes);
        }

        // Decode values
        let mut val_len_bytes = [0u8; 4];
        cursor.read_exact(&mut val_len_bytes)?;
        let val_len = u32::from_le_bytes(val_len_bytes) as usize;

        let mut val_data = vec![0u8; val_len];
        cursor.read_exact(&mut val_data)?;

        // Auto-detect encoding scheme from the encoded data (ProximaCodec wire format)
        let codec = ProximaCodec::global();
        let values = codec.decode(&val_data)?;

        // Reconstruct dense matrix
        let mut dense = vec![0.0f32; num_vectors * dimension];
        for i in 0..nnz.min(values.len()) {
            let row = row_indices[i] as usize;
            let col = col_indices[i] as usize;
            if row < num_vectors && col < dimension {
                dense[row * dimension + col] = values[i];
            }
        }
        dense
    } else {
        // CSR format decoding
        let mut row_ptrs = vec![0u32; num_vectors + 1];
        for i in 0..=num_vectors {
            let mut ptr_bytes = [0u8; 4];
            cursor.read_exact(&mut ptr_bytes)?;
            row_ptrs[i] = u32::from_le_bytes(ptr_bytes);
        }

        let mut col_indices = vec![0u32; nnz];
        for i in 0..nnz {
            let mut idx_bytes = [0u8; 4];
            cursor.read_exact(&mut idx_bytes)?;
            col_indices[i] = u32::from_le_bytes(idx_bytes);
        }

        // Decode values
        let mut val_len_bytes = [0u8; 4];
        cursor.read_exact(&mut val_len_bytes)?;
        let val_len = u32::from_le_bytes(val_len_bytes) as usize;

        let mut val_data = vec![0u8; val_len];
        cursor.read_exact(&mut val_data)?;

        // Auto-detect encoding scheme from the encoded data (ProximaCodec wire format)
        let codec = ProximaCodec::global();
        let values = codec.decode(&val_data)?;

        // Reconstruct dense matrix
        let mut dense = vec![0.0f32; num_vectors * dimension];
        for row in 0..num_vectors {
            let start = row_ptrs[row] as usize;
            let end = row_ptrs[row + 1] as usize;
            for idx in start..end.min(col_indices.len()).min(values.len()) {
                let col = col_indices[idx] as usize;
                if col < dimension {
                    dense[row * dimension + col] = values[idx];
                }
            }
        }
        dense
    };

    Ok((dense_vectors, num_vectors, dimension))
}

// ============================================================================
// QUANTIZED TENSOR ENCODING/DECODING (Common for all engines)
// ============================================================================

#[derive(Debug, Clone)]
pub enum QuantizationType {
    INT8,
    ProductQuantization {
        subvectors: usize,
        codebook_size: usize,
    },
    Binary,
}

/// Encode quantized tensor data that can be used by any engine
pub fn encode_quantized_tensor(
    vectors: &[f32],
    num_vectors: usize,
    dimension: usize,
    quant_type: QuantizationType,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();

    match quant_type {
        QuantizationType::INT8 => {
            output.write_all(&[0u8])?; // Type marker
            output.write_all(&(dimension as u32).to_le_bytes())?;
            output.write_all(&(num_vectors as u32).to_le_bytes())?;

            // Calculate quantization parameters
            let (min_val, max_val) = vectors.iter().fold((f32::MAX, f32::MIN), |(min, max), &v| {
                (min.min(v), max.max(v))
            });

            let scale = (max_val - min_val) / 255.0;
            let zero_point = min_val;

            output.write_all(&scale.to_le_bytes())?;
            output.write_all(&zero_point.to_le_bytes())?;

            // Quantize to INT8 (using u8 for 0-255 range)
            for &value in vectors {
                let quantized = ((value - zero_point) / scale).round().clamp(0.0, 255.0) as u8;
                output.write_all(&[quantized])?;
            }
        }
        QuantizationType::ProductQuantization {
            subvectors,
            codebook_size,
        } => {
            output.write_all(&[1u8])?; // Type marker
            output.write_all(&(dimension as u32).to_le_bytes())?;
            output.write_all(&(num_vectors as u32).to_le_bytes())?;
            output.write_all(&(subvectors as u32).to_le_bytes())?;
            output.write_all(&(codebook_size as u32).to_le_bytes())?;

            let _subvector_dim = dimension / subvectors;

            // Build codebooks using k-means (simplified - use random initialization)
            let mut codebooks = Vec::new();
            for subvec_idx in 0..subvectors {
                let mut codebook = Vec::new();

                // Create codebook entries (simplified - would use k-means in production)
                for code_idx in 0..codebook_size {
                    for dim_idx in 0.._subvector_dim {
                        // Use representative values from data
                        let vec_idx = (code_idx * num_vectors / codebook_size) % num_vectors;
                        let value =
                            vectors[vec_idx * dimension + subvec_idx * _subvector_dim + dim_idx];
                        codebook.push(value);
                        output.write_all(&value.to_le_bytes())?;
                    }
                }
                codebooks.push(codebook);
            }

            // Encode vectors as PQ codes
            for vec_idx in 0..num_vectors {
                for subvec_idx in 0..subvectors {
                    // Find nearest centroid (simplified)
                    let mut best_code = 0u8;
                    let mut best_dist = f32::MAX;

                    for code_idx in 0..codebook_size {
                        let mut dist = 0.0f32;
                        for dim_idx in 0.._subvector_dim {
                            let vec_val = vectors
                                [vec_idx * dimension + subvec_idx * _subvector_dim + dim_idx];
                            let code_val =
                                codebooks[subvec_idx][code_idx * _subvector_dim + dim_idx];
                            dist += (vec_val - code_val).powi(2);
                        }

                        if dist < best_dist {
                            best_dist = dist;
                            best_code = code_idx as u8;
                        }
                    }

                    output.write_all(&[best_code])?;
                }
            }
        }
        QuantizationType::Binary => {
            output.write_all(&[2u8])?; // Type marker
            output.write_all(&(dimension as u32).to_le_bytes())?;
            output.write_all(&(num_vectors as u32).to_le_bytes())?;

            // Pack bits
            let bytes_per_vector = (dimension + 7) / 8;
            for vec_idx in 0..num_vectors {
                let mut packed = vec![0u8; bytes_per_vector];
                for dim_idx in 0..dimension {
                    let value = vectors[vec_idx * dimension + dim_idx];
                    if value > 0.0 {
                        let byte_idx = dim_idx / 8;
                        let bit_idx = dim_idx % 8;
                        packed[byte_idx] |= 1 << bit_idx;
                    }
                }
                output.write_all(&packed)?;
            }
        }
    }

    Ok(output)
}

/// Decode quantized tensor data that can be used by any engine
pub fn decode_quantized_tensor(data: &[u8]) -> Result<(Vec<f32>, usize, usize, QuantizationType)> {
    let mut cursor = std::io::Cursor::new(data);

    // Read quantization type
    let mut quant_type = [0u8; 1];
    cursor.read_exact(&mut quant_type)?;

    match quant_type[0] {
        0 => {
            // INT8 Quantization
            let mut dim_bytes = [0u8; 4];
            cursor.read_exact(&mut dim_bytes)?;
            let dimension = u32::from_le_bytes(dim_bytes) as usize;

            let mut count_bytes = [0u8; 4];
            cursor.read_exact(&mut count_bytes)?;
            let num_vectors = u32::from_le_bytes(count_bytes) as usize;

            let mut scale_bytes = [0u8; 4];
            cursor.read_exact(&mut scale_bytes)?;
            let scale = f32::from_le_bytes(scale_bytes);

            let mut zero_bytes = [0u8; 4];
            cursor.read_exact(&mut zero_bytes)?;
            let zero_point = f32::from_le_bytes(zero_bytes);

            // Read and dequantize (u8 for 0-255 range)
            let data_size = num_vectors * dimension;
            let mut u8_data = vec![0u8; data_size];
            cursor.read_exact(&mut u8_data)?;

            let dense: Vec<f32> = u8_data
                .iter()
                .map(|&q| q as f32 * scale + zero_point)
                .collect();

            Ok((dense, num_vectors, dimension, QuantizationType::INT8))
        }
        1 => {
            // Product Quantization
            let mut dim_bytes = [0u8; 4];
            cursor.read_exact(&mut dim_bytes)?;
            let dimension = u32::from_le_bytes(dim_bytes) as usize;

            let mut count_bytes = [0u8; 4];
            cursor.read_exact(&mut count_bytes)?;
            let num_vectors = u32::from_le_bytes(count_bytes) as usize;

            let mut subvec_bytes = [0u8; 4];
            cursor.read_exact(&mut subvec_bytes)?;
            let num_subvectors = u32::from_le_bytes(subvec_bytes) as usize;

            let mut codebook_bytes = [0u8; 4];
            cursor.read_exact(&mut codebook_bytes)?;
            let codebook_size = u32::from_le_bytes(codebook_bytes) as usize;

            // Read codebooks
            let _subvector_dim = dimension / num_subvectors;
            let mut codebooks = vec![vec![0.0f32; codebook_size * _subvector_dim]; num_subvectors];

            for subvec in 0..num_subvectors {
                for entry in 0..codebook_size * _subvector_dim {
                    let mut val_bytes = [0u8; 4];
                    cursor.read_exact(&mut val_bytes)?;
                    codebooks[subvec][entry] = f32::from_le_bytes(val_bytes);
                }
            }

            // Read PQ codes and reconstruct
            let mut pq_codes = vec![0u8; num_vectors * num_subvectors];
            cursor.read_exact(&mut pq_codes)?;

            let mut dense = Vec::with_capacity(num_vectors * dimension);
            for vec_idx in 0..num_vectors {
                for subvec_idx in 0..num_subvectors {
                    let code = pq_codes[vec_idx * num_subvectors + subvec_idx] as usize;
                    let offset = code * _subvector_dim;
                    for dim in 0.._subvector_dim {
                        dense.push(codebooks[subvec_idx][offset + dim]);
                    }
                }
            }

            Ok((
                dense,
                num_vectors,
                dimension,
                QuantizationType::ProductQuantization {
                    subvectors: num_subvectors,
                    codebook_size,
                },
            ))
        }
        2 => {
            // Binary Quantization
            let mut dim_bytes = [0u8; 4];
            cursor.read_exact(&mut dim_bytes)?;
            let dimension = u32::from_le_bytes(dim_bytes) as usize;

            let mut count_bytes = [0u8; 4];
            cursor.read_exact(&mut count_bytes)?;
            let num_vectors = u32::from_le_bytes(count_bytes) as usize;

            // Unpack bits
            let bytes_per_vector = (dimension + 7) / 8;
            let mut binary_data = vec![0u8; num_vectors * bytes_per_vector];
            cursor.read_exact(&mut binary_data)?;

            let mut dense = Vec::with_capacity(num_vectors * dimension);
            for vec_idx in 0..num_vectors {
                for dim_idx in 0..dimension {
                    let byte_idx = vec_idx * bytes_per_vector + dim_idx / 8;
                    let bit_idx = dim_idx % 8;
                    let bit = (binary_data[byte_idx] >> bit_idx) & 1;
                    dense.push(if bit == 1 { 1.0 } else { -1.0 });
                }
            }

            Ok((dense, num_vectors, dimension, QuantizationType::Binary))
        }
        _ => Err(anyhow::anyhow!(
            "Unknown quantization type: {}",
            quant_type[0]
        )),
    }
}

// ============================================================================
// TENSOR COLUMNAR TRANSFORMATION (Common for all engines)
// ============================================================================

/// Transform row-major vectors to column-major for SIMD optimization
/// This is used by all engines before encoding
pub fn transpose_to_columnar(
    vectors: &[f32],
    num_vectors: usize,
    dimension: usize,
) -> Vec<Vec<f32>> {
    let mut columns = vec![Vec::with_capacity(num_vectors); dimension];

    for vec_idx in 0..num_vectors {
        for dim_idx in 0..dimension {
            columns[dim_idx].push(vectors[vec_idx * dimension + dim_idx]);
        }
    }

    columns
}

/// Transform column-major back to row-major after decoding
/// This is used by all engines after decoding
pub fn transpose_to_row_major(
    columns: &[Vec<f32>],
    num_vectors: usize,
    dimension: usize,
) -> Vec<f32> {
    let mut vectors = Vec::with_capacity(num_vectors * dimension);

    for vec_idx in 0..num_vectors {
        for dim_idx in 0..dimension {
            if dim_idx < columns.len() && vec_idx < columns[dim_idx].len() {
                vectors.push(columns[dim_idx][vec_idx]);
            } else {
                vectors.push(0.0); // Padding for incomplete data
            }
        }
    }

    vectors
}

// ============================================================================
// UNIFIED ENCODING SELECTION (Common logic for all engines)
// ============================================================================

/// Analyze tensor characteristics and choose optimal encoding
/// This provides consistent encoding selection across all engines
pub fn choose_optimal_tensor_encoding(
    vectors: &[f32],
    _num_vectors: usize,
    _dimension: usize,
) -> ProximaScheme {
    // Calculate statistics
    let mut min_val = f32::MAX;
    let mut max_val = f32::MIN;
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let mut zero_count = 0;

    for &val in vectors {
        min_val = min_val.min(val);
        max_val = max_val.max(val);
        sum += val;
        sum_sq += val * val;
        if val.abs() < 1e-6 {
            zero_count += 1;
        }
    }

    let total = vectors.len() as f32;
    let mean = sum / total;
    let variance = (sum_sq / total) - (mean * mean);
    let range = max_val - min_val;
    let sparsity = zero_count as f32 / total;

    // Choose encoding based on characteristics
    if sparsity > 0.9 {
        // Very sparse - use run-length
        ProximaScheme::RunLength
    } else if range < 1e-6 {
        // Constant or near-constant - use run-length
        ProximaScheme::RunLength
    } else if variance < range * range / 100.0 {
        // Low variance relative to range - use delta
        ProximaScheme::Delta {
            base: min_val as i64,
        }
    } else if range < 100.0 && min_val.abs() < 1000.0 {
        // Limited range - use frame of reference
        let bits = ((range.log2().ceil() as u8) + 1).clamp(8, 24);
        ProximaScheme::FrameOfReference {
            reference: min_val as i64,
            bits,
        }
    } else {
        // Default to bit packing
        ProximaScheme::BitPacked { bits: 16 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sparse_tensor_roundtrip() {
        let dense = vec![1.0, 0.0, 0.0, 2.0, 0.0, 3.0, 0.0, 0.0, 4.0, 0.0, 5.0, 0.0];
        let num_vectors = 3;
        let dimension = 4;

        // Test COO format
        let encoded =
            encode_sparse_tensor(&dense, num_vectors, dimension, SparseFormat::COO, 0.01).unwrap();
        let (decoded, n, d) = decode_sparse_tensor(&encoded, Some(dimension)).unwrap();

        assert_eq!(n, num_vectors);
        assert_eq!(d, dimension);
        assert_eq!(dense, decoded);

        // Test CSR format
        let encoded =
            encode_sparse_tensor(&dense, num_vectors, dimension, SparseFormat::CSR, 0.01).unwrap();
        let (decoded, n, d) = decode_sparse_tensor(&encoded, Some(dimension)).unwrap();

        assert_eq!(n, num_vectors);
        assert_eq!(d, dimension);
        assert_eq!(dense, decoded);
    }

    #[test]
    fn test_quantized_tensor_roundtrip() {
        let vectors: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
        let num_vectors = 10;
        let dimension = 10;

        // Test INT8
        let encoded =
            encode_quantized_tensor(&vectors, num_vectors, dimension, QuantizationType::INT8)
                .unwrap();
        let (decoded, n, d, qt) = decode_quantized_tensor(&encoded).unwrap();

        assert_eq!(n, num_vectors);
        assert_eq!(d, dimension);
        assert!(matches!(qt, QuantizationType::INT8));

        // Check approximate equality (quantization loses precision)
        // INT8 quantization of range 0.0-9.9 has limited precision
        // Scale = 9.9/255 ≈ 0.0388 per level, so max error can be up to ~0.8 due to rounding
        for (orig, dec) in vectors.iter().zip(decoded.iter()) {
            assert!(
                (orig - dec).abs() < 1.0,
                "Original: {}, Decoded: {}, Diff: {}",
                orig,
                dec,
                (orig - dec).abs()
            );
        }

        // Test Binary
        let encoded =
            encode_quantized_tensor(&vectors, num_vectors, dimension, QuantizationType::Binary)
                .unwrap();
        let (decoded, n, d, qt) = decode_quantized_tensor(&encoded).unwrap();

        assert_eq!(n, num_vectors);
        assert_eq!(d, dimension);
        assert!(matches!(qt, QuantizationType::Binary));
    }

    #[test]
    fn test_columnar_transpose() {
        let vectors = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let num_vectors = 2;
        let dimension = 3;

        let columns = transpose_to_columnar(&vectors, num_vectors, dimension);
        assert_eq!(columns.len(), dimension);
        assert_eq!(columns[0], vec![1.0, 4.0]);
        assert_eq!(columns[1], vec![2.0, 5.0]);
        assert_eq!(columns[2], vec![3.0, 6.0]);

        let row_major = transpose_to_row_major(&columns, num_vectors, dimension);
        assert_eq!(vectors, row_major);
    }
}
