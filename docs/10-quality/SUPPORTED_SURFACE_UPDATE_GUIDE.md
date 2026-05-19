# Supported Surface Update Guide

This document describes how to update the SUPPORTED_SURFACE.md document when capabilities change.

## When to Update

Update SUPPORTED_SURFACE.md when:
1. New capabilities are added to an engine
2. Capabilities are removed (requires major version bump)
3. New engines are added
4. Experimental engines graduate to stable
5. API endpoints are added/removed

## Update Process

### 1. Update Engine Capabilities

Modify the engine's capability declaration in `src/storage/engines/factory.rs` or the engine implementation:

```rust
impl UnifiedStorageEngine for SstEngine {
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            // Add new capabilities here
        ])
    }
}
```

### 2. Run Contract Tests

Verify the engine actually supports the claimed capabilities:

```bash
cargo test --test capability_contract_test
```

### 3. Generate Snapshots

Generate updated capability snapshots:

```bash
./scripts/generate_capability_snapshots.sh
```

This creates/updates JSON files in `snapshots/capabilities/`:
- `sst.json`
- `viper.json`
- `helix.json`
- `nova.json`
- `swift.json`
- `raptor.json`

### 4. Update SUPPORTED_SURFACE.md

Review and manually update `SUPPORTED_SURFACE.md`:

1. **Update engine sections** - Add/remove capabilities in the appropriate engine section
2. **Update feature matrix** - Add/remove checkmarks in the feature matrix table
3. **Update version compatibility** - Add new versions to the version compatibility table
4. **Update unsupported features** - Move graduated features from unsupported to supported

### 5. Commit Changes

Commit the updates together:

```bash
git add snapshots/capabilities/
git add SUPPORTED_SURFACE.md
git commit -m "docs: Update supported surface for [feature]

- Added [capability] to [engine]
- Updated SUPPORTED_SURFACE.md
- Regenerated capability snapshots

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

## Validation

The CI pipeline will automatically:
1. Run capability contract tests
2. Check snapshot consistency
3. Verify SUPPORTED_SURFACE.md is updated

If the CI fails:
- Contract test failed: Engine doesn't actually support the claimed capability
- Snapshot drift: Run `./scripts/generate_capability_snapshots.sh` and commit
- Documentation check: Update SUPPORTED_SURFACE.md

## Example: Adding a New Capability

### Scenario: Add `FullTextSearch` capability to SST engine

**Step 1**: Update engine capabilities

```rust
// src/storage/engines/impls/sst/mod.rs
fn capabilities(&self) -> CapabilitySet {
    CapabilitySet::from_capabilities(&[
        Capability::Scan,
        Capability::Filter,
        Capability::FullTextSearch,  // NEW
        // ... other capabilities
    ])
}
```

**Step 2**: Add contract test

```rust
// tests/query/capability_contract_test.rs
async fn test_sst_fulltext_search_contract() {
    let engine = create_sst(&Default::default()).await.unwrap();
    
    // Test that full-text search actually works
    let collection = create_test_collection_with_fulltext(&engine).await;
    let results = search_fulltext(&engine, &collection, "search query").await;
    
    assert!(results.len() > 0, "Full-text search should return results");
}
```

**Step 3**: Run tests

```bash
cargo test --test capability_contract_test test_sst_fulltext_search_contract
```

**Step 4**: Generate snapshots

```bash
./scripts/generate_capability_snapshots.sh
```

**Step 5**: Update SUPPORTED_SURFACE.md

Add to SST engine section:
```markdown
### SST (Scalar Sorted Table)

**Supported Capabilities**:
- ✅ Scan
- ✅ Filter
- ✅ FullTextSearch  # NEW
- ...
```

Update feature matrix:
```markdown
| Feature | SST | VIPER | HELIX | NOVA |
|---------|-----|-------|-------|------|
| **Indexes** |
| Full-Text | ✅ | ❌ | ✅ | ❌ |  # NEW
```

**Step 6**: Commit

```bash
git add src/storage/engines/impls/sst/mod.rs
git add tests/query/capability_contract_test.rs
git add snapshots/capabilities/sst.json
git add SUPPORTED_SURFACE.md
git commit -m "feat: Add full-text search support to SST engine"
```

## CI Integration

The `.github/workflows/capability-contract-tests.yml` workflow automatically:

1. **On every PR**:
   - Runs capability contract tests
   - Checks snapshot consistency
   - Validates SUPPORTED_SURFACE.md updates

2. **On main branch push**:
   - Generates and uploads capability matrix artifact
   - Archives snapshot for version tracking

## Troubleshooting

### "Capability snapshots have changed" Error

**Problem**: CI detects snapshot drift  
**Solution**: Run `./scripts/generate_capability_snapshots.sh` and commit the changes

### Contract Test Failed

**Problem**: Engine claims capability it doesn't support  
**Solution**: Either remove the capability claim or implement the capability

### "SUPPORTED_SURFACE.md should be updated" Warning

**Problem**: Documentation doesn't match code  
**Solution**: Update SUPPORTED_SURFACE.md to reflect the new capabilities

## Best Practices

1. **Test Before Claiming**: Always implement and test capabilities before claiming them
2. **Update Documentation**: Keep SUPPORTED_SURFACE.md in sync with code changes
3. **Version Carefully**: Removing capabilities requires a major version bump
4. **Experimental Features**: Mark experimental capabilities clearly
5. **Cross-Reference**: Ensure API docs, SUPPORTED_SURFACE.md, and code all agree

## Automated Update Script (Future)

For future automation, consider creating a script that:

```bash
#!/bin/bash
# scripts/update_supported_surface.sh

# 1. Run contract tests
cargo test --test capability_contract_test

# 2. Generate snapshots
./scripts/generate_capability_snapshots.sh

# 3. Extract capability matrix
cargo test --test capability_contract_test generate_capability_matrix -- --nocapture > capability-matrix.txt

# 4. Update SUPPORTED_SURFACE.md from template
# (TODO: Implement template-based generation)

echo "✅ Supported surface updated successfully"
```

---

*Last Updated: April 2, 2026*
