//! Lloyd-Max scalar quantizer for the Beta((d-1)/2, (d-1)/2) marginal.
//!
//! After multiplication by a random orthogonal matrix, each coordinate of a
//! unit vector on the (d-1)-sphere follows Beta((d-1)/2, (d-1)/2) shifted
//! to `[-1, 1]`. The Lloyd-Max algorithm finds the scalar quantizer that
//! minimises mean squared error against that *theoretical* marginal — no
//! data, no training, no codebook drift.
//!
//! ## Outputs
//!
//! - `boundaries`: `2^bits - 1` cut points on `[-1, 1]`. A value `x` is
//!   quantized to code `c` where `c` is the number of boundaries strictly
//!   less than `x`.
//! - `centroids`: `2^bits` reconstruction values, one per code.
//!
//! Both are returned as `f32` for direct use by the encode pipeline; the
//! Lloyd iteration runs in `f64` so the Simpson-rule centroid integrals
//! stay numerically stable for `dim` up to a few thousand.
//!
//! ## Lloyd iteration
//!
//! Starting from centroids spread over ±3 standard deviations of the
//! marginal:
//!
//! 1. Boundaries := midpoints of consecutive centroids.
//! 2. For each cell `[lo, hi]`, new centroid := conditional mean
//!    `∫ x · f(x) dx / Pr[lo < X <= hi]` evaluated against the Beta PDF.
//! 3. Repeat until the max centroid shift is below `tol`, or `max_iter`
//!    iterations have elapsed.
//!
//! The conditional-mean integral is computed with adaptive Simpson's rule
//! to keep evaluation cost bounded without sacrificing accuracy near the
//! distribution tails.

use statrs::distribution::{Beta, Continuous, ContinuousCDF};

/// Lloyd-Max iteration budget. Hits convergence in typically 30–60 steps
/// for `bits ∈ {2, 4}`; 200 is a safe ceiling.
const MAX_ITERATIONS: usize = 200;

/// Convergence threshold on the max per-iteration centroid shift in
/// `f64` space.
const CONVERGENCE_TOLERANCE: f64 = 1e-12;

/// Compute `(boundaries, centroids)` for the given `bits` and `dim`.
///
/// Both vectors are deterministic functions of `(bits, dim)` — no random
/// seed, no input data. Callers can cache the result per `(bits, dim)` and
/// reuse it across encode and search.
///
/// # Panics
///
/// Does not panic for any `dim >= 2` and `bits ∈ {2, 3, 4}`. (`dim == 1`
/// degenerates because the Beta((0)/2, (0)/2) is not defined; callers
/// should validate dim via [`super::check_dim`] first — which rejects 0
/// and non-multiples-of-8.)
pub fn codebook(bits: usize, dim: usize) -> (Vec<f32>, Vec<f32>) {
    lloyd_max(bits, dim, MAX_ITERATIONS, CONVERGENCE_TOLERANCE)
}

