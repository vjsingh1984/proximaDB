#!/bin/bash
# Release Mode Performance Benchmark Runner
#
# Runs graph performance benchmarks in release mode for production-grade metrics
# Expected improvements over debug mode: 2-5x

set -e

echo "========================================"
echo "ProximaDB Graph Performance Benchmarks"
echo "Mode: RELEASE (optimized)"
echo "Date: $(date)"
echo "========================================"
echo ""

# Build in release mode
echo "Building in release mode..."
cargo build --release --test graph_performance_benchmark

echo ""
echo "Running benchmarks..."
echo "========================================"

# Run benchmarks with release profile
RUST_LOG=error cargo test --release --test graph_performance_benchmark -- --nocapture --test-threads=1

echo ""
echo "========================================"
echo "Benchmark Complete!"
echo "========================================"
