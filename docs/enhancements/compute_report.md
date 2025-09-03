# `compute` Module Review Report - Feature Gaps

## Identified Feature Gaps

The following items indicate areas for future work or missing features:

*   **File:** `quantization/unified.rs`
    *   **Line 321:** `anyhow::bail!("Custom quantization not yet implemented")`
    *   **Line 1247:** `// TODO: Implement sync dequantization for other types`
*   **File:** `quantization/storage_engine.rs`
    *   **Line 251:** `// TODO: Implement codebook caching when get_codebook_store is available`
    *   **Line 569:** `// TODO: Implement proper PQ distance calculation with lookup tables`
*   **File:** `quantization/hardware_accelerated.rs`
    *   **Line 90:** `// TODO: Add SIMD implementation for u16`
    *   **Line 551:** `// TODO: Implement CUDA quantization`
    *   **Line 552:** `tracing::warn!("CUDA quantization not yet implemented, falling back to scalar");`
    *   **Line 558:** `// TODO: Implement ROCm quantization`
    *   **Line 559:** `tracing::warn!("ROCm quantization not yet implemented, falling back to scalar");`
    *   **Line 565:** `// TODO: Implement Metal Performance Shaders quantization`
    *   **Line 566:** `tracing::warn!("MPS quantization not yet implemented, falling back to scalar");`
    *   **Line 572:** `// TODO: Implement OpenCL quantization`
    *   **Line 573:** `tracing::warn!("OpenCL quantization not yet implemented, falling back to scalar");`
*   **File:** `distance_computation/quantized.rs`
    *   **Line 676:** `cache_hits: 0, // TODO: Aggregate from stages`