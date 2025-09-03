# `infrastructure` Module Review Report - Feature Gaps

## Identified Feature Gaps

The following items indicate areas for future work or missing features:

*   **File:** `tier_data_movement.rs`
    *   **Line 34:** `// Temporarily disabled due to arrow-arith compilation conflicts - TODO: Re-enable when resolved`
    *   **Line 36:** `// use crate::storage::engines::impls::viper::flush::ViperFlushOperation; // TODO: Import correct flush module`
    *   **Line 322:** `// TODO: Re-enable when UnifiedParquetReader is available`
    *   **Line 326:** `// TODO: Implement VIPER reading when UnifiedParquetReader is restored`
    *   **Line 348:** `// TODO: Use VIPER flush operation to write Parquet`
*   **File:** `adaptive_structures.rs`
    *   **Line 1353:** `#[ignore] // TODO: Fix test - IndexBackend API has changed`
    *   **Line 1387:** `#[ignore] // TODO: Fix test - API has changed`
    *   **Line 1431:** `#[ignore] // TODO: Fix test - API has changed`
    *   **Line 1466:** `#[ignore] // TODO: Fix test - API has changed`
    *   **Line 1501:** `#[ignore] // TODO: Fix test - API has changed`