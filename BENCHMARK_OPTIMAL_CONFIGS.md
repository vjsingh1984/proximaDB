# Benchmark Optimal Configurations - Engine-Specific Guide

**Created**: 2025-12-18
**Purpose**: Document optimal configurations for exact vs approximate modes per engine

---

## Critical Finding: Block Pruning Only Works on Specific Engines

### ✅ Engines with Block Pruning Support (SST-style)

| Engine | Architecture | Block Structure | Pruning Method |
|--------|--------------|-----------------|----------------|
| **SST** | LSM-tree with ProximaBlocks | Block-level centroids | Centroid distance (sqrt mode) |
| **SWIFT** | Superblock-cached SST | Block-level centroids | Centroid distance (sqrt mode) |
| **HELIX** | Hilbert curve + LSM | ProximaBlocks with Hilbert keys | Hilbert range + centroid distance |

### ❌ Engines WITHOUT Block Pruning (Columnar/Parquet-based)

| Engine | Architecture | Why No Block Pruning |
|--------|--------------|----------------------|
| **VIPER** | Columnar Parquet | Uses row-group statistics, not block centroids |
| **NOVA** | Progressive columnar | Zone maps, not centroid-based |
| **RAPTOR** | Adaptive row-group | Dynamic row-groups, not fixed blocks |

---

## Configuration Matrix

### SST Engine Configuration

**From `src/core/config.rs:818-913`**:

```rust
pub struct SstConfig {
    pub block_size_kb: u32,  // KEY PARAMETER for block pruning
    pub bloom_filter_config: Option<BloomFilterConfig>,
    pub compression: String,
    pub compression_level: i32,
    // ... other fields
}
```

#### Exact Mode (100% recall, minimal overhead)
```rust
SstConfig {
    block_size_kb: 4096,  // 4MB - LARGE blocks (fewer blocks, less overhead)
    bloom_filter_config: Some(BloomFilterConfig {
        target_fpp: 0.01,  // 1% FPP (standard)
        enabled: true,
    }),
    compression: "lz4",  // Fast compression
    compression_level: 1,  // Low compression (speed over size)
}
```

**Rationale**:
- **Large blocks (4MB)**: Fewer blocks = less metadata overhead
- **Standard bloom filter**: For ID lookups, not critical for full scan
- **Fast compression**: LZ4 level 1 prioritizes speed

#### Approximate Mode (90-95% recall, optimize for pruning)
```rust
SstConfig {
    block_size_kb: 256,  // 256KB - SMALL blocks (more blocks, better granularity)
    bloom_filter_config: Some(BloomFilterConfig {
        target_fpp: 0.005,  // 0.5% FPP (better accuracy for pruning)
        enabled: true,
    }),
    compression: "zstd",  // Better compression
    compression_level: 3,  // Balanced compression
}
```

**Rationale**:
- **Small blocks (256KB)**: More blocks = finer granularity for centroid-based pruning
- **Better bloom filter**: Lower FPP helps skip more blocks accurately
- **Better compression**: ZSTD level 3 for better compression ratios

**Example**:
```
40K vectors, 768D, 40KB per vector

Exact Mode (4MB blocks):
  - 40KB × 40,000 = ~1.5GB total
  - 1.5GB / 4MB = ~375 blocks
  - Sqrt pruning: sqrt(375) = ~19 blocks selected
  - Granularity: Coarse (each block = ~107 vectors)

Approximate Mode (256KB blocks):
  - 1.5GB / 256KB = ~6,000 blocks
  - Sqrt pruning: sqrt(6000) = ~77 blocks selected
  - Granularity: Fine (each block = ~7 vectors)
```

---

### HELIX Engine Configuration

**From `src/storage/engines/impls/helix/mod.rs:139-200`**:

```rust
pub struct HelixConfig {
    pub proxima_block_size: usize,  // KEY: Vectors per block
    pub bloom_filter_bits_per_key: u32,
    pub hilbert_bits_per_dimension: usize,
    pub enable_liquid_clustering: bool,
    pub storage_quantization: bool,
    // ... other fields
}
```

