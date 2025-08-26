# Quantization Guide for ProximaDB

## Overview

ProximaDB supports multiple quantization levels for vector and distance compression, allowing you to trade off between accuracy and storage/memory requirements. This guide explains the available quantization methods and their characteristics.

## Quantization Methods

### 4-bit Quantization (u4/q4)
- **Compression Ratio**: 50% of 8-bit (0.5 bytes per value)
- **Packing**: 2 values per byte
- **Quantization Levels**: 16 (0-15)
- **Maximum Error**: ~6.67% of value range
- **Use Case**: Aggressive compression when accuracy can be sacrificed

### 6-bit Quantization (u6/q6)
- **Compression Ratio**: 75% of 8-bit (0.75 bytes per value)
- **Packing**: 4 values per 3 bytes
- **Quantization Levels**: 64 (0-63)
- **Maximum Error**: ~1.59% of value range
- **Use Case**: Balanced compression with reasonable accuracy

### 8-bit Quantization (u8/q8)
- **Compression Ratio**: Baseline (1 byte per value)
- **Packing**: 1 value per byte
- **Quantization Levels**: 256 (0-255)
- **Maximum Error**: ~0.39% of value range
- **Use Case**: Standard quantization for most applications

### 16-bit Quantization (u16/q16)
- **Compression Ratio**: 200% of 8-bit (2 bytes per value)
- **Packing**: 1 value per 2 bytes
- **Quantization Levels**: 65536 (0-65535)
- **Maximum Error**: ~0.0015% of value range
- **Use Case**: High accuracy when storage is less critical

## Storage Format

All quantized formats store the following parameters for accurate reconstruction:

```
[min: 4 bytes][max: 4 bytes][num_values: 4 bytes*][packed_data]
```
*Note: num_values is only stored for 4-bit and 6-bit formats

## Accuracy Analysis

### Mean Squared Error (MSE) Comparison

Based on empirical testing with distance values in range [0.1, 15.5]:

| Bit Width | MSE | Max Error | Storage Size | Relative Size |
|-----------|-----|-----------|--------------|---------------|
| 4-bit | ~0.25 | ~1.0 | 16B for 32 values | 50% |
| 6-bit | ~0.04 | ~0.25 | 24B for 32 values | 75% |
| 8-bit | ~0.01 | ~0.06 | 32B for 32 values | 100% |
| 16-bit | <0.0001 | ~0.0002 | 64B for 32 values | 200% |

### Reconstruction Formula

For all quantization levels:
```
original_value ≈ min + (quantized_value / max_quantized) * (max - min)
```

Where:
- `min`: Minimum value in original data
- `max`: Maximum value in original data
- `quantized_value`: The quantized integer value
- `max_quantized`: Maximum quantized value (15 for 4-bit, 63 for 6-bit, 255 for 8-bit, 65535 for 16-bit)

## Usage in ProximaDB

### Configuration

In RAPTOR engine configuration:

```rust
use crate::storage::engines::raptor::config::CompressionStrategy;

// For maximum compression (50% size)
CompressionStrategy::Quantized4

// For balanced compression (75% size)
CompressionStrategy::Quantized6

// For standard compression (100% size)
CompressionStrategy::Quantized8

// For high precision (200% size)
CompressionStrategy::Float16
```

### API Usage

```rust
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;

let engine = StorageQuantizationEngine::new_default();

// Quantize distances
let distances = vec![0.5, 1.0, 1.5, 2.0];

// 4-bit quantization
let (packed, min, max, num_values) = engine.quantize_to_u4(&distances);
let reconstructed = engine.dequantize_u4(&packed, min, max, num_values);

// 6-bit quantization
let (packed, min, max, num_values) = engine.quantize_to_u6(&distances);
let reconstructed = engine.dequantize_u6(&packed, min, max, num_values);

// 8-bit quantization
let (quantized, min, max) = engine.quantize_to_u8(&distances);
let reconstructed = engine.dequantize_u8(&quantized, min, max);

// 16-bit quantization
let (quantized, min, max) = engine.quantize_to_u16(&distances);
let reconstructed = engine.dequantize_u16(&quantized, min, max);
```

## Recommendations

### When to Use Each Level

1. **4-bit (q4)**:
   - Extreme storage constraints
   - Approximate nearest neighbor is sufficient
   - Large-scale filtering before reranking
   - Acceptable error: ~6-7% of range

2. **6-bit (q6)**:
   - Good balance of size and accuracy
   - Production search systems with reranking
   - Acceptable error: ~1-2% of range

3. **8-bit (q8)**:
   - Standard choice for most applications
   - Good accuracy with 4x compression vs float32
   - Acceptable error: <0.5% of range

4. **16-bit (q16)**:
   - Scientific/research applications
   - Financial data requiring high precision
   - When accuracy is critical
   - Acceptable error: <0.002% of range

### Performance Considerations

- **Memory Bandwidth**: Lower bit widths reduce memory transfer
- **Cache Efficiency**: More values fit in CPU cache with lower bits
- **SIMD Operations**: 4-bit and 8-bit align well with SIMD instructions
- **Decompression Overhead**: 6-bit has slightly higher overhead due to complex packing

## Edge Cases

The quantization handles these edge cases correctly:

1. **All Same Values**: When min == max, all values quantize to the same value and reconstruct correctly
2. **Empty Arrays**: Returns empty results with infinite min and negative infinite max
3. **Single Value**: Properly handled with appropriate packing
4. **Negative Values**: Full range supported including negative values
5. **Odd Counts**: 4-bit and 6-bit handle non-aligned counts properly

## Future Enhancements

Potential improvements under consideration:

1. **Adaptive Quantization**: Automatically select bit width based on data distribution
2. **Non-uniform Quantization**: Use logarithmic or custom quantization for skewed distributions
3. **Vector Quantization**: Quantize groups of values together for better accuracy
4. **Learned Quantization**: Use machine learning to optimize quantization boundaries