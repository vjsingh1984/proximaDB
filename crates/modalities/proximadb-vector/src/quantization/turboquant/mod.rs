//! TurboQuant data-oblivious vector quantizer (ADR-021).
//!
//! Paper-driven reimplementation of TurboQuant (arXiv:2504.19874) plus the
//! TQ+ per-coordinate calibration and the RaBitQ-style length renormalization
//! correction (arXiv:2405.12497). NOT a derivative of the MIT `turbovec`
//! reference implementation — `turbovec` is used as a cross-validation
//! artifact only (see `docs/12-design/TURBOQUANT_HLD_2026_05_30.adoc`).
//!
//! # Module layout
//!
//! - [`error`] — typed error enum returned by every boundary entry point.
//! - [`rotation`] — seeded random orthogonal matrix construction via QR
//!   (P2.3). Per-collection seed lives in xCatalog (P8).
//! - [`codebook`] — Lloyd-Max scalar quantizer for the Beta((d-1)/2, (d-1)/2)
//!   marginal that rotated unit-vector coordinates follow (P2.4).
//! - [`encode`] — normalize → rotate → quantize → bit-pack (P2.5). TQ+
//!   calibration and per-vector length-renorm scale land in P3.
//!
//! # Phase coverage
//!
//! This file's submodules cover P2 (encode pipeline). P3 adds calibration,
//! P4 adds the SIMD scoring kernels, P5–P9 cover allowlist filtering, AXIS
//! integration, observability, and benchmark validation. The
//! `Implementation Status` table in
//! `docs/12-design/TURBOQUANT_LLD_2026_05_30.adoc` is the live tracker.

pub mod calibration;
pub mod codebook;
pub mod encode;
pub mod error;
pub mod id_map;
pub mod io;
pub mod kernel;
pub mod mask;
pub mod rotation;
pub mod store;

pub use calibration::{Calibration, TQPLUS_MIN_SAMPLES, fit_calibration};
pub use error::TurboQuantError;
pub use id_map::{IdMapIndex, IdSearchHit};
pub use io::{MAGIC, PersistedStore, VERSION};
pub use kernel::{SearchHit, search};
pub use mask::{
    BLOCKS_SKIPPED_BY_MASK, block_has_allowed, blocks_skipped_by_mask, mask_allows,
    reset_blocks_skipped_by_mask,
};
pub use store::{StoreStats, TurboQuantStore};

/// Vector dimension multiplier required by the bit-packing layout: every
/// coordinate is packed into a byte at 1-bit (in the bit-plane stage), so
/// dim must be a multiple of 8. Mirrors LLD §"Algorithm Constants".
pub(crate) const DIM_ALIGNMENT: usize = 8;

/// Bit-widths supported in P1+P2. 3-bit is gated on P10 per LLD Q10.
pub(crate) const SUPPORTED_BIT_WIDTHS: &[u8] = &[2, 4];

/// Maximum permitted input coordinate magnitude. Beyond this, an f32
/// sum-of-squares norm can overflow to +Inf for any realistic dim. See LLD
/// §"Algorithm Constants" — the bound leaves a ~7x safety margin against
/// any realistic embedding value.
pub(crate) const MAX_INPUT_MAGNITUDE: f32 = 1e16;

/// Validate that a dim is supported by the kernel layout. Lifts the check
/// into one place so every entry point uses the same rejection reason.
pub(crate) fn check_dim(dim: usize) -> Result<(), TurboQuantError> {
    if dim == 0 || dim % DIM_ALIGNMENT != 0 {
        return Err(TurboQuantError::DimNotMultipleOf8(dim));
    }
    Ok(())
}

/// Validate that a bit-width is supported by the current phase. Per LLD
/// Q10, 3-bit is deferred — accepted in the type but rejected here until
/// P10 lands.
pub(crate) fn check_bit_width(bit_width: u8) -> Result<(), TurboQuantError> {
    if !SUPPORTED_BIT_WIDTHS.contains(&bit_width) {
        return Err(TurboQuantError::BitWidthOutOfRange(bit_width));
    }
    Ok(())
}

