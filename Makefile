# ProximaDB Build and Test Makefile

.PHONY: all clean build test test-python test-rust benchmark release install help capability-matrix-check workspace-boundaries-check workspace-rebuild-baseline panic-policy-report panic-policy-no-regression panic-policy-module-guard panic-policy-baseline hygiene-check proto-check

# Default target
all: build test

PANIC_POLICY_BASELINE ?= docs/_internal/roadmap/PANIC_POLICY_BASELINE.json
PANIC_POLICY_ARTIFACT ?= artifacts/panic_policy/latest_metrics.json
PANIC_POLICY_CRITICAL_MODULES ?= network_rest,api_handlers,graph,query
PYTHON ?= python3

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
	cd clients/python && PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=$(PWD)/clients/python/src $(PYTHON) -m pytest -v

test-python-install:
	@echo "📦 Installing Python test dependencies..."
	cd clients/python && pip install -r tests/requirements.txt

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

hygiene-check:
	@echo "🧹 Running tracked artifact hygiene check..."
	@bad_files=$$(git ls-files | rg '(^|/)\\.victor($|/)|\\.bak[0-9]*$|\\.disabled$'); \
	if [ -n "$$bad_files" ]; then \
		echo "❌ Forbidden tracked artifacts detected:"; \
		echo "$$bad_files"; \
		exit 1; \
	fi; \
	echo "✅ No tracked artifact files detected."

check: fmt clippy test hygiene-check
	@echo "✅ Code quality checks passed"

capability-matrix-check:
	@echo "🧭 Validating capability matrix..."
	python3 scripts/validate_capability_matrix.py

proto-check:
	@echo "🧬 Validating protobuf/OpenAPI contract drift..."
	cargo check -p proximadb-proto
	cd clients/python && $(PYTHON) -m pytest --confcutdir=tests/unit -q tests/unit/test_grpc_proto_drift.py tests/unit/test_openapi_contract.py

workspace-boundaries-check:
	@echo "🧱 Validating workspace dependency boundaries..."
	python3 scripts/check_workspace_boundaries.py

workspace-rebuild-baseline:
	@echo "⏱️ Measuring targeted workspace rebuild baseline..."
	python3 scripts/measure_workspace_rebuild.py --keep-going

panic-policy-report:
	@echo "🧯 WS-2 panic policy report (non-blocking)..."
	@mkdir -p artifacts/panic_policy
	bash scripts/count_panic_patterns.sh --mode report --baseline $(PANIC_POLICY_BASELINE) --format text --write $(PANIC_POLICY_ARTIFACT)

panic-policy-no-regression:
	@echo "🧯 WS-2 panic policy no-regression check..."
	@mkdir -p artifacts/panic_policy
	bash scripts/count_panic_patterns.sh --mode no-regression --baseline $(PANIC_POLICY_BASELINE) --format text --write $(PANIC_POLICY_ARTIFACT)

panic-policy-module-guard:
	@echo "🧯 WS-2 panic policy critical-module guard..."
	@mkdir -p artifacts/panic_policy
	bash scripts/count_panic_patterns.sh --mode module-guard --baseline $(PANIC_POLICY_BASELINE) --critical-modules $(PANIC_POLICY_CRITICAL_MODULES) --format text --write $(PANIC_POLICY_ARTIFACT)

panic-policy-baseline:
	@echo "🧯 Refreshing WS-2 panic policy baseline..."
	bash scripts/count_panic_patterns.sh --mode report --format json --write $(PANIC_POLICY_BASELINE)
	@echo "Updated baseline: $(PANIC_POLICY_BASELINE)"

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
	@echo "  hygiene-check      - Detect tracked backup/disabled/.victor artifacts"
	@echo "  capability-matrix-check - Validate docs/_internal/roadmap/CAPABILITY_MATRIX.toml"
	@echo "  proto-check        - Validate generated proto crate and Python/OpenAPI contract drift"
	@echo "  panic-policy-report - WS-2 panic metrics report (non-blocking)"
	@echo "  panic-policy-no-regression - Fail on total panic-pattern regression"
	@echo "  panic-policy-module-guard - Fail on critical module panic regression"
	@echo "  panic-policy-baseline - Refresh panic-policy baseline artifact"
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
docs-update-gaps:
	python3 tools/update_critical_gaps.py docs/09-roadmap/planned/graph_database_requirements_spec.adoc