#### Exact Mode (100% recall)
```rust
HelixConfig {
    proxima_block_size: 512,  // LARGE blocks (512 vectors per block)
    bloom_filter_bits_per_key: 10,  // Standard (1% FPP)
    hilbert_bits_per_dimension: 16,  // Standard resolution
    enable_liquid_clustering: false,  // Disable for exact mode
    storage_quantization: false,  // No quantization in exact mode
}
```

**Rationale**:
- **Large proxima blocks**: 512 vectors per block = fewer blocks
- **No clustering**: Exact mode doesn't benefit from clustering
- **No quantization**: Full precision for 100% accuracy

#### Approximate Mode (90-95% recall, optimize for Hilbert pruning)
```rust
HelixConfig {
    proxima_block_size: 64,  // SMALL blocks (64 vectors per block)
    bloom_filter_bits_per_key: 12,  // Better FPP (0.5%)
    hilbert_bits_per_dimension: 20,  // Higher resolution for better clustering
    enable_liquid_clustering: true,  // Enable for adaptive clustering
    storage_quantization: true,  // Use quantization for speed
}
```

**Rationale**:
- **Small proxima blocks**: 64 vectors per block = more blocks for fine-grained pruning
- **Higher resolution Hilbert**: Better spatial locality
- **Quantization enabled**: Faster distance calculations

**Example**:
```
40K vectors, 768D

Exact Mode (512 vectors/block):
  - 40,000 / 512 = ~78 blocks
  - Sqrt pruning: sqrt(78) = ~9 blocks selected
  - Granularity: Coarse (512 vectors per block)

Approximate Mode (64 vectors/block):
  - 40,000 / 64 = ~625 blocks
  - Sqrt pruning: sqrt(625) = ~25 blocks selected
  - Granularity: Fine (64 vectors per block)
```

---

### SWIFT Engine Configuration

**SWIFT uses SST-style blocks but with superblock caching**

#### Exact Mode
```rust
SwiftConfig {
    block_size_kb: 2048,  // 2MB (medium-large blocks)
    superblock_cache_size_mb: 512,  // Cache for frequent access
    compression: "snappy",  // Ultra-fast compression
}
```

#### Approximate Mode
```rust
SwiftConfig {
    block_size_kb: 512,  // 512KB (smaller blocks for pruning)
    superblock_cache_size_mb: 256,  // Smaller cache (more blocks)
    compression: "lz4",  // Balanced speed
}
```

---

### VIPER/NOVA/RAPTOR (Columnar Engines)

**These engines do NOT support block pruning!**

They use different optimization strategies:
- **VIPER**: Parquet row-group statistics, predicate pushdown
- **NOVA**: Zone maps for range pruning
- **RAPTOR**: Adaptive row-group sizing

**Configuration Impact**: Minimal difference between exact and approximate modes.

For these engines, `search_mode="approximate"` only affects **partition-level pruning**, NOT block pruning.

---

## Python Benchmark Modifications

### Current Issue

The benchmark creates collections like this:

```python
db.create_collection(collection_name, dimension, engine)
```

**Problem**: Uses default configurations for all engines, regardless of search mode.

### Solution: Engine-Specific Configuration

We need to modify the benchmark to pass engine-specific configs:

```python
def create_collection_optimized(db, collection_name, dimension, engine, search_mode):
    """Create collection with optimal config for search mode."""

    if engine.lower() in ['sst', 'swift', 'helix']:
        # Block-pruning engines: different configs per mode
        if search_mode == "approximate":
            if engine.lower() == 'sst' or engine.lower() == 'swift':
                config = {
                    'block_size_kb': 256,  # Small blocks for pruning
                    'compression': 'zstd',
                    'compression_level': 3,
                    'bloom_fpp': 0.005,
                }
            elif engine.lower() == 'helix':
                config = {
                    'proxima_block_size': 64,  # Small blocks
                    'bloom_filter_bits_per_key': 12,
                    'hilbert_bits_per_dimension': 20,
                    'enable_liquid_clustering': True,
                    'storage_quantization': True,
                }
        else:  # exact mode
            if engine.lower() == 'sst' or engine.lower() == 'swift':
                config = {
                    'block_size_kb': 4096,  # Large blocks
                    'compression': 'lz4',
                    'compression_level': 1,
                    'bloom_fpp': 0.01,
                }
            elif engine.lower() == 'helix':
                config = {
                    'proxima_block_size': 512,  # Large blocks
                    'bloom_filter_bits_per_key': 10,
                    'hilbert_bits_per_dimension': 16,
                    'enable_liquid_clustering': False,
                    'storage_quantization': False,
                }

        # Create with config (API needs to be exposed)
        db.create_collection_with_config(collection_name, dimension, engine, config)
    else:
        # Columnar engines: no block pruning, use default
        db.create_collection(collection_name, dimension, engine)
```

