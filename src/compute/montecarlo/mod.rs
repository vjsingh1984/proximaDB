/*
 * Copyright 2024 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Monte Carlo European Option Pricing Kernel
//!
//! A compute-heavy, embarrassingly-parallel financial workload used to demonstrate
//! ProximaDB's Rust-native I/O + compute performance against JVM Spark (see
//! `docs/12-design/PROXIMA_NOTEBOOK_PSEUDO_DISTRIBUTED_BLUEPRINT_2026_06_04.adoc`).
//!
//! The kernel prices European call/put options under the Black–Scholes model by
//! simulating terminal asset prices with Geometric Brownian Motion (GBM):
//!
//! ```text
//! S_T = S_0 * exp((r - 0.5*sigma^2) * T + sigma * sqrt(T) * Z),   Z ~ N(0, 1)
//! price = exp(-r*T) * mean( payoff(S_T) )
//! ```
//!
//! ## Design notes
//!
//! * **Deterministic & parallel-safe RNG**: a counter-based `splitmix64` seeded per
//!   `(row, path)` plus Box–Muller. No global RNG and no wall-clock seeding, so results
//!   are reproducible and identical regardless of thread scheduling — this is what makes
//!   the benchmark fair and the tests non-flaky.
//! * **Correctness oracle**: the closed-form [`black_scholes`] price (with a high-accuracy
//!   normal CDF) is the ground truth the Monte Carlo estimate must converge to.
//! * **Batch entry**: [`mc_price_batch`] prices many contracts in parallel with rayon —
//!   this is the hot path wrapped by the DataFusion `mc_price` scalar UDF (Phase A2).
//!
//! The kernel is intentionally dependency-free (no `rand`, no `statrs`) so it stays
//! unit-testable without the DataFusion feature and free of non-deterministic inputs.

use rayon::prelude::*;
use std::f64::consts::PI;

/// `splitmix64` step: a fast, well-distributed counter-based PRNG.
///
/// Advancing a `u64` state by the golden-ratio increment and mixing yields a stream
/// that is deterministic and independent across distinct seeds — ideal for seeding each
/// option row independently without shared mutable RNG state across rayon workers.
#[inline]
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Map the RNG state to a uniform `f64` in the open interval (0, 1).
///
/// Uses the top 53 bits (the f64 mantissa width) and centres each bucket by adding 0.5,
/// guaranteeing the value is strictly inside (0, 1) so the `ln()` in Box–Muller is finite.
#[inline]
fn next_uniform(state: &mut u64) -> f64 {
    let bits = splitmix64(state) >> 11; // 53 significant bits
    (bits as f64 + 0.5) * (1.0 / 9_007_199_254_740_992.0) // 2^53
}

/// Standard normal cumulative distribution function.
///
/// Zelen & Severo rational approximation (Abramowitz & Stegun 26.2.17), accurate to
/// ~7.5e-8 absolute — far tighter than the Monte Carlo sampling error, so it is a sound
/// oracle for the convergence tests.
#[inline]
pub fn norm_cdf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.231_641_9 * x.abs());
    let pdf = 0.398_942_280_401_4 * (-0.5 * x * x).exp(); // 1/sqrt(2*pi) * exp(-x^2/2)
    let poly = t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    let upper_tail = pdf * poly;
    if x >= 0.0 {
        1.0 - upper_tail
    } else {
        upper_tail
    }
}

/// Closed-form Black–Scholes price for a European option (the correctness oracle).
///
/// `is_call = true` prices a call, `false` a put. Degenerate inputs (`t <= 0` or
/// `vol <= 0`) collapse to the discounted forward intrinsic value.
pub fn black_scholes(spot: f64, strike: f64, vol: f64, rate: f64, t: f64, is_call: bool) -> f64 {
    let disc = (-rate * t).exp();
    if t <= 0.0 || vol <= 0.0 {
        // Deterministic forward: S_0 * e^{rT}; discounted intrinsic = max(S_0 - K*e^{-rT}, 0).
        let fwd = spot * (rate * t).exp();
        let intrinsic = if is_call {
            (fwd - strike).max(0.0)
        } else {
            (strike - fwd).max(0.0)
        };
        return disc * intrinsic;
    }
    let sqrt_t = t.sqrt();
    let d1 = ((spot / strike).ln() + (rate + 0.5 * vol * vol) * t) / (vol * sqrt_t);
    let d2 = d1 - vol * sqrt_t;
    if is_call {
        spot * norm_cdf(d1) - strike * disc * norm_cdf(d2)
    } else {
        strike * disc * norm_cdf(-d2) - spot * norm_cdf(-d1)
    }
}

/// Price a single European option via Monte Carlo simulation of `n_paths` GBM terminal
/// prices. Deterministic in `seed`: the same arguments always return the identical `f64`.
///
/// Box–Muller produces two independent normals per uniform pair, both consumed, so the
/// effective sample count equals `n_paths`.
pub fn mc_price_european(
    spot: f64,
    strike: f64,
    vol: f64,
    rate: f64,
    t: f64,
    is_call: bool,
    n_paths: usize,
    seed: u64,
) -> f64 {
    if n_paths == 0 {
        return f64::NAN;
    }
    if t <= 0.0 {
        // Already expired: payoff is the (undiscounted) intrinsic value at spot.
        return if is_call {
            (spot - strike).max(0.0)
        } else {
            (strike - spot).max(0.0)
        };
    }

    let drift = (rate - 0.5 * vol * vol) * t;
    let diffusion = vol * t.sqrt();
    let mut state = seed;
    let mut sum = 0.0f64;
    let mut produced = 0usize;

    while produced < n_paths {
        // Box–Muller transform: one uniform pair -> two standard normals.
        let u1 = next_uniform(&mut state);
        let u2 = next_uniform(&mut state);
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = 2.0 * PI * u2;
        let z0 = radius * angle.cos();
        let z1 = radius * angle.sin();

        for z in [z0, z1] {
            if produced >= n_paths {
                break;
            }
            let terminal = spot * (drift + diffusion * z).exp();
            let payoff = if is_call {
                (terminal - strike).max(0.0)
            } else {
                (strike - terminal).max(0.0)
            };
            sum += payoff;
            produced += 1;
        }
    }

    (-rate * t).exp() * (sum / n_paths as f64)
}

/// Price one row `i`, deriving an independent RNG stream from `base_seed` so adjacent rows
/// draw decorrelated path sets. Shared by the batch entry points below.
#[inline]
#[allow(clippy::too_many_arguments)]
fn price_row(
    i: usize,
    spot: &[f64],
    strike: &[f64],
    vol: &[f64],
    rate: &[f64],
    t: &[f64],
    is_call: &[bool],
    n_paths: usize,
    base_seed: u64,
) -> f64 {
    let mut s = base_seed.wrapping_add(i as u64).wrapping_add(1);
    let row_seed = splitmix64(&mut s);
    mc_price_european(
        spot[i], strike[i], vol[i], rate[i], t[i], is_call[i], n_paths, row_seed,
    )
}

#[inline]
fn assert_equal_lengths(
    spot: &[f64],
    strike: &[f64],
    vol: &[f64],
    rate: &[f64],
    t: &[f64],
    is_call: &[bool],
) {
    let n = spot.len();
    debug_assert!(
        strike.len() == n
            && vol.len() == n
            && rate.len() == n
            && t.len() == n
            && is_call.len() == n,
        "mc_price batch: all input columns must have equal length"
    );
}

/// Price a batch of European options **in parallel** (rayon), one independent RNG stream
/// per row. Use this when the caller owns the whole dataset in a single call (e.g. a
/// standalone bulk job or the kernel benchmark baseline).
///
/// Do NOT call this inside a per-partition engine operator that is itself parallelized
/// (e.g. a DataFusion UDF) — that oversubscribes cores. Use [`mc_price_batch_seq`] there.
///
/// All slices must have the same length; returns one price per row in input order.
#[allow(clippy::too_many_arguments)]
pub fn mc_price_batch(
    spot: &[f64],
    strike: &[f64],
    vol: &[f64],
    rate: &[f64],
    t: &[f64],
    is_call: &[bool],
    n_paths: usize,
    base_seed: u64,
) -> Vec<f64> {
    assert_equal_lengths(spot, strike, vol, rate, t, is_call);
    (0..spot.len())
        .into_par_iter()
        .map(|i| price_row(i, spot, strike, vol, rate, t, is_call, n_paths, base_seed))
        .collect()
}

/// Sequential variant of [`mc_price_batch`] — prices rows on the calling thread, returning
/// identical results (same per-row seeds).
///
/// This is what per-partition engine operators (the DataFusion `mc_price` UDF) call: the
/// engine already parallelizes across partitions, so nesting rayon here would oversubscribe
/// (partition-threads × rayon-threads). Parallelism comes from the engine's partitioning.
#[allow(clippy::too_many_arguments)]
pub fn mc_price_batch_seq(
    spot: &[f64],
    strike: &[f64],
    vol: &[f64],
    rate: &[f64],
    t: &[f64],
    is_call: &[bool],
    n_paths: usize,
    base_seed: u64,
) -> Vec<f64> {
    assert_equal_lengths(spot, strike, vol, rate, t, is_call);
    (0..spot.len())
        .map(|i| price_row(i, spot, strike, vol, rate, t, is_call, n_paths, base_seed))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u64 = 0x5DEE_CE66_D5B0_1234;

    fn rel_err(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1.0)
    }

    #[test]
    fn norm_cdf_known_points() {
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-9);
        // Symmetry: Phi(-x) = 1 - Phi(x).
        for &x in &[0.3, 1.0, 1.96, 2.5] {
            assert!((norm_cdf(-x) - (1.0 - norm_cdf(x))).abs() < 1e-7);
        }
        // Phi(1.96) ~ 0.975.
        assert!((norm_cdf(1.96) - 0.975).abs() < 1e-3);
    }

    #[test]
    fn mc_converges_to_black_scholes_atm_and_itm() {
        // Deterministic seed -> deterministic estimate; tolerance covers sampling error
        // at 400k paths for substantial (ATM/ITM) prices.
        let rate = 0.03;
        let vol = 0.2;
        let t = 1.0;
        let n = 400_000;
        let spot = 100.0;
        for &strike in &[80.0, 100.0, 120.0] {
            for &is_call in &[true, false] {
                let mc = mc_price_european(spot, strike, vol, rate, t, is_call, n, SEED);
                let bs = black_scholes(spot, strike, vol, rate, t, is_call);
                assert!(
                    rel_err(mc, bs) < 0.03,
                    "strike={strike} call={is_call}: mc={mc:.4} bs={bs:.4} rel_err={:.4}",
                    rel_err(mc, bs)
                );
            }
        }
    }

    #[test]
    fn put_call_parity_holds_on_shared_paths() {
        // C - P = S0 - K*e^{-rT}. With the same seed the call/put share path samples,
        // so the residual is pure mean-sampling error and must be small.
        let (spot, strike, vol, rate, t, n) = (100.0, 105.0, 0.25, 0.02, 0.75, 400_000);
        let call = mc_price_european(spot, strike, vol, rate, t, true, n, SEED);
        let put = mc_price_european(spot, strike, vol, rate, t, false, n, SEED);
        let theoretical = spot - strike * (-rate * t).exp();
        assert!(
            (call - put - theoretical).abs() < 0.25,
            "parity: C-P={:.4} expected={:.4}",
            call - put,
            theoretical
        );
    }

    #[test]
    fn deterministic_same_seed_same_price() {
        let a = mc_price_european(100.0, 100.0, 0.2, 0.03, 1.0, true, 50_000, SEED);
        let b = mc_price_european(100.0, 100.0, 0.2, 0.03, 1.0, true, 50_000, SEED);
        assert_eq!(a.to_bits(), b.to_bits(), "same seed must be bit-identical");
    }

    #[test]
    fn zero_vol_degenerates_to_discounted_intrinsic() {
        // With zero volatility every path is the deterministic forward, so MC == BS ==
        // discounted forward intrinsic.
        let (spot, strike, rate, t, n) = (100.0, 90.0, 0.05, 1.0, 1_000);
        let mc = mc_price_european(spot, strike, 0.0, rate, t, true, n, SEED);
        let expected = (spot - strike * (-rate * t).exp()).max(0.0);
        assert!(rel_err(mc, expected) < 1e-6, "mc={mc} expected={expected}");
        assert!((black_scholes(spot, strike, 0.0, rate, t, true) - expected).abs() < 1e-9);
    }

    #[test]
    fn batch_matches_per_row_calls() {
        // The parallel batch must equal independent per-row pricing with the same derived
        // seeds (no cross-row contamination from rayon).
        let spot = vec![100.0, 95.0, 110.0];
        let strike = vec![100.0, 100.0, 105.0];
        let vol = vec![0.2, 0.3, 0.15];
        let rate = vec![0.03, 0.03, 0.03];
        let t = vec![1.0, 0.5, 2.0];
        let is_call = vec![true, false, true];
        let n = 20_000;

        let batch = mc_price_batch(&spot, &strike, &vol, &rate, &t, &is_call, n, 42);
        assert_eq!(batch.len(), 3);
        for (i, &price) in batch.iter().enumerate() {
            let mut s = 42u64.wrapping_add(i as u64).wrapping_add(1);
            let row_seed = splitmix64(&mut s);
            let direct = mc_price_european(
                spot[i], strike[i], vol[i], rate[i], t[i], is_call[i], n, row_seed,
            );
            assert_eq!(price.to_bits(), direct.to_bits());
        }
    }

    #[test]
    fn seq_and_parallel_batches_are_identical() {
        let spot = vec![100.0, 95.0, 110.0, 100.0, 105.0];
        let strike = vec![100.0, 100.0, 105.0, 90.0, 110.0];
        let vol = vec![0.2, 0.3, 0.15, 0.25, 0.2];
        let rate = vec![0.03; 5];
        let t = vec![1.0, 0.5, 2.0, 1.0, 0.25];
        let is_call = vec![true, false, true, false, true];
        let par = mc_price_batch(&spot, &strike, &vol, &rate, &t, &is_call, 10_000, 7);
        let seq = mc_price_batch_seq(&spot, &strike, &vol, &rate, &t, &is_call, 10_000, 7);
        assert_eq!(par.len(), seq.len());
        for (p, s) in par.iter().zip(seq.iter()) {
            assert_eq!(
                p.to_bits(),
                s.to_bits(),
                "seq and parallel must match bit-for-bit"
            );
        }
    }

    #[test]
    fn batch_prices_are_reasonable_vs_black_scholes() {
        let spot = vec![100.0; 4];
        let strike = vec![90.0, 100.0, 110.0, 100.0];
        let vol = vec![0.2, 0.2, 0.2, 0.4];
        let rate = vec![0.03; 4];
        let t = vec![1.0; 4];
        let is_call = vec![true, true, true, false];
        let n = 400_000;
        let batch = mc_price_batch(&spot, &strike, &vol, &rate, &t, &is_call, n, SEED);
        for i in 0..4 {
            let bs = black_scholes(spot[i], strike[i], vol[i], rate[i], t[i], is_call[i]);
            assert!(
                rel_err(batch[i], bs) < 0.04,
                "row {i}: mc={:.4} bs={bs:.4}",
                batch[i]
            );
        }
    }
}
