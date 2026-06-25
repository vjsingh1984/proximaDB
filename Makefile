# ProximaDB Build and Test Makefile

.PHONY: all clean build test test-python test-rust test-fast check-fast install-fast-tools benchmark release install help capability-matrix-check workspace-boundaries-check tenant-path-check deterministic-commit-contract-check work-commit-check validated-commit-check workspace-rebuild-baseline panic-policy-report panic-policy-no-regression panic-policy-module-guard panic-policy-baseline hygiene-check proto-check verify-openapi-spec gen-go-sdk gen-ts-sdk gen-rust-sdk gen-python-sdk release-check docs-claim-check release-smoke

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

# Fast iteration loop. Uses nextest (parallel, process-isolated) if installed;
# falls back to libtest with parallel --test-threads. Bypasses the
# RUST_TEST_THREADS=1 env in .cargo/config.toml (which is for integration tests).
test-fast:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		echo "Running unit tests via nextest (profile=unit)..."; \
		cargo nextest run --lib --profile unit; \
	else \
		echo "nextest not installed; falling back to: cargo test --lib --test-threads=6"; \
		echo "  install with: make install-fast-tools"; \
		cargo test --lib -- --test-threads=6; \
	fi

# Type-check only (no codegen, no link) for the tightest inner edit loop.
check-fast:
	@cargo check-fast

# Install optional build-speed tools. Idempotent.
install-fast-tools:
	@if ! command -v cargo-nextest >/dev/null 2>&1; then \
		echo "Installing cargo-nextest..."; \
		cargo install cargo-nextest --locked; \
	else echo "cargo-nextest: already installed"; fi
	@if ! command -v cargo-watch >/dev/null 2>&1; then \
		echo "Installing cargo-watch..."; \
		cargo install cargo-watch --locked; \
	else echo "cargo-watch: already installed"; fi

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

deterministic-commit-contract-check:
	@echo "🧷 Validating deterministic commit contract..."
	python3 scripts/check_deterministic_commit_contract.py

proto-check:
	@echo "🧬 Validating protobuf/OpenAPI contract drift..."
	cargo check -p proximadb-proto
	cd clients/python && $(PYTHON) -m pytest --confcutdir=tests/unit -q tests/unit/test_grpc_proto_drift.py tests/unit/test_openapi_contract.py

# TD-126 Phase 1: the OpenAPI spec is GENERATED from the annotated REST handlers
# (src/network/rest/openapi.rs), not hand-maintained. This regenerates it in
# memory and fails if it drifts from the committed docs/openapi/proximadb-openapi.yaml.
# Mirrors the proto-sync generated-artifact gate. Regenerate with:
#   UPDATE_OPENAPI_SPEC=1 cargo test -p proximadb --test openapi_spec_gen
verify-openapi-spec:
	@echo "🧬 Validating OpenAPI spec ↔ handler drift (spec-from-code)..."
	cargo test -p proximadb --test openapi_spec_gen

# TD-126 Phase 2 (spec-driven SDK pilot, Go): the Go REST transport's wire
# plumbing (clients/go/proximadb/internal/genrest) is GENERATED from the
# published OpenAPI spec via oapi-codegen (pinned in clients/go/codegen/go.mod),
# behind a thin hand-written ergonomic facade (internal/rest/adapter.go). This
# regenerates the client; commit the result. The CI gate `go-sdk-codegen-drift`
# runs this and `git diff --exit-code`s the generated client — same pattern as
# verify-openapi-spec / the proto-sync gate.
gen-go-sdk:
	@echo "🧬 Generating Go REST transport from OpenAPI spec (TD-126 Phase 2)..."
	PYTHON=$(PYTHON) bash clients/go/codegen/gen.sh

# TD-126 Phase 4 (spec-driven SDK pilot, TypeScript): the TS REST transport's
# wire types (clients/nodejs-embedded/src/generated) are GENERATED from the
# published OpenAPI spec via openapi-typescript (pinned in the SDK package.json),
# driven by openapi-fetch behind a thin hand-written ergonomic facade
# (src/client.ts + src/transport.ts). This regenerates the types; commit the
# result. The CI gate `ts-sdk-codegen-drift` runs this and
# `git diff --exit-code`s the generated dir — same pattern as gen-go-sdk /
# verify-openapi-spec. openapi-typescript is 3.1-native, so no down-conversion.
gen-ts-sdk:
	@echo "🧬 Generating TypeScript REST transport from OpenAPI spec (TD-126 Phase 4)..."
	cd clients/nodejs-embedded && npm run gen-sdk

# TD-126 Phase 4 (spec-driven SDK, Rust): the Rust REST transport's wire
# plumbing (clients/rust/src/genrest.rs) is GENERATED from the published OpenAPI
# spec via progenitor (pinned in clients/rust/codegen/Cargo.toml), behind the
# unchanged hand-written ergonomic facade (clients/rust/src/client.rs). This
# regenerates the client; commit the result. The CI gate `rust-sdk-codegen-drift`
# runs this and `git diff --exit-code`s the generated client — same pattern as
# gen-go-sdk / verify-openapi-spec / the proto-sync gate.
gen-rust-sdk:
	@echo "🧬 Generating Rust REST transport from OpenAPI spec (TD-126 Phase 4)..."
	PYTHON=$(PYTHON) bash clients/rust/codegen/gen.sh

