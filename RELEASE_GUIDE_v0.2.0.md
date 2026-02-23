# ProximaDB v0.2.0 - Quick Release Guide

## TL;DR - Release Commands

```bash
# 1. Ensure we're on main and up to date
git checkout main && git pull origin main

# 2. Verify version consistency
bash scripts/version-sync.sh check

# 3. Create and push tag (this triggers the release!)
git tag -a v0.2.0 -m "Release v0.2.0 - Platform Packages"
git push origin v0.2.0

# 4. Monitor the release at:
# https://github.com/vjsingh1984/proximadb/actions
```

## What Happens Automatically

When you push the `v0.2.0` tag:

1. **GitHub Actions** triggers the `release.yml` workflow
2. **Builds** all binaries and platform packages:
   - Linux binary (15 min)
   - Windows binary (25 min)
   - RPM package (45 sec)
   - DEB package (45 sec)
   - MSI installer (60 sec)
3. **Validates** all artifacts
4. **Creates** GitHub Release with all artifacts
5. **Publishes** to PyPI (Python package)
6. **Publishes** to crates.io (Rust crate)

**Total time:** ~45-60 minutes

## Verification

After the workflow completes:

```bash
# Check release exists
gh release view v0.2.0

# Check PyPI
curl -s https://pypi.org/pypi/proximadb/json | jq .info.version

# Check crates.io
curl -s https://crates.io/api/v1/crates/proximadb | jq .crate.versions[-1].num
```

## Expected Release Artifacts

### Platform Packages (NEW!)
- `proximadb-0.2.0-1.el8.x86_64.rpm` (RHEL/CentOS/Fedora)
- `proximadb_0.2.0_amd64.deb` (Debian/Ubuntu)
- `proximadb-0.2.0-x64.msi` (Windows)

### Binaries
- `proximadb-0.2.0-x86_64-unknown-linux-gnu.tar.gz` (Linux)
- `proximadb-0.2.0-x86_64-pc-windows-msvc.zip` (Windows)

### Source
- Source distribution (.tar.gz)

## Key Links

- **Release Workflow:** https://github.com/vjsingh1984/proximDB/actions/workflows/release.yml
- **Pre-Release CI:** https://github.com/vjsingh1984/proximDB/actions/workflows/prerelease-ci.yml
- **Releases Page:** https://github.com/vjsingh1984/proximDB/releases

## Troubleshooting

**If the workflow fails:**
1. Check the Actions tab for error details
2. Fix the issue
3. Delete the tag: `git push origin :refs/tags/v0.2.0`
4. Create a new tag: `git tag -a v0.2.1 ...`

**If you need to delete the release:**
```bash
gh release delete v0.2.0 -y
git push origin :refs/tags/v0.2.0
```

## Success Criteria

✅ Release created on GitHub
✅ All platform packages built successfully
✅ PyPI published
✅ crates.io published
✅ Installation tests pass

---

**Current Status:** ✅ READY TO RELEASE

All validation completed:
- Version consistency: ✅
- Platform packages: ✅
- Pre-release CI: ✅
- Dry-run release: ✅

**Proceed with tagging when ready!**
