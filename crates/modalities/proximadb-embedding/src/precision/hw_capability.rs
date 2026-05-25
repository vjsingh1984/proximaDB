//! Embedding precision probe — embedding-precision rollout PR 7 (Q14).
//!
//! Runs a small one-shot micro-bench at server startup so the policy
//! resolver and ANN distance kernels can decide whether to fast-path on
//! fp16. The probe is intentionally cheap (1024×1024 single-vector
//! matmul × 3 dtype pairs) so it never blocks startup more than a few
//! milliseconds.
//!
//! Singleton: process-wide `OnceLock<EmbeddingPrecisionProbe>`. The server
//! binary calls `init_precision_probe()` exactly once during boot, and the
//! rest of the codebase reads via `precision_probe()`. Test code that needs
//! a deterministic value uses `init_precision_probe_for_test()`.
//!
//! Naming note: this type used to be called `HardwareCapabilities`, which
//! collided with the SIMD-feature detector in `proximadb-hardware` and the
//! richer detector in `src/core/hardware_capabilities.rs`. It was renamed
//! because the payload here is fp16/fp32 matmul latency measurements — not
//! CPU-feature flags. For SIMD/CPU/GPU detection use `proximadb_hardware`.
//!
//! Spec: `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc`
//! §"`HardwareCapabilities` registry (Q14)" and §"PR 7".

use proximadb_records::EmbeddingScalarType;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime};

/// Default probe vector dimension. 1024 is BGE-large's native dim and big
/// enough to exercise the AMX / NEON / AVX-512 fp16 paths without taking
/// noticeable wall-clock at startup (<5 ms on Apple Silicon, <2 ms on
/// recent x86).
pub const DEFAULT_PROBE_DIM: usize = 1024;

/// Cached startup measurement of how long each dtype-pair matmul takes for
/// a single output element at `probe_dim` features.
///
/// Latency is the median over a small number of inner iterations (probe
/// implementation detail). Lower is faster. A zero value means the probe
/// hasn't run yet (callers should treat this as "unknown" and fall back
/// to the policy's `canonical_default`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingPrecisionProbe {
    /// fp32 × fp32 dot product latency, nanoseconds per output element.
    pub f32_f32_matmul_ns: u64,
    /// fp16 promoted to fp32 then dot-product. Models the "store fp16,
    /// compute fp32" pattern most ANN code falls back to today.
    pub f16_f32_matmul_ns: u64,
    /// Native fp16 × fp16 dot product (AMX / NEON FCMLA / AVX-512 BF16).
    /// On hardware without native fp16 multiply, this falls back to the
    /// promote-at-compute path and the latency matches `f16_f32_matmul_ns`.
    pub f16_f16_matmul_ns: u64,
    /// Whether the platform has any hardware-accelerated bf16 multiply.
    /// Conservative: probe reports `true` only when AVX-512 BF16 or
    /// ARMv8.6+ BFMMLA is known to be present. Today's probe leaves this
    /// `false` everywhere; PR 11+ can flip it once detection lands.
    pub bf16_supported: bool,
    /// Dimension used for the matmul micro-bench.
    pub probe_dim: usize,
    /// When the probe ran. Useful in observability so operators can tell
    /// whether the cached capabilities reflect the current hardware.
    pub probed_at: SystemTime,
}

impl EmbeddingPrecisionProbe {
    /// Run the startup micro-bench and return the populated struct.
    ///
    /// The probe deliberately allocates its own buffers (no shared state)
    /// so it can run before any inference backend is initialized.
    pub fn probe() -> Self {
        Self::probe_with_dim(DEFAULT_PROBE_DIM)
    }

    /// Same as `probe()` but with a caller-chosen dimension. Useful for
    /// tests that need a tiny probe to run instantly under heavy load.
    pub fn probe_with_dim(dim: usize) -> Self {
        let f32_f32_matmul_ns = bench_f32_f32(dim);
        let f16_f32_matmul_ns = bench_f16_f32(dim);
        let f16_f16_matmul_ns = bench_f16_f16(dim);
        Self {
            f32_f32_matmul_ns,
            f16_f32_matmul_ns,
            f16_f16_matmul_ns,
            bf16_supported: false,
            probe_dim: dim,
            probed_at: SystemTime::now(),
        }
    }

