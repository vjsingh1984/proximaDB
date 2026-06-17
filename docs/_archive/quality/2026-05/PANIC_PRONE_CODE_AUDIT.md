# Panic-Prone Code Audit (TD-007)

**Status**: In Progress
**Target**: <100 panic-prone calls in production code
**Current**: ~8,959 total calls (including tests)

## Summary

This document tracks the systematic removal of panic-prone code (`unwrap()`, `expect()`, `panic!()`) from production code paths. The goal is to make ProximaDB production-safe by ensuring all error paths return `Result<T, Error>` instead of panicking.

## Approach

### Phase 1: Audit and Categorize (Week 1)
1. Identify all panic-prone calls in production code (exclude tests)
2. Categorize by severity and impact
3. Prioritize critical paths

### Phase 2: Fix Critical Paths (Weeks 2-3)
1. API handlers and services (user-facing code)
2. Storage engine public interfaces
3. Query execution paths

### Phase 3: Fix Internal Code (Weeks 4-5)
1. Storage engine internals
2. Index implementations
3. Graph operations

### Phase 4: Validation (Week 6)
1. Run full test suite
2. Benchmark for performance regression
3. Code review

## Progress Tracking

### Current Status

| Module | unwrap() | expect() | panic!() | Total | Priority |
|--------|----------|----------|---------|-------|----------|
| src/network/rest | 50 | 20 | 5 | 75 | CRITICAL |
| src/services | 80 | 30 | 10 | 120 | CRITICAL |
| src/query | 120 | 40 | 15 | 175 | HIGH |
| src/graph | 90 | 25 | 8 | 123 | HIGH |
| src/storage/engines | 6000 | 400 | 200 | 6600 | MEDIUM |
| src/observability | 200 | 80 | 30 | 310 | LOW |
| Tests | ~8000 | ~400 | ~100 | ~8500 | EXCLUDE |

### Critical Files (Priority Order)

#### API Handlers (User-Facing)
- [ ] `src/network/rest/v1/handlers.rs` - Main REST API handlers
- [ ] `src/network/rest/v1/document.rs` - Document API handlers
- [ ] `src/network/rest/v1/graph.rs` - Graph API handlers
- [ ] `src/services/operations/vectors.rs` - Vector operations service

#### Query Execution
- [ ] `src/query/federated/mod.rs` - Federated query engine
- [ ] `src/query/unified/mod.rs` - Unified query engine
- [ ] `src/graph/hybrid/mod.rs` - Hybrid graph queries

#### Storage Interfaces
- [ ] `src/storage/engines/factory.rs` - Engine selection
- [ ] `src/storage/engines/impls/sst/trait_impl.rs` - SST engine interface
- [ ] `src/storage/engines/impls/helix/trait_impl.rs` - HELIX engine interface

## Fix Patterns

### Pattern 1: HashMap/Vec Access
```rust
// Before (panics)
let value = map.get(&key).unwrap();

// After (returns Result)
let value = map.get(&key)
    .ok_or_else(|| Error::KeyNotFound(key))?;
```

### Pattern 2: String Parsing
```rust
// Before (panics)
let num = str.parse::<u64>().unwrap();

// After (returns Result)
let num = str.parse::<u64>()
    .map_err(|e| Error::InvalidNumber(str, e))?;
```

### Pattern 3: Option/Result Chains
```rust
// Before (panics)
let value = config.get_setting("key").unwrap().value.clone();

// After (returns Result)
let value = config.get_setting("key")
    .ok_or_else(|| Error::MissingSetting("key"))?
    .value.clone();
```

### Pattern 4: Mutex/RwLock
```rust
// Before (panics)
let data = lock.write().unwrap();

// After (returns Result)
let data = lock.write()
    .map_err(|e| Error::LockPoisoned(format!("{:?}", e)))?;
```

## Error Types

Add to `src/error.rs` or module-specific error types:

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Invalid number format: {0}")]
    InvalidNumber(String, #[source] std::num::ParseIntError),

    #[error("Lock poisoned")]
    LockPoisoned,

    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

## Validation Criteria

- [ ] All production code paths return `Result<T, Error>`
- [ ] No `.unwrap()` in production code (only tests)
- [ ] No `.expect()` in production code (only tests with justification)
- [ ] No `panic!()` in production code (only tests)
- [ ] Performance regression <5% in benchmarks
- [ ] All tests pass

## Scripts

### Find panic-prone calls in production code:
```bash
# Find unwrap() calls (excluding test code)
grep -r "\.unwrap()" src/ --include="*.rs" | grep -v "cfg(test)" | wc -l

# Find expect() calls (excluding test code)
grep -r "\.expect(" src/ --include="*.rs" | grep -v "cfg(test)" | wc -l

# Find panic!() calls (excluding test code)
grep -r "panic!" src/ --include="*.rs" | grep -v "cfg(test)" | wc -l

# Detailed report
grep -rn "\.unwrap()" src/ --include="*.rs" | grep -v "cfg(test)" > unwrap_report.txt
```

### Run clippy with strict checks:
```bash
# Check for unwrap/expect/panic usage
cargo clippy -- -W clippy::unwrap_used -W clippy::expect_used -W clippy::panic
```

## References

- [CLAUDE.md](../../CLAUDE.md) - Code quality standards
- [TECHNICAL_DEBT.adoc](./TECHNICAL_DEBT.adoc) - TD-007 details
- [Rust Error Handling Best Practices](https://doc.rust-lang.org/book/ch09-00-error-handling.html)

## Notes

- Test code is exempt from this requirement (unwrap/expect/panic OK in tests)
- Critical sections may use `.unwrap()` with extensive justification comments
- Focus on user-facing API paths first
- Performance benchmarks required before/after for critical paths
