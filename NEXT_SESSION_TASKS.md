# Next Session Priority Tasks

## 1. Fix Config Loading Issue (HIGH PRIORITY)

**Problem**: Server ignoring config.toml paths, using hardcoded defaults

**Evidence**:
```
Config says: /tmp/proximadb/data, /tmp/proximadb/metadata
Server uses: ./data, ./metadata
```

**Fix Location**: `src/bin/proximadb-server.rs`
- Check config loading code
- Ensure Config::from_file() properly reads all fields
- Remove hardcoded path fallbacks

## 2. Complete Python SDK Test Verification

**Status**: Fixes committed, verification pending

**Fixed**:
- API method names (search_vectors → search)
- Response object access (Pydantic model handling)
- Concurrent insertion count logic
- Removed /workspace hardcoded paths

**Verify**: Re-run pytest after config fix

## 3. Dependency Audit (REQUESTED)

**Tasks**:
```bash
cargo install cargo-udeps cargo-audit
cargo +nightly udeps  # Find unused dependencies
cargo audit           # Security vulnerabilities
```

**Review**: Cargo.toml for:
- Commented dependencies (remove or document)
- Duplicate dependencies (consolidate)
- Outdated versions (hyper 0.14 → 1.x)
- Unpinned versions (add consistency)

## 4. Performance Claims Final Audit

**10K Benchmark Extraction**: Complete manual verification of all 30 tests (61-90)

**Current Verified**:
- SST-LZ4: 5.34ms ✅
- HELIX-None: 13.51ms ✅

**Need Verification**:
- VIPER, NOVA, SWIFT, RAPTOR (all compressions)
- Update all docs with verified numbers only

## 5. Roadmap Update

**Add to MASTER_FEATURE_DASHBOARD**:
- Feature 122: Type-Safe Metadata Filtering ✅
- Feature 123: Performance Benchmarking ✅
- Feature 124: Block Size Optimization ✅
- Feature 125: Compression Optimization ✅

**Move Items**: Based on codebase verification

---

## Session Achievements (Already Committed)

✅ Type-safe metadata filtering (47/47 tests)
✅ Performance optimizations (validated)
✅ Documentation streamlined (2.5KB single source)
✅ Python SDK test fixes (committed)
✅ Benchmark corrections (10K verified)

**Git**: 39 commits, 143 files changed
**Branch**: development (all pushed)
