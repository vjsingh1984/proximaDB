# `compute` Module Review Report

## Identified Issues

### Tech Debt / Feature Gaps (TODOs)

The following `// TODO:` comments indicate areas for future work, potential tech debt, or feature gaps:

*   **File:** `quantization/unified.rs`
    *   **Line 1247:** `// TODO: Implement sync dequantization for other types`
*   **File:** `quantization/storage_engine.rs`
    *   **Line 251:** `// TODO: Implement codebook caching when get_codebook_store is available`
    *   **Line 569:** `// TODO: Implement proper PQ distance calculation with lookup tables`
*   **File:** `quantization/hardware_accelerated.rs`
    *   **Line 90:** `// TODO: Add SIMD implementation for u16`
    *   **Line 551:** `// TODO: Implement CUDA quantization`
    *   **Line 558:** `// TODO: Implement ROCm quantization`
    *   **Line 565:** `// TODO: Implement Metal Performance Shaders quantization`
    *   **Line 572:** `// TODO: Implement OpenCL quantization`
*   **File:** `distance_computation/quantized.rs`
    *   **Line 676:** `cache_hits: 0, // TODO: Aggregate from stages`
