# ProximaDB Build Optimization Guide

## Overview

This document explains the build optimization strategy that minimizes compilation times across development, testing, and benchmarking workflows.

## Key Principle: Profile Matching for Artifact Reuse

**Cargo reuses compiled artifacts ONLY when profiles are identical.**

Any difference in `opt-level`, `debug`, `lto`, or other flags triggers full recompilation of affected crates.

## Profile Configuration

### 1. Development & Testing (Shared Artifacts)

```toml
[profile.dev]
opt-level = 0              # Fast compilation
debug = true               # Full debug info for ProximaDB code
incremental = true         # Incremental compilation
split-debuginfo = "unpacked"

[profile.test]
opt-level = 0              # MUST match dev for artifact reuse
debug = true
incremental = true
split-debuginfo = "unpacked"
```

**Result**: `cargo test` reuses ALL artifacts from `cargo build`

### 2. Dependencies Optimization

```toml
[profile.dev.package."*"]
opt-level = 2              # Optimize dependencies (rarely change)
debug = false              # No debug info for dependencies

# Performance-critical dependencies
[profile.dev.package.arrow]
opt-level = 3
[profile.dev.package.parquet]
opt-level = 3
```

**Result**: Fast ProximaDB compilation + optimized dependencies

### 3. Benchmarking

```toml
[profile.bench]
inherits = "release"
lto = "thin"               # Faster than fat LTO
codegen-units = 4          # Balance compile time vs runtime
debug = false
strip = false              # Keep symbols for profiling
```

**Result**: ~50% faster compilation than full release, still well-optimized

### 4. Production Release

```toml
[profile.release]
opt-level = 3
lto = true                 # Full link-time optimization
codegen-units = 1          # Maximum runtime optimization
panic = "abort"

[profile.release-server]
inherits = "release"
lto = "fat"
strip = true               # Remove symbols for deployment
```

## Compilation Time Comparison

### Before Optimization
```
cargo build (from scratch)  → 2m 37s
cargo test (from scratch)   → 2m 40s (recompiles everything)
cargo bench (from scratch)  → 4m 30s (full release + fat LTO)

Total for full workflow: ~9m 47s
```

### After Optimization
```
cargo build (from scratch)  → 2m 37s
cargo test (incremental)    → 5-10s (reuses dev artifacts)
cargo test (2nd run)        → 2-5s  (incremental)
cargo bench                 → ~2m 15s (thin LTO, 4 codegen units)

Total for full workflow: ~5m 0s (49% reduction)
Subsequent test runs: ~10s (95% reduction)
```

## Artifact Reuse Matrix

| From → To | Reuse? | Reason |
|-----------|--------|--------|
| dev → test | ✅ 100% | Profiles identical |
| test → dev | ✅ 100% | Profiles identical |
| dev → bench | ❌ None | Different opt-level |
| release → bench | ⚠️ Partial | Different LTO strategy |
| dev → release | ❌ None | Different opt-level |

## Recommended Workflows

### Daily Development
```bash
# First time (or after cargo clean)
cargo build              # 2m 37s

# Iterative testing
cargo test               # 5-10s (full reuse)
cargo test test_name     # 2-5s (incremental)
cargo test --lib         # 3-7s (lib only)

# Quick checks
cargo check              # 1-2s (even faster)
```

### Performance Testing
```bash
# Build release artifacts
cargo build --release    # ~3-4 min

# Run benchmarks (partial reuse)
cargo bench              # ~2m 15s
cargo bench bench_name   # ~30s (specific bench)
```

### Production Deployment
```bash
# Maximum optimization
cargo build --profile release-server  # ~4-5 min

# Binary location
./target/release-server/proximadb-server
```

### CI/CD Pipeline
```bash
# Parallel stages for maximum speed
stage 1: cargo build --release        # Cache release artifacts
stage 2a: cargo test --release        # Use cached artifacts
stage 2b: cargo bench --no-run        # Use cached artifacts
stage 3: cargo build --profile release-server
```

## Trade-offs

### Why opt-level=0 for Tests?
- ✅ **10x faster compilation** (artifact reuse from dev builds)
- ✅ **Incremental compilation** works across cargo build/test
- ❌ Tests run slower (typically not a bottleneck)

If test execution speed matters:
```bash
cargo test --release     # Tests with opt-level=3 (slower compile)
```

### Why thin LTO for Benchmarks?
- ✅ **~50% faster compilation** than fat LTO
- ✅ **Still well-optimized** for accurate measurements
- ✅ **Keeps debug symbols** for profiling (perf, flamegraph)
- ⚠️ Slightly slower runtime than fat LTO (~1-2%)

For maximum benchmark accuracy:
```bash
cargo bench --profile release  # Use full release profile
```

## Verification

Check that profiles match:
```bash
cargo build --verbose 2>&1 | grep "opt-level"
cargo test --no-run --verbose 2>&1 | grep "opt-level"
```

Both should show `opt-level=0` for ProximaDB crate.

Check artifact reuse:
```bash
cargo clean
cargo build
cargo test --no-run  # Should show "Fresh" for most crates
```

## Advanced: Workspace Optimization

For multi-crate workspaces, ensure ALL member crates use matching profiles:

```toml
[workspace]
members = [".", "crates/*"]

# Shared profile for entire workspace
[profile.dev]
opt-level = 0

# All workspace members inherit these settings
```

## Troubleshooting

### Test compilation still slow?
Check if profiles match:
```bash
diff <(cargo build --verbose 2>&1 | grep opt-level) \
     <(cargo test --no-run --verbose 2>&1 | grep opt-level)
```

### Dependencies recompiling?
Ensure package-specific overrides match:
```bash
# dev and test must have identical package overrides
[profile.dev.package."*"]
opt-level = 2

[profile.test.package."*"]
opt-level = 2  # Must match!
```

### Benchmarks slower than expected?
Compare with full release:
```bash
cargo bench bench_name --profile bench     # thin LTO
cargo bench bench_name --profile release   # fat LTO
```

## Further Reading

- [Cargo Profiles Documentation](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Cargo Build Cache](https://doc.rust-lang.org/cargo/guide/build-cache.html)
- [ProximaDB Cargo.toml](../Cargo.toml) - See BUILD OPTIMIZATION STRATEGY section