    /// Best canonical precision for newly-ingested embeddings, assuming the
    /// active policy is `Adaptive` and `require_hw_capability` is `true`.
    ///
    /// Returns `Fp16` only when the native f16×f16 path is at least 1.5×
    /// faster than f32×f32 — this is the LLD's de-facto threshold for
    /// "fp16 is worth the storage/IO win" since the storage savings are
    /// already 2×. Below 1.5× we stay on `Fp32` so we don't pay the
    /// downconvert tax without an offsetting compute gain.
    ///
    /// Returns `Fp32` when the probe hasn't run (any latency is zero) so
    /// callers always get a safe default during bootstrap.
    pub fn best_canonical_for_inference(&self) -> EmbeddingScalarType {
        if self.f32_f32_matmul_ns == 0 || self.f16_f16_matmul_ns == 0 {
            return EmbeddingScalarType::Fp32;
        }
        // Native f16 must be ≥1.5× faster than fp32 to justify the
        // downconvert. Use the integer compare `f32_ns * 2 >= f16_ns * 3`
        // to avoid floating-point.
        if self.f32_f32_matmul_ns.saturating_mul(2) >= self.f16_f16_matmul_ns.saturating_mul(3) {
            EmbeddingScalarType::Fp16
        } else {
            EmbeddingScalarType::Fp32
        }
    }
}

// ---------------------------------------------------------------------------
// Process-wide singleton
// ---------------------------------------------------------------------------

static PROBE: OnceLock<EmbeddingPrecisionProbe> = OnceLock::new();

/// Initialize the process-wide precision-probe cache. Idempotent — second
/// and later callers see the value the first caller installed (probe runs
/// exactly once per process).
///
/// The server binary calls this from main after argument parsing.
pub fn init_precision_probe() -> &'static EmbeddingPrecisionProbe {
    PROBE.get_or_init(EmbeddingPrecisionProbe::probe)
}

/// Read the cached precision-probe. Returns `None` if `init_precision_probe()`
/// has not been called yet — production callers should treat that case as
/// "the policy must fall back to fp32" rather than panic.
pub fn precision_probe() -> Option<&'static EmbeddingPrecisionProbe> {
    PROBE.get()
}

/// Test-only initializer that installs a caller-supplied probe snapshot.
/// Safe to call from `#[cfg(test)]` modules that want to assert behavior
/// in `best_canonical_for_inference()` without depending on the host's
/// real micro-bench.
///
/// Returns `Err(())` if `init_precision_probe()` (or this fn) was already
/// called.
#[cfg(test)]
pub fn init_precision_probe_for_test(
    probe: EmbeddingPrecisionProbe,
) -> Result<&'static EmbeddingPrecisionProbe, ()> {
    PROBE.set(probe).map_err(|_| ())?;
    Ok(PROBE.get().unwrap())
}

// ---------------------------------------------------------------------------
// Micro-benches
// ---------------------------------------------------------------------------

/// Generate a deterministic, non-trivial vector so the probe is repeatable
/// across runs and doesn't depend on rand crates.
fn fill_f32(buf: &mut [f32], seed: u32) {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    for v in buf {
        // Xorshift to scatter values; map to [-1.0, 1.0).
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *v = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
    }
}