---

## Configuration Exposure Status

### Currently Exposed in Python

✅ **Basic creation**: `db.create_collection(name, dimension, engine)`

### NOT Currently Exposed (Needs Implementation)

❌ **Engine-specific configs**: `block_size_kb`, `proxima_block_size`, etc.
❌ **Compression settings**: Per-engine compression configuration
❌ **Bloom filter tuning**: FPP, bits per key

### Required Changes

**1. Add to Python bindings** (`src/embedded/python.rs`):

```rust
#[pyo3(signature = (name, dimension, engine=None, config=None))]
fn create_collection(
    &self,
    name: &str,
    dimension: u32,
    engine: Option<&str>,
    config: Option<PyDict>,  // NEW: Accept config dict
) -> PyResult<()> {
    // Parse config dict into engine-specific config
    // ...
}
```

**2. Add to embedded module** (`src/embedded/mod.rs`):

```rust
pub fn create_collection_with_config(
    &self,
    name: &str,
    dimension: u32,
    engine: &str,
    config: HashMap<String, Value>,
) -> Result<()> {
    // Convert config to engine-specific struct
    // ...
}
```

---

## Expected Performance Impact

### SST Engine with Optimal Configs

| Mode | Block Size | Num Blocks | Pruned Blocks | Vectors Scanned | Query Time |
|------|------------|------------|---------------|-----------------|------------|
| **Exact** (4MB) | 4MB | 375 | 0 | 40,000 | ~45ms |
| **Approx** (256KB) | 256KB | 6,000 | 5,923 (98.7%) | ~539 | **~3-5ms** |

**Speedup**: 9-15x faster with optimal config!

### HELIX Engine with Optimal Configs

| Mode | Vectors/Block | Num Blocks | Pruned Blocks | Vectors Scanned | Query Time |
|------|---------------|------------|---------------|-----------------|------------|
| **Exact** (512) | 512 | 78 | 0 | 40,000 | ~45ms |
| **Approx** (64) | 64 | 625 | 600 (96%) | ~1,600 | **~6-8ms** |

**Speedup**: 6-8x faster with optimal config!

---

## Verification Checklist

### ✅ Correctly Configured

**Block-Pruning Engines (SST, SWIFT, HELIX)**:
- [ ] Exact mode: Large blocks (4MB or 512 vectors/block)
- [ ] Approximate mode: Small blocks (256KB or 64 vectors/block)
- [ ] Approximate mode: Better bloom filters (lower FPP)
- [ ] Approximate mode: Appropriate compression

