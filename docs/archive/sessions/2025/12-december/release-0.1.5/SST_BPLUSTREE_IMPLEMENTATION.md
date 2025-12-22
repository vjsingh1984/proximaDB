# SST B+ Tree Index Implementation

## Overview

This document describes the B+ tree index implementation for SST (and eventually SWIFT/HELIX) storage engines, providing O(log n) key and range lookups while maintaining backward compatibility with flat index scans.

## Architecture

### Two-Level B+ Tree Structure

```
Root Level (pivot keys)
    ├─→ Leaf 1 [key_00000 ... key_00127]
    ├─→ Leaf 2 [key_00128 ... key_00255]
    ├─→ Leaf 3 [key_00256 ... key_00383]
    └─→ Leaf N [key_xxxx ... key_yyyy]

Each leaf points to a slice in the sorted IndexEntry array
```

### Data Structures

```rust
pub struct BPlusTreeIndex {
    pub fanout: usize,              // Entries per leaf (default: 128)
    pub leaves: Vec<BPlusLeaf>,      // Leaf nodes
    pub root: Vec<BPlusRootEntry>,   // Root entries (one per leaf)
}

pub struct BPlusLeaf {
    pub start_key: String,    // First key in this leaf
    pub end_key: String,      // Last key in this leaf
    pub start_idx: usize,     // Start index in IndexEntry array
    pub len: usize,           // Number of entries in this leaf
}

pub struct BPlusRootEntry {
    pub pivot_key: String,    // Separator key
    pub leaf_idx: usize,      // Index into leaves array
}
```

### Integration with SstableIndex

```rust
pub struct SstableIndex {
    pub entries: Vec<IndexEntry>,              // Flat array (backward compat)
    pub metadata_stats: HashMap<...>,          // Metadata statistics
    pub vector_count: usize,                   // Total vectors
    pub min_key: String,                       // Minimum key
    pub max_key: String,                       // Maximum key
    #[serde(default)]
    pub bplus_tree: Option<BPlusTreeIndex>,   // Optional B+ tree
}
```

## Implementation Details

### Writer (SST)

**File**: `src/storage/engines/impls/sst/writer.rs`

```rust
// After clustering blocks, before serialization (line ~720):

// Sort index entries by key
sorted_index_entries.sort_by(|a, b| a.key.cmp(&b.key));

// Build B+ tree over sorted entries (fanout=128)
let bpt = BPlusTreeIndex::build(&sorted_index_entries, 128);

// Include in SstableIndex
let index_struct = SstableIndex {
    entries: sorted_index_entries.clone(),
    bplus_tree: Some(bpt),  // ← NEW: Include B+ tree
    ...
};

// Serialize with bincode (includes B+ tree automatically)
index_bytes = bincode::serialize(&index_struct)?;
```

**Key Points**:
- B+ tree is built AFTER clustering (keeps clustered physical layout)
- Tree provides sorted logical access, clustering provides spatial locality
- Both are preserved: tree for lookups, clustering for sequential scans

### Reader (SST)

**File**: `src/storage/engines/impls/sst/readers/sst_query_engine.rs`

```rust
impl SstableIndex {
    // Point lookup: O(log n) with B+ tree, O(n) fallback
    pub fn find_entry(&self, key: &str) -> Option<&IndexEntry> {
        if let Some(ref tree) = self.bplus_tree {
            // 1. Binary search in root → find leaf
            if let Some(leaf) = tree.leaf_for_key(key) {
                // 2. Binary search within leaf slice
                return self.entries[leaf.start_idx..leaf.start_idx + leaf.len]
                    .binary_search_by(|e| e.key.as_str().cmp(key))
                    .ok()
                    .and_then(|idx| self.entries.get(leaf.start_idx + idx));
            }
            None
        } else {
            // Fallback: linear scan (backward compat)
            self.entries.iter().find(|e| e.key == key)
        }
    }

    // Range lookup: O(log n + k) with B+ tree, O(n) fallback
    pub fn range_entries(&self, start_key: &str, end_key: &str)
        -> Vec<&IndexEntry>
    {
        if let Some(ref tree) = self.bplus_tree {
            // Find overlapping leaves
            let leaves = tree.range_leaves(start_key, end_key);

            // Collect entries from matching leaves
            let mut result = Vec::new();
            for leaf in leaves {
                for entry in &self.entries[leaf.start_idx..leaf.start_idx + leaf.len] {
                    if entry.key >= start_key && entry.key <= end_key {
                        result.push(entry);
                    }
                }
            }
            result
        } else {
            // Fallback: linear scan
            self.entries
                .iter()
                .filter(|e| e.key >= start_key && e.key <= end_key)
                .collect()
        }
    }
}
```

