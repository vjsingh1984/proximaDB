# ProximaDB MVP Demo Container - Optimized for Production
# Build: docker build -t proximadb:mvp-demo .
# Run: docker run -p 5678:5678 -p 5679:5679 -v proximadb-data:/data proximadb:mvp-demo

# Stage 1: Build environment
FROM rust:1.75-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    cmake \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /build

# Copy manifests first for better layer caching
COPY Cargo.toml Cargo.lock build.rs ./

# Create dummy source structure for dependency building
RUN mkdir -p src/bin && \
    echo 'fn main() {}' > src/bin/proximadb-server.rs && \
    echo 'fn main() {}' > src/lib.rs

# Build dependencies (cached layer)
RUN cargo build --release --bin proximadb-server && \
    rm -rf src target/release/deps/proximadb*

# Copy actual source code
COPY src/ ./src/
COPY proto/ ./proto/
COPY schemas/ ./schemas/ 

# Build optimized binary with MVPs optimizations
ENV RUSTFLAGS="-C target-cpu=native -C opt-level=3"
RUN cargo build --release --bin proximadb-server

# Stage 2: Runtime environment (Alpine for smaller size)
FROM alpine:3.19

# Set metadata
LABEL maintainer="ProximaDB Development Team <singhvjd@gmail.com>"
LABEL description="ProximaDB MVP Demo - Cloud-native vector database"
LABEL version="0.1.0-mvp"
LABEL org.opencontainers.image.title="ProximaDB MVP Demo"
LABEL org.opencontainers.image.description="Optimized demo container for ProximaDB MVP"
LABEL org.opencontainers.image.vendor="ProximaDB"

# Install runtime dependencies (minimal)
RUN apk add --no-cache \
    ca-certificates \
    curl \
    bash \
    su-exec \
    dumb-init

# Create application user
RUN addgroup -g 1001 proximadb && \
    adduser -D -s /bin/sh -u 1001 -G proximadb proximadb

# Create optimized directory structure
RUN mkdir -p /opt/proximadb/{bin,config} && \
    mkdir -p /data/{wal,metadata,collections,logs} && \
    chown -R proximadb:proximadb /opt/proximadb /data

# Copy binary from builder stage
COPY --from=builder /build/target/release/proximadb-server /opt/proximadb/bin/
RUN chmod +x /opt/proximadb/bin/proximadb-server

# Create optimized demo configuration
COPY <<EOF /opt/proximadb/config/mvp-demo.toml
# ProximaDB MVP Demo Configuration - Optimized for Demo Use
[server]
rest_port = 5678
grpc_port = 5679
host = "0.0.0.0"
log_level = "info"

[storage]
# Persistent data paths (volume mounted)
wal_url = "file:///data/wal"
collections_url = "file:///data/collections"
metadata_url = "file:///data/metadata"

[storage.performance]
# MVP-optimized settings for demo
enable_compression = true
compression_level = 3
cache_size_mb = 512
enable_mmap = true
buffer_pool_size = 256
enable_background_compaction = true

[storage.viper]
# VIPER engine optimizations
enable_clustering = true
target_compression_ratio = 0.7
enable_dictionary_encoding = true
row_group_size = 10000

[indexing]
# Default HNSW parameters for demo
default_algorithm = "hnsw"
hnsw_m = 16
hnsw_ef_construction = 200
enable_quantization = false

[logging]
level = "info"
format = "structured"
output = "/data/logs/proximadb.log"
enable_request_logging = true

[monitoring]
enable_metrics = true
enable_health_checks = true
metrics_port = 9090
health_check_interval_secs = 30

[demo]
# Demo-specific settings
auto_create_collections = true
sample_data_enabled = true
api_documentation_enabled = true
EOF

# Create health check script
COPY <<EOF /opt/proximadb/bin/health-check.sh
#!/bin/bash
set -e

# Check if process is running
if ! pgrep -f "proximadb-server" > /dev/null; then
    echo "❌ ProximaDB server process not found"
    exit 1
fi

# Check REST API health
if ! curl -f -s --max-time 3 http://localhost:5678/health > /dev/null; then
    echo "❌ REST API health check failed"
    exit 1
fi

# Check gRPC health (if grpcurl is available)
# This is optional for basic health check
echo "✅ ProximaDB MVP demo healthy"
exit 0
EOF

# Create comprehensive demo setup script
COPY <<EOF /opt/proximadb/bin/setup-mvp-demo.sh
#!/bin/bash
set -e

echo "🚀 Setting up ProximaDB MVP Demo Environment..."
echo "   Version: 0.1.0-mvp"
echo "   Waiting for server to be ready..."

# Wait for server with timeout
timeout=60
count=0
while ! curl -f -s http://localhost:5678/health > /dev/null; do
    if [ \$count -ge \$timeout ]; then
        echo "❌ Server failed to start within \${timeout}s"
        exit 1
    fi
    sleep 1
    count=\$((count + 1))
done

echo "✅ Server ready after \${count}s"

# Create MVP demo collections
echo "📚 Creating MVP demo collections..."

