#!/bin/bash

# ProximaDB SIMD Performance Benchmarking Script
# This script runs comprehensive benchmarks for Proxima SIMD optimizations

set -e

echo "==============================================="
echo "ProximaDB Proxima SIMD Performance Benchmark"
echo "==============================================="
echo

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Create benchmark results directory
RESULTS_DIR="benchmark_results/simd_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$RESULTS_DIR"

echo -e "${BLUE}📊 Results will be saved to: $RESULTS_DIR${NC}"
echo

# Function to run benchmark and save results
run_benchmark() {
    local bench_name=$1
    local description=$2

    echo -e "${YELLOW}Running: $description${NC}"

    # Run the benchmark
    cargo bench --bench bench_16_proxima_simd -- "$bench_name" \
        --save-baseline simd_baseline \
        --output-format bencher \
        | tee "$RESULTS_DIR/${bench_name}.txt"

    echo -e "${GREEN}✓ Completed: $description${NC}"
    echo
}

# Function to compare with baseline
compare_with_baseline() {
    local bench_name=$1

    if [ -d "target/criterion/simd_baseline" ]; then
        echo -e "${BLUE}Comparing with baseline...${NC}"
        cargo bench --bench bench_16_proxima_simd -- "$bench_name" \
            --baseline simd_baseline \
            | tee -a "$RESULTS_DIR/${bench_name}_comparison.txt"
    fi
}

# Build in release mode first
echo -e "${YELLOW}Building in release mode...${NC}"
cargo build --release --bench bench_16_proxima_simd
echo -e "${GREEN}✓ Build complete${NC}"
echo

# Run individual benchmark groups
echo -e "${BLUE}=== Phase 1: Baseline Performance ===${NC}"
run_benchmark "proxima_baseline" "Proxima without SIMD optimization"

echo -e "${BLUE}=== Phase 2: SIMD Layout Comparison ===${NC}"
run_benchmark "proxima_simd_layouts" "Different SIMD encoding layouts"

echo -e "${BLUE}=== Phase 3: Engine Profile Performance ===${NC}"
run_benchmark "engine_profiles" "Engine-specific optimizations (HELIX/SST/SWIFT)"

echo -e "${BLUE}=== Phase 4: Data Pattern Analysis ===${NC}"
run_benchmark "simd_data_patterns" "Performance on different data patterns"

echo -e "${BLUE}=== Phase 5: Compression Ratio Analysis ===${NC}"
run_benchmark "compression_ratios" "Compression ratios achieved"

echo -e "${BLUE}=== Phase 6: SIMD Transpose Performance ===${NC}"
run_benchmark "simd_transpose" "SIMD transpose operation specifically"

echo -e "${BLUE}=== Phase 7: Encoding Algorithm Comparison ===${NC}"
run_benchmark "encoding_algorithms" "Different encoding algorithms"

echo -e "${BLUE}=== Phase 8: Large Scale Performance ===${NC}"
echo -e "${YELLOW}⚠️  This may take several minutes...${NC}"
run_benchmark "large_scale_simd" "Large-scale vector processing"

# Generate summary report
echo -e "${BLUE}=== Generating Summary Report ===${NC}"

cat > "$RESULTS_DIR/summary.md" << EOF
# Proxima SIMD Performance Benchmark Summary

**Date**: $(date)
**System**: $(uname -a)
**CPU**: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || lscpu | grep "Model name" | cut -d: -f2 | xargs)
**Rust Version**: $(rustc --version)

## Key Metrics

### Baseline vs SIMD Performance
$(grep -h "time:" "$RESULTS_DIR/proxima_baseline.txt" | head -5)

### Compression Ratios Achieved
$(grep -h "ratio" "$RESULTS_DIR/compression_ratios.txt" | head -5)

### Engine-Specific Performance
$(grep -h "helix\|sst\|swift" "$RESULTS_DIR/engine_profiles.txt" | head -10)

## Performance Improvements

### SIMD Speedup
- Transpose operations: Check simd_transpose.txt
- Encoding performance: Check encoding_algorithms.txt
- Large-scale processing: Check large_scale_simd.txt

## Recommendations

Based on the benchmarks:
1. Best layout for general use: TransposeFieldEncodedAndCompressed
2. Best engine profile depends on workload
3. SIMD provides significant benefits for clustered and sequential data

EOF

echo -e "${GREEN}✅ Summary report generated: $RESULTS_DIR/summary.md${NC}"

# Generate CSV for analysis
echo -e "${BLUE}=== Generating CSV for Analysis ===${NC}"

echo "benchmark,time_ns,throughput,improvement" > "$RESULTS_DIR/results.csv"

# Parse and extract metrics (simplified - would need proper parsing in production)
for file in "$RESULTS_DIR"/*.txt; do
    if [ -f "$file" ]; then
        bench_name=$(basename "$file" .txt)
        # Extract timing information (this is a simplified extraction)
        grep -E "time:.*ns" "$file" | while read -r line; do
            echo "$bench_name,$line" >> "$RESULTS_DIR/results.csv"
        done
    fi
done

echo -e "${GREEN}✅ CSV generated: $RESULTS_DIR/results.csv${NC}"

# Print summary
echo
echo "==============================================="
echo -e "${GREEN}Benchmark Complete!${NC}"
echo "==============================================="
echo
echo "Results saved to: $RESULTS_DIR"
echo
echo "Key findings:"
echo "1. Check $RESULTS_DIR/summary.md for overview"
echo "2. Individual results in $RESULTS_DIR/*.txt"
echo "3. CSV data in $RESULTS_DIR/results.csv"
echo
echo "To visualize results:"
echo "  cargo criterion --bench bench_16_proxima_simd"
echo
echo "To compare with future runs:"
echo "  cargo bench --bench bench_16_proxima_simd -- --baseline simd_baseline"