/// Walk a coordinate buffer once and return the position of the first
/// non-finite or out-of-range value, if any. Matches the LLD §"Error Model"
/// contract: `add()`/`search()` reject NaN/Inf/|x| >= 1e16 at the boundary
/// with a typed error (no panic).
///
/// `values.len()` is expected to be a multiple of `dim`; the caller is
/// responsible for that check (`VectorBufferNotMultipleOfDim`).
pub(crate) fn first_invalid_coord(values: &[f32], dim: usize) -> Option<(usize, usize, f32)> {
    for (i, &x) in values.iter().enumerate() {
        if !x.is_finite() || x.abs() >= MAX_INPUT_MAGNITUDE {
            let vector_index = if dim == 0 { 0 } else { i / dim };
            let coord_index = if dim == 0 { i } else { i % dim };
            return Some((vector_index, coord_index, x));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_dim_accepts_multiples_of_8() {
        assert!(check_dim(8).is_ok());
        assert!(check_dim(1536).is_ok());
        assert!(check_dim(3072).is_ok());
    }

    #[test]
    fn check_dim_rejects_zero_and_misaligned() {
        assert_eq!(check_dim(0), Err(TurboQuantError::DimNotMultipleOf8(0)));
        assert_eq!(check_dim(7), Err(TurboQuantError::DimNotMultipleOf8(7)));
        assert_eq!(
            check_dim(1535),
            Err(TurboQuantError::DimNotMultipleOf8(1535))
        );
    }

    #[test]
    fn check_bit_width_accepts_2_and_4() {
        assert!(check_bit_width(2).is_ok());
        assert!(check_bit_width(4).is_ok());
    }

    #[test]
    fn check_bit_width_rejects_3_until_p10() {
        // Per LLD Q10: 3-bit is deferred. When P10 lands, update
        // SUPPORTED_BIT_WIDTHS and this test together.
        assert_eq!(
            check_bit_width(3),
            Err(TurboQuantError::BitWidthOutOfRange(3))
        );
    }

    #[test]
    fn check_bit_width_rejects_out_of_range() {
        assert_eq!(
            check_bit_width(0),
            Err(TurboQuantError::BitWidthOutOfRange(0))
        );
        assert_eq!(
            check_bit_width(1),
            Err(TurboQuantError::BitWidthOutOfRange(1))
        );
        assert_eq!(
            check_bit_width(8),
            Err(TurboQuantError::BitWidthOutOfRange(8))
        );
    }

    #[test]
    fn first_invalid_coord_accepts_clean_input() {
        let v = vec![0.0f32, 0.5, -0.5, 1.0, -1.0, 1e15, -1e15, 0.001];
        assert!(first_invalid_coord(&v, 8).is_none());
    }

    #[test]
    fn first_invalid_coord_rejects_nan() {
        let mut v = vec![0.0f32; 16];
        v[10] = f32::NAN;
        let (vi, ci, val) = first_invalid_coord(&v, 8).unwrap();
        assert_eq!(vi, 1);
        assert_eq!(ci, 2);
        assert!(val.is_nan());
    }

    #[test]
    fn first_invalid_coord_rejects_inf() {
        let mut v = vec![0.0f32; 16];
        v[5] = f32::INFINITY;
        let (vi, ci, val) = first_invalid_coord(&v, 8).unwrap();
        assert_eq!(vi, 0);
        assert_eq!(ci, 5);
        assert_eq!(val, f32::INFINITY);
    }

    #[test]
    fn first_invalid_coord_rejects_neg_inf() {
        let mut v = vec![0.0f32; 16];
        v[15] = f32::NEG_INFINITY;
        let (vi, ci, val) = first_invalid_coord(&v, 8).unwrap();
        assert_eq!(vi, 1);
        assert_eq!(ci, 7);
        assert_eq!(val, f32::NEG_INFINITY);
    }

    #[test]
    fn first_invalid_coord_rejects_oversized() {
        let mut v = vec![0.0f32; 16];
        v[3] = 2e16; // beyond MAX_INPUT_MAGNITUDE
        let (vi, ci, val) = first_invalid_coord(&v, 8).unwrap();
        assert_eq!(vi, 0);
        assert_eq!(ci, 3);
        assert_eq!(val, 2e16);
    }

    #[test]
    fn first_invalid_coord_returns_first_occurrence_only() {
        let mut v = vec![0.0f32; 24];
        v[5] = f32::NAN;
        v[10] = f32::INFINITY;
        v[15] = 2e16;
        let (vi, ci, _) = first_invalid_coord(&v, 8).unwrap();
        assert_eq!(vi, 0);
        assert_eq!(ci, 5);
    }
}