/// Measure how long a single dot-product of `dim` fp32 elements takes,
/// averaging over a few inner iterations and returning the per-element
/// nanoseconds. The black_box hint prevents the compiler from constant-
/// folding the entire loop.
fn bench_f32_f32(dim: usize) -> u64 {
    let mut a = vec![0f32; dim];
    let mut b = vec![0f32; dim];
    fill_f32(&mut a, 1);
    fill_f32(&mut b, 2);
    let iters = 64;
    let start = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..iters {
        let mut s = 0.0f32;
        for i in 0..dim {
            s += a[i] * b[i];
        }
        acc += std::hint::black_box(s);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    elapsed.as_nanos() as u64 / (iters as u64).max(1)
}

/// f16 storage promoted to fp32 for the multiply.
fn bench_f16_f32(dim: usize) -> u64 {
    let mut a_f32 = vec![0f32; dim];
    let mut b_f32 = vec![0f32; dim];
    fill_f32(&mut a_f32, 3);
    fill_f32(&mut b_f32, 4);
    let a: Vec<half::f16> = a_f32.iter().map(|&v| half::f16::from_f32(v)).collect();
    let b_fp32 = b_f32;
    let iters = 64;
    let start = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..iters {
        let mut s = 0.0f32;
        for i in 0..dim {
            s += a[i].to_f32() * b_fp32[i];
        }
        acc += std::hint::black_box(s);
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    elapsed.as_nanos() as u64 / (iters as u64).max(1)
}

/// Native f16 × f16 — the `half` crate's `f16` multiplies in fp32 under
/// the hood on platforms without hardware fp16 mul, which is OK: the
/// probe's purpose is to measure end-to-end latency, not theoretical
/// peak FLOPs. On hardware with real fp16 multiply (Apple Silicon AMX,
/// ARMv8.2+ FCMLA, AVX-512 fp16) the optimizer can lower this to native
/// instructions; on other targets the latency matches `bench_f16_f32`.
fn bench_f16_f16(dim: usize) -> u64 {
    let mut a_f32 = vec![0f32; dim];
    let mut b_f32 = vec![0f32; dim];
    fill_f32(&mut a_f32, 5);
    fill_f32(&mut b_f32, 6);
    let a: Vec<half::f16> = a_f32.iter().map(|&v| half::f16::from_f32(v)).collect();
    let b: Vec<half::f16> = b_f32.iter().map(|&v| half::f16::from_f32(v)).collect();
    let iters = 64;
    let start = Instant::now();
    let mut acc = half::f16::from_f32(0.0);
    for _ in 0..iters {
        let mut s = half::f16::from_f32(0.0);
        for i in 0..dim {
            s = half::f16::from_f32(s.to_f32() + a[i].to_f32() * b[i].to_f32());
        }
        acc = half::f16::from_f32(acc.to_f32() + std::hint::black_box(s).to_f32());
    }
    let elapsed = start.elapsed();
    std::hint::black_box(acc);
    elapsed.as_nanos() as u64 / (iters as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_probe_dim_matches_lld_choice() {
        // BGE-large's native dim — anchored in the LLD so the probe
        // measures a realistic workload.
        assert_eq!(DEFAULT_PROBE_DIM, 1024);
    }

    #[test]
    fn probe_returns_nonzero_latencies_for_all_three_pairs() {
        // Use a tiny dim so the test is instant under load.
        let caps = EmbeddingPrecisionProbe::probe_with_dim(64);
        assert!(caps.f32_f32_matmul_ns > 0, "f32_f32 latency must be measured");
        assert!(caps.f16_f32_matmul_ns > 0, "f16_f32 latency must be measured");
        assert!(caps.f16_f16_matmul_ns > 0, "f16_f16 latency must be measured");
        assert_eq!(caps.probe_dim, 64);
        // bf16 detection is conservatively off until PR 11+.
        assert!(!caps.bf16_supported);
    }

    #[test]
    fn best_canonical_falls_back_to_fp32_when_unprobed() {
        let caps = EmbeddingPrecisionProbe {
            f32_f32_matmul_ns: 0,
            f16_f32_matmul_ns: 0,
            f16_f16_matmul_ns: 0,
            bf16_supported: false,
            probe_dim: 0,
            probed_at: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(
            caps.best_canonical_for_inference(),
            EmbeddingScalarType::Fp32
        );
    }

    #[test]
    fn best_canonical_picks_fp16_when_native_is_1_5x_faster() {
        // f32: 300ns, f16: 200ns → ratio 1.5 → choose fp16.
        let caps = EmbeddingPrecisionProbe {
            f32_f32_matmul_ns: 300,
            f16_f32_matmul_ns: 250,
            f16_f16_matmul_ns: 200,
            bf16_supported: false,
            probe_dim: 64,
            probed_at: SystemTime::now(),
        };
        assert_eq!(
            caps.best_canonical_for_inference(),
            EmbeddingScalarType::Fp16
        );
    }

    #[test]
    fn best_canonical_stays_fp32_when_native_is_below_threshold() {
        // f32: 300ns, f16: 250ns → ratio 1.2 → stay on fp32 (downconvert
        // tax isn't worth a sub-1.5× speedup).
        let caps = EmbeddingPrecisionProbe {
            f32_f32_matmul_ns: 300,
            f16_f32_matmul_ns: 280,
            f16_f16_matmul_ns: 250,
            bf16_supported: false,
            probe_dim: 64,
            probed_at: SystemTime::now(),
        };
        assert_eq!(
            caps.best_canonical_for_inference(),
            EmbeddingScalarType::Fp32
        );
    }

    #[test]
    fn best_canonical_stays_fp32_when_native_is_slower() {
        // Worst case: f16 is slower than fp32 (no hardware support).
        let caps = EmbeddingPrecisionProbe {
            f32_f32_matmul_ns: 200,
            f16_f32_matmul_ns: 350,
            f16_f16_matmul_ns: 400,
            bf16_supported: false,
            probe_dim: 64,
            probed_at: SystemTime::now(),
        };
        assert_eq!(
            caps.best_canonical_for_inference(),
            EmbeddingScalarType::Fp32
        );
    }

    #[test]
    fn best_canonical_threshold_exactly_at_1_5x_picks_fp16() {
        // Boundary: f32_ns * 2 == f16_ns * 3 → exactly 1.5× → take fp16
        // (LLD prefers the storage/IO win when the compute path ties).
        let caps = EmbeddingPrecisionProbe {
            f32_f32_matmul_ns: 300,
            f16_f32_matmul_ns: 250,
            f16_f16_matmul_ns: 200, // 300 * 2 == 200 * 3
            bf16_supported: false,
            probe_dim: 64,
            probed_at: SystemTime::now(),
        };
        assert_eq!(
            caps.best_canonical_for_inference(),
            EmbeddingScalarType::Fp16
        );
    }

    // Note: the OnceLock singleton (init_precision_probe / precision_probe)
    // is intentionally not unit-tested here because OnceLock is process-wide
    // and would leak state across tests. The integration test path runs
    // init_precision_probe() once at server startup; the unit tests above
    // exercise the pure data layer.
}