# 1. BERT-compatible collection for document search
echo "  → Creating 'documents' collection (384d, BERT-compatible)..."
curl -X POST http://localhost:5678/v1/collections \
  -H "Content-Type: application/json" \
  -s -w "%{http_code}" \
  -d '{
    "name": "documents",
    "dimension": 384,
    "distance_metric": "cosine",
    "storage_engine": "viper",
    "indexing_algorithm": "hnsw",
    "filterable_metadata_fields": ["category", "author", "language", "source"],
    "config": {
      "hnsw_m": 16,
      "hnsw_ef_construction": 200,
      "enable_compression": true
    }
  }' | grep -q "200" && echo "    ✅ Documents collection created"

# 2. Product catalog collection
echo "  → Creating 'products' collection (768d, for product embeddings)..."
curl -X POST http://localhost:5678/v1/collections \
  -H "Content-Type: application/json" \
  -s -w "%{http_code}" \
  -d '{
    "name": "products",
    "dimension": 768,
    "distance_metric": "cosine",
    "storage_engine": "viper", 
    "indexing_algorithm": "hnsw",
    "filterable_metadata_fields": ["brand", "category", "price_range", "rating"],
    "config": {
      "hnsw_m": 32,
      "hnsw_ef_construction": 400,
      "enable_compression": true
    }
  }' | grep -q "200" && echo "    ✅ Products collection created"

# 3. Images collection for visual similarity
echo "  → Creating 'images' collection (512d, for visual embeddings)..."
curl -X POST http://localhost:5678/v1/collections \
  -H "Content-Type: application/json" \
  -s -w "%{http_code}" \
  -d '{
    "name": "images",
    "dimension": 512,
    "distance_metric": "cosine",
    "storage_engine": "viper",
    "indexing_algorithm": "hnsw",
    "filterable_metadata_fields": ["type", "size", "format", "tags"],
    "config": {
      "hnsw_m": 24,
      "hnsw_ef_construction": 300
    }
  }' | grep -q "200" && echo "    ✅ Images collection created"

echo ""
echo "🎉 ProximaDB MVP Demo Ready!"
echo ""
echo "📡 API Endpoints:"
echo "   REST API:  http://localhost:5678"
echo "   gRPC API:  localhost:5679"
echo "   Health:    http://localhost:5678/health"
echo "   Metrics:   http://localhost:9090/metrics"
echo ""
echo "📚 Demo Collections:"
echo "   • documents  (384d) - BERT-compatible document search"
echo "   • products   (768d) - Product catalog with filtering"
echo "   • images     (512d) - Visual similarity search"
echo ""
echo "🔗 Quick Start Examples:"
echo "   # List collections"
echo "   curl http://localhost:5678/v1/collections"
echo ""
echo "   # Insert vector (documents collection)"
echo "   curl -X POST http://localhost:5678/v1/collections/documents/vectors \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{\"id\":\"doc1\", \"vector\":[0.1,0.2,...], \"metadata\":{\"title\":\"Sample\"}}}'"
echo ""
echo "   # Search vectors"
echo "   curl -X POST http://localhost:5678/v1/collections/documents/search \\"
echo "     -H 'Content-Type: application/json' \\"
echo "     -d '{\"vector\":[0.1,0.2,...], \"k\":5}'"
echo ""
echo "📖 Full API documentation available at: http://localhost:5678/docs"
EOF

# Create optimized startup script
COPY <<EOF /opt/proximadb/bin/start-mvp-demo.sh
#!/bin/bash
set -e

echo "🎯 ProximaDB MVP Demo Container"
echo "   Version: 0.1.0-mvp"
echo "   Platform: \$(uname -m)"
echo "   Ports: REST:5678, gRPC:5679, Metrics:9090"
echo ""

# Ensure data directory permissions
chown -R proximadb:proximadb /data

# Start ProximaDB server as non-root user
echo "🚀 Starting ProximaDB MVP server..."
exec su-exec proximadb /opt/proximadb/bin/proximadb-server \
  --config /opt/proximadb/config/mvp-demo.toml \
  --data-dir /data &

SERVER_PID=\$!
echo "   Server PID: \$SERVER_PID"

# Setup demo environment in background
su-exec proximadb /opt/proximadb/bin/setup-mvp-demo.sh &

# Handle signals gracefully
trap 'echo "🛑 Stopping ProximaDB MVP demo..."; kill \$SERVER_PID; wait \$SERVER_PID; echo "✅ Stopped"' SIGTERM SIGINT

echo "✅ ProximaDB MVP demo container started"
echo "📊 Monitor logs: docker logs <container-id> -f"
echo "📊 Container logs: tail -f /data/logs/proximadb.log"

# Wait for server process
wait \$SERVER_PID
EOF

# Make scripts executable
RUN chmod +x /opt/proximadb/bin/*.sh

# Create volume for persistent data
VOLUME ["/data"]

# Expose ports
EXPOSE 5678 5679 9090

# Optimized health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD /opt/proximadb/bin/health-check.sh

# Use dumb-init for proper signal handling
ENTRYPOINT ["/usr/bin/dumb-init", "--"]

# Default command
CMD ["/opt/proximadb/bin/start-mvp-demo.sh"]