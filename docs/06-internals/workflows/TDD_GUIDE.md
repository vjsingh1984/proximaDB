# TDD Development Guide

This guide explains how to use Test-Driven Development (TDD) methodology when contributing to ProximaDB.

## What is TDD?

Test-Driven Development follows a simple cycle:

1. **🔴 Red**: Write a failing test that defines the desired behavior
2. **🟢 Green**: Write minimal code to make the test pass
3. **♻️ Refactor**: Improve the code while keeping tests green

## Why TDD?

- **Specification**: Tests serve as executable specifications
- **Regression Prevention**: Bugs are caught immediately
- **Better Design**: Writing tests first forces you to think about the API
- **Documentation**: Tests show how to use the code
- **Confidence**: Refactor safely knowing tests catch breakage

## ProximaDB Testing Realities

Generic TDD applies, but ProximaDB has concrete rules that override the defaults above. Read these before writing tests.

### nextest profiles (zero-retry unit contract)

Tests run under [`cargo-nextest`](https://nexte.st) with profiles defined in `.config/nextest.toml`:

| Profile | Retries | Use |
|---------|---------|-----|
| `unit` | **0** | Library unit tests (`--lib`). The blocking CI gate (`ci.yml` `rust-test`) runs `cargo nextest run --lib --profile unit`. |
| `integration` | 1 | Integration tests — tolerates exactly one transient **port-bind** flake, nothing else. |
| `default` | 0 | Fallback. |

**The unit profile has zero retries on purpose.** A unit test that only passes on retry is a *bug in the test or the code*, not something to paper over. Do **not** raise `retries` to make a red unit test green — that has masked real defects here before (e.g. a load-induced WAL-recovery visibility bug). Run locally exactly as CI does:

```bash
cargo nextest run --lib --profile unit
```

### Determinism: filesystem, WAL, and one-boot-per-process

- **Use a fresh temp dir per test** (`tempfile::TempDir`). Never hardcode `/tmp` or `/private/tmp` — that aliases state across concurrent tests and is non-deterministic under load.
- **One ProximaDB boot per test process.** The global WAL manifest is a *set-once process singleton*; a second full-DB boot in the same test binary reads stale/empty state. Either give each scenario its own test file, or run multiple phases inside a single `#[tokio::test]`.
- **No wall-clock/sleep-based assertions.** Drive on observable state (await the condition), not `sleep(n)`.

### Tenant-path and route-contract tests

- **Tenant isolation:** any test that writes to object storage must go through `DrPathBuilder` (`data/{tenant_id}/{namespace_id}/{collection_id}/`). The `scripts/check_tenant_path_guard.py` CI guard fails new raw `format!("data/{...}/...")` prefixes — assert on `DrResolvedPath::root_prefix()`, not hand-built strings.
- **Routing is a contract:** when you touch query routing, assert the *route decision* and *EXPLAIN output*, not just results. See `ComputeScheduler::route_select` route-selection tests and the `EXPLAIN`/`compute_route` assertions in `tests/pgwire_relational_engine_e2e.rs` for the pattern (OLTP→Native/Volcano, OLAP-over-Parquet→DataFusion, "OLTP never routes off native").

### Flaky-test quarantine policy

1. **First, fix it.** Reproduce with the unit profile (`cargo nextest run --lib --profile unit`, repeat). Flakiness is almost always shared global state, a temp-dir collision, a second boot, or a timing assumption — see the determinism rules above.
2. **Never mask with retries.** Do not add nextest `retries` to the `unit` profile. The single `integration` retry exists only for OS port-bind races.
3. **If you must quarantine**, mark the test `#[ignore = "flaky: <one-line reason> — tracked in <issue/TD>"]` with a tracked owner. A quarantined test is debt with a due date, not a resolution.

## Project Structure

```
tests/
├── tdd/                           # Integration tests
│   ├── mod.rs
│   └── test_utils/                # Common test utilities
│       ├── mod.rs
│       ├── approx.rs              # Approximate equality assertions
│       ├── mock_data.rs           # Mock data generators
│       └── perf.rs                # Performance assertions

src/
└── <module>/                     # Unit tests (in same file as implementation)
    └── tests/
        └── <module>_test.rs       # Module-specific unit tests

clients/python/tests/
└── tdd/                          # Python SDK tests
    └── __init__.py
```

## Quick Start

### 1. Set Up Your Environment

```bash
# Install TDD pre-commit hook
make install-tdd-hooks

# Install test dependencies
cargo install cargo-llvm-cov cargo-nextest
```

### 2. Start TDD Cycle for a New Feature

```bash
# 1. Create a new test (it will fail)
#    Example: src/core/search/hybrid/tests/fusion_test.rs

# 2. Run the test (should fail)
make test-tdd-unit

# 3. Implement minimal code to pass
#    Example: src/core/search/hybrid/fusion.rs

# 4. Run test again (should pass)
make test-tdd-unit

# 5. Refactor while keeping tests green
make test-tdd-unit
```

### 3. Run All Tests Before Committing

```bash
# Run pre-commit checks
make tdd-precommit

# Or manually:
make fmt && make clippy && make test
```

## Test Utilities

### TestContext

Automatic cleanup for tests:

```rust
use proxima::tdd::test_utils::TestContext;

#[tokio::test]
async fn test_feature() {
    let ctx = TestContext::new().await.unwrap();
    ctx.create_vector_collection(128).await.unwrap();

    // ... test code ...

    // ctx automatically cleans up on drop
}
```

### AssertApprox

Floating-point comparisons:

```rust
use proxima::tdd::test_utils::AssertApprox;

#[test]
fn test_vector_similarity() {
    let vec1 = vec![0.1, 0.2, 0.3];
    let vec2 = vec![0.1001, 0.2001, 0.3001];

    AssertApprox::assert_vec_close(&vec1, &vec2, 0.001);
}
```

### MockData

Generate realistic test data:

```rust
use proxima::tdd::test_utils::MockData;

#[test]
fn test_with_mock_data() {
    let vector = MockData::random_normalized_vector(128);
    let text = MockData::random_text(20);
    let bar = MockData::random_ohlcv_bar(timestamp_ns);

    // ... test code ...
}
```

### AssertPerf

Performance assertions:

```rust
use proxima::tdd::test_utils::AssertPerf;

#[tokio::test]
async fn test_search_performance() {
    AssertPerf::assert_duration_under(
        async { db.search(query).await },
        100  // 100ms max
    ).await.unwrap();
}
```

## Writing Good Tests

### DO's ✅

- **Write tests FIRST** - Before implementation
- **Test ONE thing** - Each test should verify one behavior
- **Use descriptive names** - `test_hybrid_search_rrf_fusion()` not `test1()`
- **Test edge cases** - Empty inputs, boundary conditions, errors
- **Use test utilities** - TestContext, MockData, etc.
- **Make tests independent** - No test depends on another test
- **Test behavior, not implementation** - Test what, not how

### DON'Ts ❌

- **Don't skip tests** - Every feature needs tests
- **Don't test everything** - Focus on important paths
- **Prefer `expect("why")` over bare `unwrap()` in tests** - so a failure names what broke. (The hard *no-`unwrap`/`expect`/`panic!`* mandate in CLAUDE.md is for **production** code, not tests; tests may use them, but legible messages beat bare unwraps.)
- **Don't make tests fragile** - Avoid brittle timing assumptions
- **Don't write implementation-specific tests** - Test the contract, not details

## Test Organization

### Unit Tests (`src/<module>/tests/`)

Test individual functions and types:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fusion_rrf_basic() {
        // Test RRF fusion with simple input
        let engine = HybridFusionEngine::new(FusionStrategy::ReciprocalRank { k: 60 });
        // ... test code ...
    }
}
```

### Integration Tests (`tests/tdd/`)

Test multiple modules working together:

```rust
#[tokio::test]
async fn hybrid_search_end_to_end() {
    let ctx = TestContext::new().await.unwrap();
    ctx.create_vector_collection(128).await.unwrap();

    // Insert test data
    // Query via API
    // Assert results
}
```

## Common Test Patterns

### Test Error Cases

```rust
#[test]
fn test_empty_collection() {
    let ctx = TestContext::new().await.unwrap();

    // Empty collection should return empty results
    let results = ctx.db.search(&ctx.collection_name, query).await.unwrap();
    assert!(results.is_empty());
}
```

### Test with Mock Data

```rust
#[tokio::test]
async fn test_with_clustered_data() {
    let ctx = TestContext::new().await.unwrap();

    // Generate correlated data for testing recall
    let (vectors, texts) = MockData::clustered_documents(3, 10, 128);
    ctx.insert_test_vectors(vectors, texts_to_metadata(texts)).await.unwrap();

    // Query should find same-cluster documents
    let results = ctx.db.search(&ctx.collection_name, query).await.unwrap();
    assert!(results.len() >= 5);
}
```

### Test Performance Requirements

```rust
#[tokio::test]
async fn test_search_latency_p99() {
    let ctx = TestContext::new().await.unwrap();

    // Insert 10K vectors
    // ...

    AssertPerf::assert_duration_under(
        async { ctx.db.search(&ctx.collection_name, query).await },
        100  // P99 latency target: 100ms
    ).await.unwrap();
}
```

## CI/CD Integration

### What actually gates a merge

The **blocking** release gate is `.github/workflows/ci.yml`, aggregated by the `ci-success` job (triggers on `main`/`develop`/`development` + PRs). It runs, among ~30 jobs:

- `cargo fmt --all -- --check`
- `cargo clippy --lib --bins -- -D warnings`
- **`cargo nextest run --lib --profile unit`** (the zero-retry unit contract)
- proto / OpenAPI drift checks; targeted integration tests
- `scripts/validate_capability_matrix.py` (capability + maturity-contract guard)
- `scripts/check_workspace_boundaries.py` and `scripts/check_tenant_path_guard.py` (layering + DrPathBuilder mandate, via the Workspace Layering Check workflow)

`.github/workflows/tdd.yml` is an **advisory** TDD suite (several steps are `continue-on-error`); a green `tdd.yml` is *not* the merge gate — `ci-success` is. Don't rely on tdd.yml to catch what the gate enforces.

### Pre-commit Hook

Automatically runs before each commit:
```bash
make install-tdd-hooks  # Install once
git commit              # Hook runs automatically
```

## Coverage Goals

> **Note:** the "Current" column below is illustrative and goes stale fast. For live numbers run `make test-coverage` / `scripts/coverage_by_module.sh`; do not cite these figures as the current state.

| Module | Target | Current (illustrative) |
|--------|--------|---------|
| Core (storage, compute, query) | >80% | ~49% |
| Graph | >80% | ~52% |
| API handlers | >90% | ~5% |
| Overall | >70% | ~49% |

Check coverage:
```bash
make test-coverage
open coverage/index.html
```

## Troubleshooting

### Tests Are Flaky

Detect with the same zero-retry profile CI uses, repeated:
```bash
for i in {1..5}; do cargo nextest run --lib --profile unit || break; done
```
Then **fix the cause** — do not raise the unit profile's retries. See the
[Flaky-test quarantine policy](#flaky-test-quarantine-policy) above: the usual
culprits are shared global state, temp-dir collisions, a second ProximaDB boot
in one process, or a timing assumption.

### Tests Are Slow

Use `--test-threads=1` to avoid port conflicts:
```bash
cargo test --lib -- --test-threads=1
```

### Can't Run Tests

Check port conflicts:
```bash
lsof -i :5678 | kill -9 $(lsof -t -i :5678)  # Kill REST server
lsof -i :5679 | kill -9 $(lsof -t -i :5679)  # Kill gRPC server
```

Clean build artifacts:
```bash
make clean
cargo build
```

## Examples

See these files for complete TDD examples:
- `tests/tdd/test_utils/mod.rs` - Test utilities usage
- `src/core/search/hybrid/tests/fusion_test.rs` - Fusion strategy tests
- `clients/python/tests/tdd/__init__.py` - Python SDK tests

## Resources

- [Effective Testing with Rust](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [TDD Best Practices](https://martinfowler.com/bliki/TestPyramid.html)
- [ProximaDB CLAUDE.md](../../../CLAUDE.md) - Project instructions

## Questions?

Ask in #development channel or check existing tests for patterns.