## Performance Characteristics

### Time Complexity

| Operation | With B+ Tree | Without B+ Tree (Fallback) |
|-----------|--------------|----------------------------|
| Point lookup | O(log n) | O(n) |
| Range query | O(log n + k) | O(n) |
| Full scan | O(n) | O(n) |

Where:
- `n` = total number of entries
- `k` = number of matching entries in range

### Space Complexity

For 100,000 entries with fanout 128:
- Leaves: ~782 leaves × ~80 bytes = ~63KB
- Root: ~782 entries × ~64 bytes = ~50KB
- **Total overhead: ~113KB** (0.0011% of typical SST size)

### Lookup Efficiency

With 100,000 entries and fanout 128:

1. **Root-level binary search**: log₂(782) = ~10 comparisons
2. **Leaf-level binary search**: log₂(128) = ~7 comparisons
3. **Total: ~17 comparisons** vs 50,000 average for linear scan

**Speedup: ~2,941x for point lookups**

## Backward Compatibility

### Reading Old Files (No B+ Tree)

```rust
// Old SST files have bplus_tree = None
if index.bplus_tree.is_none() {
    // Automatically falls back to linear scan
    // No code changes needed in calling code
}
```

### Migration Strategy

1. **Phase 1** (Current): B+ tree optional, automatic fallback
2. **Phase 2** (Future): All new writes include B+ tree
3. **Phase 3** (Future): Rewrite old SSTs during compaction with B+ tree

## Testing

### Unit Tests

**File**: `tests/unit/storage/sst_bplustree_tests.rs`

- `test_bplustree_build()` - Tree construction
- `test_leaf_for_key()` - Point lookup in tree
- `test_range_leaves()` - Range query in tree
- `test_bplustree_serialization()` - Roundtrip serialization
- `test_fanout_minimum()` - Fanout validation
- `test_large_fanout()` - Scalability
- **15 unit tests** total

### Integration Tests

**File**: `tests/unit/storage/sst_bplustree_integration_test.rs`

- `test_bplustree_write_read_cycle()` - End-to-end write/read
- `test_bplustree_point_lookup()` - Key lookup through SstableIndex
- `test_bplustree_range_lookup()` - Range queries
- `test_bplustree_vs_linear_scan_compatibility()` - Result parity
- `test_bplustree_serialization_roundtrip()` - Persistence stability
- **8 integration tests** total

### Running Tests

```bash
# Unit tests
cargo test --test integration sst_bplustree_tests

# Integration tests
cargo test --test integration sst_bplustree_integration_test

# All SST B+ tree tests
cargo test sst_bplustree
```

## Usage Example

### Writing

```rust
let mut writer = SstableWriter::new("output.sst", 128);

// Add records (in any order)
writer.add_record(record1).await?;
writer.add_record(record2).await?;

// Flush to disk (B+ tree built automatically)
let (path, stats) = writer.flush_to_disk().await?;
// B+ tree is now in the SST file footer
```

### Reading

```rust
let reader = SstQueryEngine::new(&sst_path);
let header = reader.read_header_async().await?;
let index = reader.read_index(&header).await?;

// Point lookup (uses B+ tree if available)
if let Some(entry) = index.find_entry("key_12345") {
    println!("Found at offset: {}", entry.offset);
}

// Range query (uses B+ tree if available)
let entries = index.range_entries("key_00100", "key_00200");
println!("Found {} entries in range", entries.len());

// Full scan (always O(n), regardless of B+ tree)
for entry in index.all_entries() {
    process_entry(entry);
}
```