fn lloyd_max(bits: usize, dim: usize, max_iter: usize, tol: f64) -> (Vec<f32>, Vec<f32>) {
    let a = (dim as f64 - 1.0) / 2.0;
    // Beta(a, a) lives on [0, 1]; we shift to [-1, 1] via `x ↦ (x + 1) / 2`
    // in every CDF / PDF evaluation below.
    let beta = Beta::new(a, a).expect("Beta(a, a) is valid for any a > 0 and dim >= 2");
    let n_levels = 1usize << bits;

    // Initial centroids: equally spaced on [-spread, +spread] where spread
    // is 3 standard deviations of the Beta marginal shifted to [-1, 1].
    let var_unit = a / ((2.0 * a + 1.0) * 4.0 * a); // var of Beta(a, a) on [0, 1]
    let std_shifted = (4.0 * var_unit).sqrt(); // std on [-1, 1] = 2 * std on [0, 1]
    let spread = 3.0 * std_shifted;
    let mut centroids: Vec<f64> = (0..n_levels)
        .map(|i| {
            if n_levels == 1 {
                0.0
            } else {
                -spread + 2.0 * spread * (i as f64) / ((n_levels - 1) as f64)
            }
        })
        .collect();

    for _ in 0..max_iter {
        // Step 1: boundaries are midpoints between consecutive centroids.
        let boundaries: Vec<f64> = (0..n_levels - 1)
            .map(|i| (centroids[i] + centroids[i + 1]) / 2.0)
            .collect();

        // Cell edges sandwich `boundaries` with the marginal's full support.
        let mut edges = Vec::with_capacity(n_levels + 1);
        edges.push(-1.0);
        edges.extend_from_slice(&boundaries);
        edges.push(1.0);

        // Step 2: for each cell, integrate `x * pdf(x)` against the Beta
        // PDF on [-1, 1] and divide by the cell's probability mass to get
        // the conditional mean — the optimal centroid for that cell.
        let mut new_centroids = vec![0.0f64; n_levels];
        for i in 0..n_levels {
            let lo = edges[i];
            let hi = edges[i + 1];

            // Probability mass via the Beta CDF on [0, 1].
            let cdf_lo = beta.cdf((lo + 1.0) / 2.0);
            let cdf_hi = beta.cdf((hi + 1.0) / 2.0);
            let prob = cdf_hi - cdf_lo;

            if prob < 1e-15 {
                // Numerically empty cell; freeze the centroid where it is.
                new_centroids[i] = centroids[i];
            } else {
                // E[x | x in [lo, hi]] = (1/prob) * ∫_{lo}^{hi} x * pdf(x) dx.
                // pdf on [-1, 1] is `beta.pdf((x + 1) / 2) / 2` (Jacobian).
                let integrand = |x: f64| {
                    let t = (x + 1.0) / 2.0;
                    x * beta.pdf(t) / 2.0
                };
                let mass = adaptive_simpson(&integrand, lo, hi, 1e-14, 50);
                new_centroids[i] = mass / prob;
            }
        }

        // Check for convergence in f64 space.
        let max_change = centroids
            .iter()
            .zip(new_centroids.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        centroids = new_centroids;
        if max_change < tol {
            break;
        }
    }

    // Final pass: convert to f32. Boundaries are midpoints of the final
    // centroids; consistent with what the encode path will use.
    let boundaries: Vec<f32> = (0..n_levels - 1)
        .map(|i| ((centroids[i] + centroids[i + 1]) / 2.0) as f32)
        .collect();
    let centroids_f32: Vec<f32> = centroids.iter().map(|&c| c as f32).collect();

    (boundaries, centroids_f32)
}

/// Adaptive Simpson's rule for one-dimensional numerical integration.
///
/// Implementation is iterative-style via a recursive helper that bisects
/// when the per-interval error estimate exceeds `tol`. `max_depth` caps
/// the recursion. The tolerances and depth used at call sites here
/// (1e-14, 50) are conservative — the integrand is smooth on the open
/// interval but pushes against the Beta's [-1, 1] support endpoints.
fn adaptive_simpson<F: Fn(f64) -> f64>(f: F, a: f64, b: f64, tol: f64, max_depth: usize) -> f64 {
    let mid = (a + b) / 2.0;
    let fa = f(a);
    let fb = f(b);
    let fm = f(mid);
    let whole = (b - a) / 6.0 * (fa + 4.0 * fm + fb);
    adaptive_simpson_rec(&f, a, b, fa, fb, fm, whole, tol, max_depth)
}

#[allow(clippy::too_many_arguments)]
fn adaptive_simpson_rec<F: Fn(f64) -> f64>(
    f: &F,
    a: f64,
    b: f64,
    fa: f64,
    fb: f64,
    fm: f64,
    whole: f64,
    tol: f64,
    depth: usize,
) -> f64 {
    let mid = (a + b) / 2.0;
    let m1 = (a + mid) / 2.0;
    let m2 = (mid + b) / 2.0;
    let fm1 = f(m1);
    let fm2 = f(m2);
    let left = (mid - a) / 6.0 * (fa + 4.0 * fm1 + fm);
    let right = (b - mid) / 6.0 * (fm + 4.0 * fm2 + fb);
    let refined = left + right;
    if depth == 0 || (refined - whole).abs() < 15.0 * tol {
        refined + (refined - whole) / 15.0
    } else {
        adaptive_simpson_rec(f, a, mid, fa, fm, fm1, left, tol / 2.0, depth - 1)
            + adaptive_simpson_rec(f, mid, b, fm, fb, fm2, right, tol / 2.0, depth - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assert `(a - b).abs() < tol` with a useful message.
    fn approx(a: f32, b: f32, tol: f32, label: &str) {
        assert!(
            (a - b).abs() < tol,
            "{label}: {a} vs {b}, diff = {}",
            (a - b).abs()
        );
    }

    #[test]
    fn shapes_match_bit_width() {
        let (b2, c2) = codebook(2, 1536);
        assert_eq!(b2.len(), 3, "2-bit has 3 boundaries");
        assert_eq!(c2.len(), 4, "2-bit has 4 centroids");

        let (b4, c4) = codebook(4, 1536);
        assert_eq!(b4.len(), 15, "4-bit has 15 boundaries");
        assert_eq!(c4.len(), 16, "4-bit has 16 centroids");
    }

    #[test]
    fn boundaries_are_midpoints_of_centroids() {
        let (boundaries, centroids) = codebook(4, 1536);
        for (i, &b) in boundaries.iter().enumerate() {
            let expected = (centroids[i] + centroids[i + 1]) / 2.0;
            approx(b, expected, 1e-6, &format!("boundary[{i}]"));
        }
    }

    #[test]
    fn centroids_are_strictly_monotone() {
        // A valid Lloyd-Max codebook orders centroids by ascending value
        // (degenerate ties are excluded by Lloyd's strict-improvement
        // property on convex-distortion measures).
        let (_, centroids) = codebook(4, 1536);
        for w in centroids.windows(2) {
            assert!(
                w[0] < w[1],
                "centroid ordering violated: {} >= {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn centroids_are_symmetric_about_zero() {
        // Beta((d-1)/2, (d-1)/2) shifted to [-1, 1] is symmetric about 0,
        // so the optimal codebook is too: centroid[i] + centroid[N-1-i] ≈ 0.
        let (_, centroids) = codebook(4, 1536);
        let n = centroids.len();
        for i in 0..(n / 2) {
            let sum = centroids[i] + centroids[n - 1 - i];
            approx(sum, 0.0, 5e-3, &format!("symmetry[{i}]"));
        }
    }

    #[test]
    fn centroids_stay_inside_support_at_high_dim() {
        // At d=1536 the Beta marginal is very concentrated near 0; all
        // centroids should land well within the [-1, 1] support.
        let (_, centroids) = codebook(2, 1536);
        for &c in &centroids {
            assert!(
                c.abs() < 0.5,
                "2-bit centroid at d=1536 should be |c| < 0.5, got {c}"
            );
        }
        // At d=3072 even tighter.
        let (_, centroids) = codebook(2, 3072);
        for &c in &centroids {
            assert!(
                c.abs() < 0.4,
                "2-bit centroid at d=3072 should be |c| < 0.4, got {c}"
            );
        }
    }

    #[test]
    fn deterministic_across_calls() {
        let (b1, c1) = codebook(4, 1536);
        let (b2, c2) = codebook(4, 1536);
        for (x, y) in b1.iter().zip(b2.iter()) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
        for (x, y) in c1.iter().zip(c2.iter()) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }

    #[test]
    fn dim_affects_codebook_width() {
        // Smaller d → wider Beta marginal → wider codebook spread.
        let (_, c_small) = codebook(4, 128);
        let (_, c_large) = codebook(4, 3072);
        let span_small = c_small.last().unwrap() - c_small.first().unwrap();
        let span_large = c_large.last().unwrap() - c_large.first().unwrap();
        assert!(
            span_small > span_large,
            "expected smaller-d codebook to be wider: small={span_small}, large={span_large}"
        );
    }

    #[test]
    fn adaptive_simpson_integrates_constant() {
        // ∫_0^1 5 dx = 5.
        let v = adaptive_simpson(|_| 5.0, 0.0, 1.0, 1e-12, 20);
        assert!((v - 5.0).abs() < 1e-10);
    }

    #[test]
    fn adaptive_simpson_integrates_linear() {
        // ∫_0^2 x dx = 2.
        let v = adaptive_simpson(|x| x, 0.0, 2.0, 1e-12, 20);
        assert!((v - 2.0).abs() < 1e-10);
    }

    #[test]
    fn adaptive_simpson_integrates_quadratic() {
        // ∫_{-1}^1 x^2 dx = 2/3.
        let v = adaptive_simpson(|x| x * x, -1.0, 1.0, 1e-12, 20);
        assert!((v - 2.0 / 3.0).abs() < 1e-10);
    }
}
