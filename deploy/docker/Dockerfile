# ProximaDB Production Dockerfile
# Multi-stage build for minimal image size (~150MB target)
# Supports AWS EKS, Azure AKS, and Google GKE deployments

# ============================================================================
# Stage 1: Builder
# ============================================================================
FROM rust:1.88-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    cmake \
    protobuf-compiler \
    libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create dummy source for dependency caching
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/server.rs && \
    echo "pub fn lib() {}" > src/lib.rs

# Build dependencies only (cached layer)
RUN cargo build -p proximadb-server --release --bin proximadb-server 2>/dev/null || true

# Copy actual source code. Workspace members (apps/, clients/, crates/)
# are required for cargo to resolve the workspace — Cargo.toml at /build
# declares them and refuses to build any single member otherwise.
COPY src/ src/
COPY proto/ proto/
COPY config/ config/
COPY apps/ apps/
COPY clients/ clients/
COPY crates/ crates/

# Build the actual binary
RUN cargo build -p proximadb-server --release --bin proximadb-server

# Strip debug symbols for smaller binary
RUN strip /build/target/release/proximadb-server

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM debian:bookworm-slim AS runtime

# Install minimal runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Create non-root user for security
RUN groupadd -r proximadb && useradd -r -g proximadb proximadb

# Create data directories
RUN mkdir -p /data/proximadb /config && \
    chown -R proximadb:proximadb /data /config

# Copy binary from builder
COPY --from=builder /build/target/release/proximadb-server /usr/local/bin/proximadb-server

# Copy default configuration
COPY --from=builder /build/config/config.toml /config/config.toml

# Set ownership
RUN chown proximadb:proximadb /usr/local/bin/proximadb-server

# Switch to non-root user
USER proximadb

# Environment variables
ENV RUST_LOG=info \
    PROXIMADB_DATA_DIR=/data/proximadb \
    PROXIMADB_BIND_ADDRESS=0.0.0.0 \
    PROXIMADB_REST_PORT=5678 \
    PROXIMADB_GRPC_PORT=5679 \
    PROXIMADB_ARROW_IPC_PORT=5680 \
    PROXIMADB_METRICS_PORT=9090

# Expose ports
# 5678: REST API (Prometheus metrics also served here at /metrics)
# 5679: gRPC API
# 5680: Arrow Flight (bulk ingest, columnar zero-copy)
# 5433: PostgreSQL wire protocol (pgwire) — pgvector / psql / pgAdmin
#       compatibility; enabled by default per PostgresServerConfig
EXPOSE 5678 5679 5680 5433

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -f http://localhost:5678/health || exit 1

# Volume for persistent data
VOLUME ["/data/proximadb"]

# Working directory
WORKDIR /data/proximadb

# Entrypoint: optionally fetches a runtime tier-config overlay from
# PROXIMADB_TIER_CONFIG_URL at boot, then execs the server. Fallback:
# if the fetch fails (offline, network error, no URL configured), the
# baked-in /config/tier-config.json wins — pod never crashes on
# network blips. See deploy/docker/entrypoint.sh for the full fetch +
# fallback logic; engine reads /config/tier-config.json at startup
# (Phase B-5; src/catalog/tenant_tier.rs::resolve_tier_config_source).
COPY deploy/docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--config", "/config/config.toml"]
