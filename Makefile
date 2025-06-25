# ProximaDB Build and Test Makefile

.PHONY: all clean build test test-python test-rust benchmark release install help

# Default target
all: build test

# Build targets
build:
	@echo "🔨 Building ProximaDB..."
	cargo build

build-release:
	@echo "🚀 Building ProximaDB (Release)..."
	cargo build --release

build-server:
	@echo "🚀 Building ProximaDB Server (Optimized)..."
	cargo build --profile release-server

# Test targets
test: test-rust test-python
	@echo "✅ All tests completed"

test-rust:
	@echo "🧪 Running Rust tests..."
	cargo test --verbose

test-integration:
	@echo "🔗 Running integration tests..."
	cargo test --test integration --verbose

test-python:
	@echo "🐍 Running Python tests..."
	cd tests/python && PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=/workspace/clients/python/src python3 -m pytest -v

test-python-install:
	@echo "📦 Installing Python test dependencies..."
	cd tests/python && pip install -r requirements.txt

# Benchmark targets
benchmark:
	@echo "📊 Running benchmarks..."
	cargo bench

benchmark-vector:
	@echo "📊 Running vector operation benchmarks..."
	cargo bench --bench vector_operations

benchmark-metadata:
	@echo "📊 Running metadata lifecycle benchmarks..."
	cargo bench --bench metadata_lifecycle

# Code quality targets
fmt:
	@echo "🎨 Formatting code..."
	cargo fmt

clippy:
	@echo "📎 Running clippy..."
	cargo clippy -- -D warnings

check: fmt clippy test
	@echo "✅ Code quality checks passed"

# Release targets
release: clean build-server test benchmark
	@echo "🎯 Release build completed successfully"
	@echo "📊 Release artifacts:"
	@ls -la target/release-server/proximadb-server 2>/dev/null || echo "Server binary not found"
	@ls -la target/release/proximadb-server 2>/dev/null || echo "Fallback to release binary"

install: build-release
	@echo "📦 Installing ProximaDB..."
	cargo install --path . --force

# Development targets
dev: build test-rust
	@echo "🔧 Development build completed"

server-start:
	@echo "🚀 Starting ProximaDB server..."
	cargo run --bin proximadb-server

server-start-release:
	@echo "🚀 Starting ProximaDB server (Release)..."
	cargo run --release --bin proximadb-server

# Clean targets
clean:
	@echo "🧹 Cleaning build artifacts..."
	cargo clean
	rm -rf tests/python/__pycache__/
	rm -rf tests/python/.pytest_cache/
	find . -name "*.pyc" -delete

# Documentation
docs:
	@echo "📚 Generating documentation..."
	cargo doc --open

# Docker targets (if needed)
docker-build:
	@echo "🐳 Building Docker image..."
	docker build -t proximadb:latest .

docker-run:
	@echo "🐳 Running ProximaDB in Docker..."
	docker run -p 5678:5678 proximadb:latest

# Performance testing
perf-test: build-release
	@echo "⚡ Running performance tests..."
	@echo "Starting server in background..."
	cargo run --release --bin proximadb-server &
	@echo "Waiting for server to start..."
	sleep 5
	@echo "Running performance test..."
	cd tests/python && python test_integration_comprehensive.py
	@echo "Stopping server..."
	pkill -f proximadb-server || true

# Full integration test with real server
integration-full: build-release
	@echo "🔗 Running full integration test..."
	@echo "Starting server..."
	cargo run --release --bin proximadb-server &
	@echo "Waiting for server to start..."
	sleep 5
	@echo "Running comprehensive tests..."
	cd tests/python && python -m pytest test_integration_comprehensive.py -v
	@echo "Stopping server..."
	pkill -f proximadb-server || true

# Help target
help:
	@echo "ProximaDB Build Commands:"
	@echo ""
	@echo "Building:"
	@echo "  build              - Debug build"
	@echo "  build-release      - Release build"
	@echo "  build-server       - Optimized server build"
	@echo ""
	@echo "Testing:"
	@echo "  test               - Run all tests"
	@echo "  test-rust          - Run Rust tests only"
	@echo "  test-python        - Run Python tests only"
	@echo "  test-integration   - Run integration tests"
	@echo "  perf-test          - Run performance tests with server"
	@echo "  integration-full   - Full integration test with real server"
	@echo ""
	@echo "Benchmarks:"
	@echo "  benchmark          - Run all benchmarks"
	@echo "  benchmark-vector   - Vector operation benchmarks"
	@echo "  benchmark-metadata - Metadata lifecycle benchmarks"
	@echo ""
	@echo "Code Quality:"
	@echo "  fmt                - Format code"
	@echo "  clippy             - Run linter"
	@echo "  check              - Format + lint + test"
	@echo ""
	@echo "Release:"
	@echo "  release            - Full release build with tests"
	@echo "  install            - Install ProximaDB system-wide"
	@echo ""
	@echo "Development:"
	@echo "  dev                - Quick development build"
	@echo "  server-start       - Start server (debug)"
	@echo "  server-start-release - Start server (release)"
	@echo ""
	@echo "Utilities:"
	@echo "  clean              - Clean all build artifacts"
	@echo "  docs               - Generate documentation"
	@echo "  help               - Show this help"