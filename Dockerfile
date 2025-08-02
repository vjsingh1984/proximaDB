# ProximaDB Production Docker Image
# Optimized multi-stage build with all features

# Stage 1: Build environment
FROM rust:1.82-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    cmake \
    build-essential \
    git \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /build

# Copy manifests for dependency caching
COPY Cargo.toml Cargo.lock build.rs ./

# Copy proto files (needed for build.rs)
COPY proto/ ./proto/

# Create dummy source for dependency building
RUN mkdir -p src/bin src/proto && \
    echo 'fn main() {}' > src/bin/server.rs && \
    echo 'fn main() {}' > src/lib.rs

# Copy proto descriptor file (needed by build.rs)
COPY src/proto/proximadb_descriptor.bin ./src/proto/

# Build dependencies (cached layer)
RUN cargo build --release --bin proximadb-server && \
    rm -rf src target/release/deps/proximadb*

# Copy actual source code
COPY src/ ./src/

# Build optimized binary with all features
ENV RUSTFLAGS="-C opt-level=3"
RUN cargo build --release --bin proximadb-server

# Stage 2: Runtime environment
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create application user
RUN useradd -m -u 1001 -s /bin/bash proximadb

# Create directory structure
RUN mkdir -p /opt/proximadb/{bin,config,logs} && \
    mkdir -p /data/{wal,metadata,collections,logs} && \
    chown -R proximadb:proximadb /opt/proximadb /data

# Copy binary from builder
COPY --from=builder /build/target/release/proximadb-server /opt/proximadb/bin/
RUN chmod +x /opt/proximadb/bin/proximadb-server

# Copy configuration file from demo directory
COPY demo/docker-config.toml /opt/proximadb/config/docker-config.toml

# Create health check script
RUN cat <<'EOF' > /opt/proximadb/bin/health-check.sh
#!/bin/bash
curl -f -s --max-time 3 http://localhost:5678/health > /dev/null
EOF
RUN chmod +x /opt/proximadb/bin/health-check.sh

# Set working directory
WORKDIR /opt/proximadb

# Switch to non-root user
USER proximadb

# Set environment
ENV RUST_LOG=info
ENV PROXIMADB_CONFIG_PATH=/opt/proximadb/config/docker-config.toml

# Create volume for persistent data
VOLUME ["/data"]

# Expose ports
EXPOSE 5678 5679 9090

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD /opt/proximadb/bin/health-check.sh

# Start server
CMD ["/opt/proximadb/bin/proximadb-server", "--config", "/opt/proximadb/config/docker-config.toml"]