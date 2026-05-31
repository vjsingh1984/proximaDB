//! TurboQuant error types.
//!
//! Per `TURBOQUANT_LLD_2026_05_30.adoc` §"Error Model": all production-path
//! errors are typed and propagated as `Result<_, TurboQuantError>`. Boundary
//! callers (e.g. the unified quantization engine) convert these into
//! `ProximaDBError` for the API surface. NO `unwrap`/`expect`/`panic` in
//! production code, per the clippy lints in `src/lib.rs:22-31`.

use thiserror::Error;

/// All errors that the TurboQuant pipeline can return at the boundary.
///
/// Variant choice is locked in LLD §"Error Model"; future variants should
/// be additive and surfaced in EXPLAIN as `rejected_route` reasons per
/// ADR-021.
#[derive(Debug, Error, PartialEq)]
pub enum TurboQuantError {
    /// Dimension is zero or not a multiple of 8. The kernel layout (P4) packs
    /// 8 codes per byte at 1-bit; multiples of 8 keep that math exact.
    #[error("dim {0} is not a positive multiple of 8")]
    DimNotMultipleOf8(usize),

    /// Bit-width outside {2, 3, 4}. P1 supports 2 and 4; 3 lands in P10 per
    /// LLD Q10.
    #[error("bit_width {0} not in {{2, 3, 4}}")]
    BitWidthOutOfRange(u8),

    /// Non-finite or oversized input. NaN / Inf / `|x| >= 1e16` would
    /// silently poison the encode pipeline (norm overflow, scale = Inf,
    /// LUT NaN propagation). Rejected at the boundary.
    #[error(
        "invalid input value at vector {vector_index}, coord {coord_index}: {value} \
         (must be finite and |value| < 1e16)"
    )]
    InvalidInputValue {
        vector_index: usize,
        coord_index: usize,
        value: f32,
    },

    /// Adding a batch with a different dim than the one the index was
    /// constructed with. Distinct from `DimNotMultipleOf8` (which is about
    /// the absolute value of the new dim).
    #[error("dim mismatch: existing dim={existing}, batch dim={got}")]
    DimMismatch { existing: usize, got: usize },

    /// `vectors.len()` is not a multiple of `dim`. Caller bug.
    #[error("vector buffer length {vectors_len} not a multiple of dim {dim}")]
    VectorBufferNotMultipleOfDim { vectors_len: usize, dim: usize },

    /// TQ+ calibration was requested but the index has not yet seen a batch
    /// of `TQPLUS_MIN_SAMPLES` (1000) vectors to fit it. Caller should
    /// either retry after more data is ingested or fall back to identity
    /// calibration. EXPLAIN surfaces `calibration_mode="identity"` in the
    /// fallback path.
    #[error("calibration not committed: index has fewer than {0} samples")]
    CalibrationNotCommitted(usize),

    /// Encoded codes have a different epoch tag than the collection's
    /// current epoch. Triggers re-encode from canonical `ProximaRecord` per
    /// ADR-021 `repair_source`.
    #[error("encoded epoch {encoded} does not match collection epoch {current}")]
    EpochMismatch { encoded: u64, current: u64 },

    /// On-disk `.tq` file is corrupted or version-incompatible (bad magic,
    /// version mismatch, length invariant violation). See LLD §3 file
    /// format validation rules.
    #[error("file format invalid: {0}")]
    InvalidFileFormat(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dim_not_multiple_of_8_message() {
        let e = TurboQuantError::DimNotMultipleOf8(7);
        assert_eq!(e.to_string(), "dim 7 is not a positive multiple of 8");
    }

    #[test]
    fn test_bit_width_out_of_range_message() {
        let e = TurboQuantError::BitWidthOutOfRange(5);
        assert_eq!(e.to_string(), "bit_width 5 not in {2, 3, 4}");
    }

    #[test]
    fn test_invalid_input_value_message_carries_indices() {
        let e = TurboQuantError::InvalidInputValue {
            vector_index: 3,
            coord_index: 42,
            value: f32::NAN,
        };
        // The exact NaN formatting is platform-dependent; assert the framing
        // around it is stable.
        let s = e.to_string();
        assert!(s.contains("vector 3"), "{s}");
        assert!(s.contains("coord 42"), "{s}");
        assert!(s.contains("NaN"), "{s}");
    }

    #[test]
    fn test_dim_mismatch_message() {
        let e = TurboQuantError::DimMismatch {
            existing: 1536,
            got: 768,
        };
        assert_eq!(
            e.to_string(),
            "dim mismatch: existing dim=1536, batch dim=768"
        );
    }

    #[test]
    fn test_calibration_not_committed_message() {
        let e = TurboQuantError::CalibrationNotCommitted(1000);
        assert_eq!(
            e.to_string(),
            "calibration not committed: index has fewer than 1000 samples"
        );
    }

    #[test]
    fn test_epoch_mismatch_message() {
        let e = TurboQuantError::EpochMismatch {
            encoded: 3,
            current: 5,
        };
        assert_eq!(
            e.to_string(),
            "encoded epoch 3 does not match collection epoch 5"
        );
    }
}