# TD-126 Phase 4 (spec-driven SDK, Python): the Python REST transport's wire
# plumbing (clients/python/src/proximadb_sdk/_generated/rest) is GENERATED from
# the published OpenAPI spec via openapi-python-client (pinned in
# clients/python/codegen/requirements.txt), behind the unchanged hand-written
# ergonomic facade (protocols/rest_sync.py). openapi-python-client is 3.1-native,
# so the published spec is consumed directly (no down-conversion). This
# regenerates the client; commit the result. The CI gate `python-sdk-codegen-drift`
# runs this and `git diff --exit-code`s the generated dir — same pattern as
# gen-go-sdk / gen-rust-sdk / verify-openapi-spec / the proto-sync gate.
gen-python-sdk:
	@echo "🧬 Generating Python REST transport from OpenAPI spec (TD-126 Phase 4)..."
	PYTHON=$(PYTHON) bash clients/python/codegen/gen.sh

workspace-boundaries-check:
	@echo "🧱 Validating workspace dependency boundaries..."
	python3 scripts/check_workspace_boundaries.py

tenant-path-check:
	@echo "🏢 Validating tenant path isolation guard..."
	python3 scripts/check_tenant_path_guard.py

work-commit-check: fmt-check deterministic-commit-contract-check docs-claim-check capability-matrix-check workspace-boundaries-check tenant-path-check panic-policy-module-guard hygiene-check
	@echo "✅ work-commit-check: deterministic architecture and commit guardrails passed"

validated-commit-check: work-commit-check
	@echo "✅ validated-commit-check: alias complete"

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

# Release-cut gate: one command that must be green before the v0.2 release tag.
# Sequence is fail-fast — early steps (fmt, doc-claim, proto) are cheap.
release-check: work-commit-check proto-check release-smoke build-server
	@echo "✅ release-check: all gates passed"

fmt-check:
	@echo "🎨 Checking formatting (cargo fmt --check)..."
	cargo fmt --check

# Fails if release-cut docs contain MVP-completion / production-readiness claims that
# exceed docs/SUPPORTED_SURFACE.adoc. The matched files in _archive/ and enterprise/
# marketing copy are intentionally excluded; everything else must reconcile.
docs-claim-check:
	@echo "📝 Checking release-facing docs for stale MVP/production claims..."
	@hits=$$(grep -rnE "96% complete|full cross-model query support|production[- ]ready MVP" docs/ \
		--exclude-dir=_archive --exclude-dir=enterprise --exclude-dir=business 2>/dev/null | \
		grep -v "Historical\|superseded\|Superseded\|MVP_RELEASE_READINESS"); \
	if [ -n "$$hits" ]; then \
		echo "❌ Release-facing docs still contain stale MVP/production claims:"; \
		echo "$$hits"; \
		echo ""; \
		echo "Either remove the claim, or tag the file as historical/superseded."; \
		exit 1; \
	fi; \
	echo "✅ No stale MVP/production claims in release-facing docs."
	@echo "📝 Checking that internal marketing copy is not referenced from public docs (TD-085)..."
	@xrefs=$$(grep -rnE "_internal/enterprise/|_internal/business/" docs/ \
		--exclude-dir=_internal --exclude-dir=_archive \
		--exclude=TECHNICAL_DEBT.adoc 2>/dev/null); \
	if [ -n "$$xrefs" ]; then \
		echo "❌ Public docs reference internal marketing copy:"; \
		echo "$$xrefs"; \
		echo ""; \
		echo "Files under docs/_internal/enterprise/ and docs/_internal/business/"; \
		echo "are sealed internal-only (TD-085). Move the referenced material out of"; \
		echo "those directories, or remove the reference from the public doc."; \
		echo "The Technical Debt Register is exempt from this rule (it tracks the work)."; \
		exit 1; \
	fi; \
	echo "✅ No public-doc references to internal marketing copy."

# Minimum smoke battery for the release cut. Each entry must be a non-ignored test
# that exercises the canonical v2 record path or one of the diagnostic blocks
# whose contract v0.2 commits to.
release-smoke:
	@echo "🚦 Running release-smoke tests..."
	cargo test --lib route_health -- --test-threads=1
	cargo test --lib query_optimizer -- --test-threads=1
	cargo test --lib object_economy_directory -- --test-threads=1
	cargo test --lib vector_hints_from_search_plan_hints_preserves_ann_reason -- --test-threads=1
	cargo test --test graph_branch_merge_integration_test -- --test-threads=1
	cargo test --test grpc_hybrid_integration_test -- --test-threads=1
	cargo test --test release_smoke_v2 -- --test-threads=1 --nocapture
	@echo "✅ release-smoke green"

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
	@echo "  deterministic-commit-contract-check - Validate zero-retry/test/docs guard wiring"
	@echo "  tenant-path-check  - Enforce DrPathBuilder tenant path guard"
	@echo "  work-commit-check  - Fast deterministic architecture guard before commit/push"
	@echo "  proto-check        - Validate generated proto crate and Python/OpenAPI contract drift"
	@echo "  verify-openapi-spec - Regenerate OpenAPI spec from handlers; fail on drift (TD-126)"
	@echo "  gen-go-sdk         - Regenerate the Go REST transport from the OpenAPI spec (TD-126 Phase 2)"
	@echo "  gen-ts-sdk         - Regenerate the TypeScript REST transport types from the OpenAPI spec (TD-126 Phase 4)"
	@echo "  gen-rust-sdk       - Regenerate the Rust REST transport from the OpenAPI spec (TD-126 Phase 4)"
	@echo "  gen-python-sdk     - Regenerate the Python REST transport from the OpenAPI spec (TD-126 Phase 4)"
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
