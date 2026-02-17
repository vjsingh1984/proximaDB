# Docker Build OOM Solutions

## Problem

Docker builds fail with `cannot allocate memory (signal: 9, SIGKILL)` during Rust compilation.

### Why This Happens

1. **Rust release builds are extremely memory-intensive**:
   - Linking with LTO (Link-Time Optimization) consumes massive RAM
   - opt-level=3 generates complex optimizations requiring lots of memory
   - Default codegen-units=6 creates parallel compilation spikes

2. **ProximaDB is a large project**:
   - 449+ compilation warnings
   - Many dependencies (Arrow, Parquet, Tokio, etc.)
   - Multiple features enabled by default

3. **Limited Docker environments**:
   - GitHub Actions: ~7GB RAM limit
   - Docker Desktop (Mac): Default 2GB (configurable)
   - ARM64 builds use more memory than x86_64

---

## Solutions (Recommended Order)

### ✅ Solution 1: Pre-built Binary (RECOMMENDED)

**Best for**: CI/CD, production builds

Build the binary locally/in CI, then copy to Docker image:

```bash
# Build binary first
cargo build --release --bin proximadb-server

# Build Docker image with pre-built binary
docker build -f Dockerfile.prebuilt -t proximadb:prebuilt .

# Or use the convenience script
./build-docker-prebuilt.sh
```

**Pros**:
- ✅ Zero OOM issues in Docker
- ✅ Fast Docker builds (just copying files)
- ✅ Binary can be built on any machine
- ✅ Reproducible builds

**Cons**:
- ❌ Requires separate build step
- ❌ Dockerfile doesn't show full build process

---

### 🖥️ Solution 2: Increase Docker Memory (Local Builds Only)

**Best for**: Local development on Docker Desktop

1. Open Docker Desktop
2. Go to Settings → Resources → Advanced
3. Increase memory: 2GB → **8GB+**
4. Click "Apply & Restart"

Then use the regular Dockerfile:
```bash
docker build -t proximadb:local .
```

**Pros**:
- ✅ Simple fix
- ✅ No code changes
- ✅ Full build in Docker

**Cons**:
- ❌ Doesn't help GitHub Actions (7GB limit)
- ❌ Requires manual configuration
- ❌ Not portable

---

### 🚀 Solution 3: Optimize Build Settings (Current Approach)

**Best for**: When you must build in Docker

Current Dockerfile uses:
```dockerfile
ENV CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=1 \
    CARGO_PROFILE_RELEASE_LTO=false \
    CARGO_PROFILE_RELEASE_DEBUG=false
RUN cargo build --release --bin proximadb-server --jobs=1
```

**Trade-offs**:
- ✅ Reduces memory by ~60%
- ❌ Binary is ~40% slower
- ❌ Still may OOM on limited systems

---

### 🔧 Solution 4: Use Memory-Efficient Linker

**Best for**: Production builds needing optimization

Install `mold` linker (2-3x faster, 30% less memory):

```dockerfile
RUN apt-get update && apt-get install -y mold
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"
RUN cargo build --release --bin proximadb-server
```

**Pros**:
- ✅ Faster builds
- ✅ Less memory usage
- ✅ Still optimized binary

**Cons**:
- ❌ mold not available everywhere
- ❌ Adds complexity

---

### 🎯 Solution 5: Custom Cargo Profile

**Best for**: Fine-tuned optimization

Create `.cargo/config.toml`:
```toml
[profile.docker]
inherits = "release"
opt-level = "z"     # Optimize for size
lto = false
codegen-units = 1
debug = false
strip = true        # Remove debug symbols
```

Use in Dockerfile:
```dockerfile
RUN cargo build --profile docker --bin proximadb-server
```

---

### ⚙️ Solution 6: Build with Fewer Features

**Best for**: Testing specific features

```dockerfile
ENV RUSTFLAGS="--no-default-features --features graph-first-sks,sql_frontend"
RUN cargo build --release --bin proximadb-server
```

---

## Comparison

| Solution | Memory | Speed | Complexity | Portable | CI-Friendly |
|----------|--------|-------|-----------|----------|-------------|
| Pre-built Binary | ✅ Lowest | ⚡ Fast | ⭐ Simple | ✅ Yes | ✅ Yes |
| Increase Memory | ✅ Depends | ⚡ Fast | ⭐ Simple | ❌ No | ❌ No |
| Optimize Settings | 🟡 Medium | 🟡 Medium | ⭐ Simple | ✅ Yes | ✅ Yes |
| Mold Linker | 🟡 Less | ⚡ Fast | ⭐⭐ Medium | 🟡 Mostly | 🟡 Yes |
| Custom Profile | 🟡 Less | 🟡 Medium | ⭐⭐ Medium | ✅ Yes | ✅ Yes |
| Fewer Features | 🟡 Less | ⚡ Fast | ⭐⭐ Medium | ✅ Yes | ✅ Yes |

---

## Recommended Setup

### For Development (Local):
```bash
# 1. Increase Docker memory to 8GB+
# 2. Build normally
docker build -t proximadb:dev .
```

### For CI/CD (GitHub Actions):
```yaml
# .github/workflows/build.yml
- name: Build Binary
  run: cargo build --release --bin proximadb-server

- name: Build Docker Image
  run: docker build -f Dockerfile.prebuilt -t proximadb:${{ github.sha }} .
```

### For Production:
```bash
# Use pre-built binary with optimizations
cargo build --release --bin proximadb-server
docker build -f Dockerfile.prebuilt -t proximadb:prod .
```

---

## Testing

Test which solution works for you:

```bash
# Test pre-built approach (fastest)
./build-docker-prebuilt.sh

# Test optimized build
docker build -t proximadb:test .

# Check binary size
ls -lh target/release/proximadb-server

# Test image works
docker run -p 5678:5678 proximadb:prebuilt
```

---

## Current Status

✅ **Recommended**: Use `Dockerfile.prebuilt` with pre-built binary
⚠️  **Current**: `Dockerfile` with opt-level=1 (may still OOM in CI)

To switch to pre-built approach:
```bash
# Use the prebuilt Dockerfile
docker build -f Dockerfile.prebuilt -t proximadb:latest .
```