**Columnar Engines (VIPER, NOVA, RAPTOR)**:
- [ ] Use default configurations (block size doesn't affect pruning)
- [ ] Same config for exact and approximate modes
- [ ] Approximate mode only affects partition pruning

### ✅ Logging Verification

Enable debug logs to verify:

```bash
RUST_LOG=debug,proximadb::storage::engines::impls::sst=trace \
cargo test -- --nocapture 2>&1 | grep -E "(block_size|blocks.*created|blocks.*pruned)"
```

**Expected**:
```
SST (Exact, 4MB blocks):
  - Created 375 blocks
  - Block size: 4096KB
  - Pruning: Not applicable (exact mode)

SST (Approx, 256KB blocks):
  - Created 6,000 blocks
  - Block size: 256KB
  - Pruning: Evaluating 6,000 blocks → Selected 77 blocks (98.7% pruned)
```

---

## Other Configuration Differences

### Compression Configuration

**Exact Mode**:
- **Algorithm**: LZ4 or Snappy (fastest)
- **Level**: 1 (minimal CPU)
- **Rationale**: Speed over compression ratio

**Approximate Mode**:
- **Algorithm**: ZSTD (better compression)
- **Level**: 3 (balanced)
- **Rationale**: Reading fewer blocks, compression ratio matters more

### Cache Configuration

**Exact Mode**:
- **Block cache**: Larger (1-2GB) - caching many blocks
- **Decompression cache**: Enabled
- **Rationale**: Many blocks accessed

**Approximate Mode**:
- **Block cache**: Smaller (256-512MB) - fewer blocks accessed
- **Decompression cache**: Less critical
- **Rationale**: Pruning reduces blocks accessed

### Bloom Filter Configuration

**Exact Mode**:
- **FPP**: 0.01 (1% - standard)
- **Bits per key**: 10
- **Rationale**: Used mainly for point queries

**Approximate Mode**:
- **FPP**: 0.005 (0.5% - better accuracy)
- **Bits per key**: 12
- **Rationale**: Helps skip blocks more accurately during pruning

---

## Immediate Actions Required

### 1. **Expose Engine Configs in Python** (Critical)

Without this, the benchmark can't use optimal configurations.

**Files to Modify**:
- `src/embedded/python.rs` - Add `config` parameter to `create_collection`
- `src/embedded/mod.rs` - Add config parsing logic
- `clients/python/src/proximadb_sdk/embedded.py` - Update Python SDK

### 2. **Update Benchmark to Use Optimal Configs** (Critical)

Modify `embedded_consolidated_benchmark.py`:
- Use different configs for exact vs approximate modes
- Only apply block size configs to SST/SWIFT/HELIX
- Use default configs for VIPER/NOVA/RAPTOR

### 3. **Document Which Engines Support What** (Important)

Update benchmark output to clarify:
```
--- PROXIMADB ENGINES (Approximate mode with sqrt block pruning) ---
Block pruning: ENABLED for SST, SWIFT, HELIX (centroid-based)
              DISABLED for VIPER, NOVA, RAPTOR (columnar engines use row-group stats)

SST-APPROX:    256KB blocks | 6,000 blocks created | ~98% pruned
HELIX-APPROX:  64 vec/block | 625 blocks created | ~96% pruned
VIPER-APPROX:  (row-group pruning only, not block-based)
```

### 4. **Add Configuration Verification** (Nice to Have)

Log the actual config being used:
```python
print(f"  Engine: {engine}")
print(f"  Block config: {config.get('block_size_kb', 'default')} KB")
print(f"  Compression: {config.get('compression', 'default')}")
print(f"  Search mode: {search_mode}")
```

---

## Summary

### Critical Findings

1. ✅ **Block pruning ONLY works on SST, SWIFT, HELIX** (centroid-based blocks)
2. ✅ **VIPER, NOVA, RAPTOR use different optimization** (row-group stats, zone maps)
3. ✅ **Block size configuration is CRITICAL for pruning effectiveness**
4. ❌ **Python SDK doesn't expose engine configs yet** (needs implementation)
5. ✅ **Different configs needed for exact vs approximate modes**

### Optimal Configuration Summary

| Engine | Exact Mode Block Size | Approx Mode Block Size | Supports Block Pruning |
|--------|----------------------|------------------------|------------------------|
| **SST** | 4MB (4096KB) | 256KB | ✅ YES (centroid-based) |
| **SWIFT** | 2MB (2048KB) | 512KB | ✅ YES (centroid-based) |
| **HELIX** | 512 vectors/block | 64 vectors/block | ✅ YES (centroid + Hilbert) |
| **VIPER** | N/A (default) | N/A (default) | ❌ NO (row-group stats) |
| **NOVA** | N/A (default) | N/A (default) | ❌ NO (zone maps) |
| **RAPTOR** | N/A (default) | N/A (default) | ❌ NO (adaptive row-groups) |

### Next Steps

1. **Implement config exposure in Python SDK** (required for optimal benchmarks)
2. **Update benchmark to use engine-specific configs**
3. **Clarify in output which engines use block pruning**
4. **Verify with debug logs that configs are being applied**

**Without exposing configs, the benchmark will use default settings which are not optimal for either exact or approximate modes.**
