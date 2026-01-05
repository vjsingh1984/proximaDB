# ProximaDB Container - Server
# Single container with ProximaDB server

# Stage 1: Build ProximaDB server
FROM rust:1.88-slim as builder

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

# Copy Rust SDK (workspace member required by Cargo.toml)
COPY clients/rust/ ./clients/rust/

# Create dummy source for dependency building
RUN mkdir -p src/bin src/proto && \
    echo 'fn main() {}' > src/bin/server.rs && \
    echo 'fn main() {}' > src/lib.rs

# Copy proto descriptor file (needed by build.rs)
COPY src/proto/proximadb_descriptor.bin ./src/proto/

# Build dependencies only (cached layer - reused when only source code changes)
RUN cargo build --release --bin proximadb-server && \
    rm -rf src target/release/deps/proximadb* target/release/proximadb-server

# Copy actual source code
COPY src/ ./src/

# Build final optimized binary with real source code
ENV RUSTFLAGS="-C opt-level=3"
RUN cargo build --release --bin proximadb-server

# Stage 2: Unified runtime with Python and system dependencies
FROM python:3.11-slim

# Install system dependencies for both ProximaDB and Python demos
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libssl3 \
    gcc \
    g++ \
    git \
    supervisor \
    && rm -rf /var/lib/apt/lists/*

# Create application user
RUN useradd -m -u 1001 -s /bin/bash proximadb

# Create directory structure for both ProximaDB and demo UI
RUN mkdir -p /opt/proximadb/{bin,config,logs} && \
    mkdir -p /data/{wal,metadata,collections,logs,viper_data} && \
    mkdir -p /app/{static,utils,results,embedding_cache} && \
    chown -R proximadb:proximadb /opt/proximadb /data /app

# Copy ProximaDB binary from builder
COPY --from=builder /build/target/release/proximadb-server /opt/proximadb/bin/
RUN chmod +x /opt/proximadb/bin/proximadb-server

# Copy ProximaDB configuration file
COPY demo/config/docker-config.toml /opt/proximadb/config/docker-config.toml

# Set working directory for Python components
WORKDIR /app

# Create a stable requirements file to improve Docker layer caching
RUN cat > requirements-stable.txt << 'EOF'
# Core dependencies
numpy>=1.24.0
requests>=2.28.0
asyncio-mqtt>=0.11.0

# Text processing for chunking
nltk>=3.8.0
spacy>=3.5.0
sentence-transformers>=2.2.0

# Data science and ML
pandas>=1.5.0
scikit-learn>=1.2.0

# Visualization
matplotlib>=3.6.0
seaborn>=0.12.0

# Progress bars and CLI
tqdm>=4.64.0
click>=8.1.0

# HTTP clients
aiohttp>=3.8.0
aiohttp-cors>=0.7.0
httpx[http2]>=0.24.0
h2>=4.0.0

# JSON handling
orjson>=3.8.0

# Logging
structlog>=22.3.0

# Testing
pytest>=7.2.0
pytest-asyncio>=0.21.0

# Development
black>=22.12.0
isort>=5.11.0
mypy>=1.0.0

# BERT embeddings and system (REQUIRED - no fallbacks)
sentence-transformers>=2.2.0
torch>=1.13.0
transformers>=4.21.0
tokenizers>=0.13.0
supervisor>=4.2.0

# LLM support for RAG demo (Flan-T5)
# Already included via transformers above
EOF

# Install Python dependencies (this layer will be cached unless requirements change)
RUN pip install --no-cache-dir --upgrade pip && \
    pip install --no-cache-dir -r requirements-stable.txt

# Download required NLTK data (cached layer)
RUN python -c "import nltk; nltk.download('punkt', quiet=True)" || true

# Set up proper cache directory for BERT models with correct permissions
RUN mkdir -p /app/embedding_cache /tmp/hf_cache && \
    chown -R proximadb:proximadb /app/embedding_cache /tmp/hf_cache && \
    chmod -R 777 /app/embedding_cache /tmp/hf_cache

# Set environment variables for model downloads
ENV TRANSFORMERS_CACHE=/app/embedding_cache
ENV HF_HOME=/app/embedding_cache
ENV TORCH_HOME=/app/embedding_cache
ENV TRANSFORMERS_OFFLINE=0
ENV HF_HUB_OFFLINE=0

# Pre-download BERT models with proper permissions and error handling
RUN python -c "\
import os; \
os.makedirs('/app/embedding_cache', exist_ok=True); \
os.chmod('/app/embedding_cache', 0o777); \
try: \
    from sentence_transformers import SentenceTransformer; \
    import torch; \
    print('🤖 Pre-downloading BERT models for real embeddings...'); \
    \
    # Download with explicit cache directory \
    model = SentenceTransformer('all-MiniLM-L6-v2', cache_folder='/app/embedding_cache'); \
    print('✅ all-MiniLM-L6-v2 (384-dim) downloaded'); \
    test_embedding = model.encode('test', show_progress_bar=False); \
    print(f'✅ Model test successful: {len(test_embedding)}D embedding'); \
    \
    model2 = SentenceTransformer('all-mpnet-base-v2', cache_folder='/app/embedding_cache'); \
    print('✅ all-mpnet-base-v2 (768-dim) downloaded'); \
    \
    model3 = SentenceTransformer('all-MiniLM-L12-v2', cache_folder='/app/embedding_cache'); \
    print('✅ all-MiniLM-L12-v2 (384-dim) downloaded'); \
    \
    print('🎉 All BERT models ready for production use!'); \
except Exception as e: \
    print(f'⚠️ Model download failed: {e}'); \
    print('Models will be downloaded at runtime'); \
" || echo "Model pre-download failed, will download at runtime"

# Pre-download Flan-T5 model for RAG demo (adds ~250MB to image)
# This enables offline AI Knowledge Base functionality
RUN python -c "\
import os; \
os.makedirs('/app/embedding_cache/llm', exist_ok=True); \
os.chmod('/app/embedding_cache/llm', 0o777); \
try: \
    from transformers import T5ForConditionalGeneration, T5Tokenizer; \
    print('🤖 Pre-downloading Flan-T5-small for RAG demo...'); \
    tokenizer = T5Tokenizer.from_pretrained('google/flan-t5-small', cache_dir='/app/embedding_cache/llm'); \
    model = T5ForConditionalGeneration.from_pretrained('google/flan-t5-small', cache_dir='/app/embedding_cache/llm'); \
    print('✅ Flan-T5-small downloaded for offline RAG'); \
except Exception as e: \
    print(f'⚠️ Flan-T5 download failed: {e}'); \
    print('Will use Hugging Face API or download at runtime'); \
" || echo "Flan-T5 pre-download skipped"

# Ensure cache directory has correct permissions after download
RUN chown -R proximadb:proximadb /app/embedding_cache && \
    chmod -R 755 /app/embedding_cache

# Copy and install ProximaDB Python SDK (cached unless SDK changes)
COPY clients/python /app/proximadb-sdk
RUN cd /app/proximadb-sdk && pip install -e .

# Copy demo scripts and UI in order of change frequency
# Copy configuration and requirements first (changes less frequently)
COPY demo/requirements.txt ./demo-requirements.txt

# Copy Python modules (changes moderately)
COPY demo/utils ./utils/
COPY demo/benchmarks ./benchmarks/

# Copy main scripts (changes more frequently)
COPY demo/*.py ./

# Make scripts executable
RUN chmod +x *.py || true

# Create supervisor configuration for running ProximaDB server
RUN cat <<'EOF' > /etc/supervisor/conf.d/proximadb.conf
[supervisord]
nodaemon=true
user=root
logfile=/var/log/supervisor/supervisord.log
pidfile=/var/run/supervisord.pid

[program:proximadb-server]
command=/opt/proximadb/bin/proximadb-server --config /opt/proximadb/config/docker-config.toml
user=proximadb
directory=/opt/proximadb
stdout_logfile=/dev/stdout
stdout_logfile_maxbytes=0
stderr_logfile=/dev/stderr
stderr_logfile_maxbytes=0
autorestart=true
startretries=3
priority=100
EOF

# Create health check script for ProximaDB server
RUN cat <<'EOF' > /opt/proximadb/bin/health-check.sh
#!/bin/bash
# Check ProximaDB server
if ! curl -f -s --max-time 3 http://localhost:5678/health > /dev/null; then
    echo "ProximaDB server health check failed"
    exit 1
fi

echo "ProximaDB server healthy"
exit 0
EOF
RUN chmod +x /opt/proximadb/bin/health-check.sh

# Set environment variables
ENV RUST_LOG=info
ENV PROXIMADB_CONFIG_PATH=/opt/proximadb/config/docker-config.toml
ENV PROXIMADB_SERVER_URL=http://localhost:5678
ENV PROXIMADB_GRPC_URL=localhost:5679
ENV PYTHONUNBUFFERED=1
ENV EMBEDDING_CACHE_DIR=/app/embedding_cache
ENV TRANSFORMERS_CACHE=/app/embedding_cache
ENV HF_HOME=/app/embedding_cache

# Create volume for persistent data
VOLUME ["/data"]

# Expose ports for ProximaDB server
EXPOSE 5678 5679

# Health check for ProximaDB server
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD /opt/proximadb/bin/health-check.sh

# Start ProximaDB server with supervisor
CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/conf.d/proximadb.conf"]