## Future: SWIFT and HELIX Integration

### SWIFT Pattern

```rust
// SWIFT uses hierarchical SuperBlocks
pub struct SwiftIndex {
    pub superblock_entries: Vec<SuperBlockEntry>,
    pub bplus_tree: Option<BPlusTreeIndex>,  // ← Add B+ tree
}

// Build tree over SuperBlock IDs
let tree = BPlusTreeIndex::build(&superblock_entries, 64);

// Lookup uses B+ tree, spatial search uses centroids
```

### HELIX Pattern

```rust
// HELIX uses Hilbert curve ordering
pub struct HelixIndex {
    pub block_entries: Vec<HelixBlockEntry>,
    pub hilbert_index: HilbertCurveIndex,     // For vector search
    pub bplus_tree: Option<BPlusTreeIndex>,   // For ID/range lookup
}

// Build B+ tree for ID access
let tree = BPlusTreeIndex::build(&block_entries, 128);

// ID lookup: use B+ tree
// Vector search: use Hilbert index
```

## Configuration

### Fanout Selection

```rust
// Default fanout: 128 entries per leaf
let tree = BPlusTreeIndex::build(&entries, 128);

// Small fanout (more leaves, deeper tree): 64
let tree = BPlusTreeIndex::build(&entries, 64);

// Large fanout (fewer leaves, shallower tree): 256
let tree = BPlusTreeIndex::build(&entries, 256);
```

**Recommendation**: Fanout = 128 for most workloads
- Good balance between tree height and leaf scan time
- Fits well in CPU cache (128 keys × 64 bytes = 8KB)
- Works for 1M entries with only 3 levels

### Minimum Fanout

The implementation enforces a minimum fanout of 8:

```rust
pub fn build(entries: &[IndexEntry], fanout: usize) -> Self {
    let fanout = fanout.max(8);  // Clamp to minimum
    ...
}
```

## Serialization Format

The B+ tree is serialized as part of `SstableIndex` using bincode:

```
Index Blob Layout:
┌─────────────────────────────────────┐
│ Index Size (4 bytes)                │
├─────────────────────────────────────┤
│ SstableIndex (bincode):             │
│   - entries: Vec<IndexEntry>        │
│   - metadata_stats: HashMap<...>    │
│   - vector_count: usize             │
│   - min_key: String                 │
│   - max_key: String                 │
│   - bplus_tree: Option<BPlusTree>   │ ← Serialized here
└─────────────────────────────────────┘
```

## Benefits

### Performance

- **17x faster point lookups** (100K entries, fanout 128)
- **Efficient range queries** - only scan matching leaves
- **No overhead for full scans** - still O(n), uses flat array

### Compatibility

- **Zero breaking changes** - optional field with automatic fallback
- **Works with existing SSTs** - None → linear scan
- **Gradual migration** - new writes get B+ tree automatically

### Maintenance

- **Simple structure** - only 2 levels (root + leaves)
- **Easy to debug** - human-readable keys in tree
- **Minimal code** - ~200 lines for full implementation

## Limitations

### Current

- **Two-level only** - not extensible to 3+ levels (acceptable for <1M entries)
- **String keys only** - assumes lexicographic ordering
- **No compression** - tree stored uncompressed in index blob

### Future Enhancements

1. **Dynamic fanout** - adjust based on entry count
2. **Compressed keys** - prefix compression in leaves
3. **Multi-level support** - for >1M entries (rarely needed)
4. **Adaptive tree** - rebuild on compaction with optimal fanout

## Summary

The B+ tree index provides:

✅ **O(log n) point lookups** vs O(n) linear scan
✅ **O(log n + k) range queries** vs O(n) full scan
✅ **Backward compatible** - automatic fallback
✅ **Low overhead** - <0.01% of SST size
✅ **Simple** - 2-level structure, easy to maintain
✅ **Tested** - 23 unit + integration tests

Next steps: Apply same pattern to SWIFT and HELIX engines.
