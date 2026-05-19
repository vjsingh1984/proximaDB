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
- **Don't use unwrap() in tests** - Tests should use `?` or `expect()` with messages
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

Tests run automatically on:
- **Push to main**: Full test suite
- **Pull requests**: Full test suite + coverage
- **Feature branches**: Unit tests only (faster feedback)

### GitHub Actions Workflows

- `.github/workflows/tdd.yml` - Main TDD test suite
- Runs: Unit tests, integration tests, Python SDK tests
- Enforces: Formatting, linting, coverage thresholds

### Pre-commit Hook

Automatically runs before each commit:
```bash
make install-tdd-hooks  # Install once
git commit              # Hook runs automatically
```

## Coverage Goals

| Module | Target | Current |
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

Run tests multiple times to detect flakiness:
```bash
for i in {1..3}; do
  make test-unit
done
```

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
- [ProximaDB CLAUDE.md](../../CLAUDE.md) - Project instructions

## Questions?

Ask in #development channel or check existing tests for patterns.
