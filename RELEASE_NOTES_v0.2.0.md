# ProximaDB v0.2.0 Release Notes

**Release Date:** February 22, 2026

## Overview

ProximaDB v0.2.0 is a significant milestone that introduces **platform-specific packages** for Linux and Windows, making installation easier than ever. This release includes native package managers integration (RPM, DEB) and a Windows installer (MSI).

## 🎉 What's New

### Platform Packages

For the first time, ProximaDB v0.2.0 is available as native platform packages:

#### Linux

**RPM Package (Red Hat/CentOS/Fedora)**
```bash
sudo rpm -ivh proximadb-0.2.0-1.el8.x86_64.rpm
sudo systemctl start proximadb
```

**DEB Package (Debian/Ubuntu)**
```bash
sudo dpkg -i proximadb_0.2.0_amd64.deb
sudo systemctl start proximadb
```

Both Linux packages include:
- Systemd service integration
- Automatic user and directory creation
- Configuration file management
- Start/stop scripts

#### Windows

**MSI Installer**
```powershell
msiexec /i proximadb-0.2.0-x64.msi
```

The Windows installer includes:
- Installation to `C:\Program Files\ProximaDB\`
- Configuration in `C:\ProgramData\ProximaDB\`
- PATH environment variable setup
- Start menu shortcuts
- Add/Remove Programs entry

### Release Infrastructure

- ✅ Automated platform package builds
- ✅ Multi-platform binary support (Linux, Windows)
- ✅ Pre-release CI validation
- ✅ Dry-run release testing
- ✅ Version consistency automation

## 📦 Available Artifacts

### Binaries
- `proximadb-0.2.0-x86_64-unknown-linux-gnu.tar.gz` (Linux)
- `proximadb-0.2.0-x86_64-pc-windows-msvc.zip` (Windows)

### Platform Packages
- `proximadb-0.2.0-1.el8.x86_64.rpm` (RHEL/CentOS/Fedora)
- `proximadb_0.2.0_amd64.deb` (Debian/Ubuntu)
- `proximadb-0.2.0-x64.msi` (Windows)

### Source
- Source distribution (`.tar.gz`)

## 🔧 Installation

### From Binaries

Download and extract the binary for your platform:

**Linux:**
```bash
tar -xzf proximadb-0.2.0-x86_64-unknown-linux-gnu.tar.gz
cd proximadb-0.2.0-x86_64-unknown-linux-gnu
./proximadb-server --config config.toml
```

**Windows:**
```powershell
Expand-Archive proximadb-0.2.0-x86_64-pc-windows-msvc.zip
.\proximadb-0.2.0-x86_64-pc-windows-msvc\proximadb-server.exe --config config.toml
```

### From Package Managers

**RPM-based systems (RHEL, CentOS, Fedora):**
```bash
sudo rpm -ivh proximadb-0.2.0-1.el8.x86_64.rpm
sudo systemctl start proximadb
sudo systemctl enable proximadb
```

**DEB-based systems (Debian, Ubuntu):**
```bash
sudo dpkg -i proximadb_0.2.0_amd64.deb
sudo systemctl start proximadb
sudo systemctl enable proximadb
```

**Windows:**
```powershell
msiexec /i proximadb-0.2.0-x64.msi
# Or double-click the MSI file
```

### From Source

```bash
cargo install proximadb
```

## 📚 Documentation

- [Configuration Guide](../config/README.md)
- [API Documentation](../docs/api/README.md)
- [Architecture](../docs/concepts/architecture.adoc)

## 🐛 Known Limitations

- **macOS packages (DMG)**: Not available in v0.2.0 due to ring crate CPU feature detection issues on CI. Planned for v0.2.1.
- **Python embedded wheels**: Disabled for v0.2.0 (pure Python package). Use standard Python package instead.

## 🙏 Credits

This release includes contributions from the entire ProximaDB team, with special thanks to:
- All contributors to the platform packaging infrastructure
- CI/CD improvements and automation
- Testing and validation efforts

## 📝 Upgrade Notes

### From v0.1.x

If you're upgrading from v0.1.x:
1. Backup your data directory
2. Stop the old service
3. Install the new package
4. Review and update configuration if needed
5. Start the new service

### Configuration Changes

No breaking configuration changes in this release. Existing `config.toml` files should work without modification.

## 🔗 Links

- [GitHub Repository](https://github.com/vjsingh1984/proximadb)
- [Documentation](https://github.com/vjsingh1984/proximadb/tree/main/docs)
- [Issue Tracker](https://github.com/vjsingh1984/proximadb/issues)

## ⚠️ Important Notes

- This is a pre-release version. While platform packages have been validated, please test thoroughly in your environment.
- Platform packages are currently available for x86_64 (AMD64) architecture only.
- ARM64 packages planned for future releases.

---

**SHA256 Checksums**

All release artifacts include SHA256 checksums. Verify with:
```bash
sha256sum -c SHA256SUMS.txt
```

**GPG Signature**

Release artifacts are signed with the ProximaDB signing key.