# ========================================
# TDD (Test-Driven Development) Commands
# ========================================

# Run TDD-specific tests
test-tdd:
	@echo "🧪 Running TDD tests..."
	cargo test --test tdd --verbose -- --test-threads=1 --nocapture

test-tdd-unit:
	@echo "🧪 Running TDD unit tests..."
	RUST_LOG=info cargo test --lib --verbose -- --test-threads=1 --nocapture

# Generate coverage report
test-coverage:
	@echo "📊 Generating coverage report..."
	cargo llvm-cov --lib --html --output-dir coverage
	@echo "📊 Coverage report: coverage/index.html"

# Check TDD methodology compliance
tdd-check:
	@echo "🔍 Checking TDD methodology compliance..."
	@echo "Test Count: $$(cargo test --lib --no-run --quiet 2>&1 | grep -o '[0-9]* tests' | grep -o '[0-9]*' || echo 0)"
	@echo "unwrap() calls (production): $$(grep -r '\.unwrap()' src/ --include='*.rs' | grep -v 'test' | grep -v '// OK:' | grep -v '// SAFETY:' | wc -l | xargs)"
	@echo "Target: <100"

# Install TDD pre-commit hook
install-tdd-hooks:
	@echo "📦 Installing TDD pre-commit hook..."
	@chmod +x .git/hooks/pre-commit.tdd
	@cp .git/hooks/pre-commit.tdd .git/hooks/pre-commit
	@echo "✓ TDD pre-commit hook installed"

# Install layering validation pre-commit hook
install-layering-hooks:
	@echo "Installing workspace layering validation pre-commit hook..."
	@chmod +x scripts/pre-commit-layering-hook.sh
	@ln -sf ../../scripts/pre-commit-layering-hook.sh .git/hooks/pre-commit
	@echo "✓ Layering validation pre-commit hook installed"
	@echo ""
	@echo "This hook will validate workspace layering before each commit."
	@echo "Run './scripts/check-layering.sh' manually to check for violations."

# Start TDD cycle for a new feature
tdd-start:
	@echo "Starting TDD cycle..."
	@echo ""
	@echo "1️⃣  Write failing test in tests/tdd/ or src/<module>/tests/"
	@echo "2️⃣  Run: make test-tdd-unit (should fail)"
	@echo "3️⃣  Implement feature to make test pass"
	@echo "4️⃣  Run: make test-tdd-unit (should pass)"
	@echo "5️⃣  Refactor while tests stay green"
	@echo ""
	@echo "Example workflow:"
	@echo "  1. Write test in src/core/search/hybrid/tests/fusion_test.rs"
	@echo "  2. Run: make test-tdd-unit core::search::hybrid"
	@echo "  3. Implement in src/core/search/hybrid/fusion.rs"
	@echo "  4. Run: make test-tdd-unit core::search::hybrid"
	@echo "  5. Refactor while tests stay green"

# Run TDD tests for specific module
test-tdd-module:
	@if [ -z "$(MODULE)" ]; then \
		echo "Usage: make test-tdd-module MODULE=<module_name>"; \
		echo "Example: make test-tdd-module MODULE=core::search::hybrid"; \
		exit 1; \
	fi
	@echo "🧪 Running TDD tests for $(MODULE)..."
	cargo test --lib $(MODULE) --verbose -- --test-threads=1 --nocapture

# Watch mode for TDD (requires cargo-watch)
test-watch:
	@echo "🔍 Running tests in watch mode..."
	@if command -v cargo-watch >/dev/null 2>&1; then \
		cargo watch -x 'test --lib --verbose'; \
	else \
		echo "❌ cargo-watch not installed. Install with: cargo install cargo-watch"; \
		exit 1; \
	fi

# TDD quality check (run before committing)
tdd-precommit:
	@echo "🔍 Running TDD pre-commit checks..."
	@$(MAKE) fmt-check
	@$(MAKE) clippy
	@$(MAKE) test-tdd-unit
	@echo "✅ All TDD pre-commit checks passed!"
