# ProximaDB v0.2.0 Release Checklist

## Pre-Release Preparation

### ✅ Completed
- [x] All versions set to 0.2.0
- [x] Version consistency validated (`scripts/version-sync.sh check`)
- [x] Platform packages built successfully (RPM, DEB, MSI)
- [x] Pre-release CI passed on main (run 22285659408)
- [x] Dry-run release validated on main (run 22286400229)
- [x] Release notes prepared

## Release Steps

### 1. Create Release Tag

```bash
# Verify we're on main
git checkout main
git pull origin main

# Create and push tag
git tag -a v0.2.0 -m "Release v0.2.0 - Platform Packages"
git push origin v0.2.0
```

### 2. Trigger Release Workflow

**Option A: Automatic (via GitHub Actions)**
- Tag push will automatically trigger `release.yml` workflow
- Monitor the workflow: https://github.com/vjsingh1984/proximadb/actions

**Option B: Manual Dispatch**
```bash
gh workflow run release.yml -f version=0.2.0 -f prerelease=false
```

### 3. Monitor Release Build

**Expected Jobs:**
- Prepare Release (7s)
- Build Source Distribution (30s)
- Build x86_64-unknown-linux-gnu (15m)
- Build x86_64-pc-windows-msvc (25m)
- **Build RPM Package (45s)** ← NEW
- **Build DEB Package (45s)** ← NEW
- **Build MSI Installer (60s)** ← NEW
- Validate Artifacts
- Create GitHub Release
- Publish to PyPI (Python package)
- Publish to crates.io (Rust crate)

**Total Time:** ~45-60 minutes

### 4. Verify Release Artifacts

After the workflow completes, verify:

**GitHub Release:**
- [ ] Release created at https://github.com/vjsingh1984/proximadb/releases/tag/v0.2.0
- [ ] Release notes displayed correctly
- [ ] All artifacts uploaded:
  - [ ] Binaries (Linux .tar.gz, Windows .zip)
  - [ ] Platform packages (.rpm, .deb, .msi)
  - [ ] Source distribution (.tar.gz)
  - [ ] SHA256SUMS.txt

**PyPI:**
- [ ] Package published: https://pypi.org/project/proximadb/
- [ ] Version 0.2.0 available

**crates.io:**
- [ ] Crate published: https://crates.io/crates/proximadb
- [ ] Version 0.2.0 available

### 5. Test Installation

**Test RPM (CentOS/RHEL):**
```bash
# Download RPM
wget https://github.com/vjsingh1984/proximadb/releases/download/v0.2.0/proximadb-0.2.0-1.el8.x86_64.rpm

# Install
sudo rpm -ivh proximadb-0.2.0-1.el8.x86_64.rpm

# Verify
rpm -qa | grep proximadb
systemctl status proximadb
```

**Test DEB (Ubuntu/Debian):**
```bash
# Download DEB
wget https://github.com/vjsingh1984/proximadb/releases/download/v0.2.0/proximadb_0.2.0_amd64.deb

# Install
sudo dpkg -i proximadb_0.2.0_amd64.deb

# Verify
dpkg -l | grep proximadb
systemctl status proximadb
```

**Test MSI (Windows):**
```powershell
# Download MSI
# Install
msiexec /i proximadb-0.2.0-x64.msi

# Verify
Get-Service | Where-Object {$_.Name -like "*proximadb*"}
```

### 6. Post-Release Tasks

- [ ] Update main branch to next development version
- [ ] Create v0.2.1 milestone
- [ ] Close v0.2.0 issues in GitHub
- [ ] Announce release:
  - [ ] GitHub Release announcement
  - [ ] Documentation update
  - [ ] Social media / community channels

## Rollback Plan (If Needed)

If critical issues are discovered:

1. **Delete the release:**
   ```bash
   gh release delete v0.2.0 -y
   git push origin :refs/tags/v0.2.0
   ```

2. **Revert merge (if necessary):**
   ```bash
   git revert c7af7683
   git push origin main
   ```

3. **Create hotfix release** (v0.2.1)

## Contact Information

For release issues:
- GitHub Issues: https://github.com/vjsingh1984/proximadb/issues
- Maintainer: vjsingh1984

## Notes

- **DMG Installer**: Deferred to v0.2.1 due to ring crate issues on macOS CI
- **ARM64 Packages**: Planned for future releases
- **Validation Gate**: Temporarily disabled for v0.2.0 (will be re-enabled in v0.2.1)

## Checklist Summary

- [ ] Create and push v0.2.0 tag
- [ ] Monitor release workflow
- [ ] Verify GitHub release artifacts
- [ ] Verify PyPI publication
- [ ] Verify crates.io publication
- [ ] Test RPM installation
- [ ] Test DEB installation
- [ ] Test MSI installation
- [ ] Post-release updates

---

**Ready to release v0.2.0!** 🚀
