#!/bin/bash

# ProximaDB Encoding Performance Benchmark Runner
# Implements the benchmark commands referenced in ENCODING_PERFORMANCE.adoc

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}ProximaDB Encoding Performance Benchmarks${NC}"
echo -e "${BLUE}========================================${NC}"

# Function to run benchmark with error handling
run_benchmark() {
    local benchmark_name="$1"
    local description="$2"

    echo -e "\n${YELLOW}Running: $description${NC}"
    echo -e "${BLUE}Command: cargo bench --bench $benchmark_name${NC}"

    if cargo bench --bench "$benchmark_name" 2>&1; then
        echo -e "${GREEN}✓ $description completed successfully${NC}"
    else
        echo -e "${RED}✗ $description failed${NC}"
        return 1
    fi
}

# Function to run specific dimension benchmarks
run_dimension_benchmark() {
    local dimension="$1"

    echo -e "\n${YELLOW}Running benchmarks for ${dimension}D vectors${NC}"
    echo -e "${BLUE}Command: cargo bench --bench bench_15_encoding_strategies -- $dimension${NC}"

    if cargo bench --bench bench_15_encoding_strategies -- "$dimension" 2>&1; then
        echo -e "${GREEN}✓ ${dimension}D benchmark completed${NC}"
    else
        echo -e "${RED}✗ ${dimension}D benchmark failed${NC}"
        return 1
    fi
}

# Build the project first
echo -e "\n${YELLOW}Building ProximaDB in release mode...${NC}"
if ! cargo build --release; then
    echo -e "${RED}✗ Build failed. Please fix compilation errors first.${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Build successful${NC}"

# Check if benchmark exists
if [ ! -f "benches/bench_15_encoding_strategies.rs" ]; then
    echo -e "${RED}✗ Encoding benchmark file not found: benches/bench_15_encoding_strategies.rs${NC}"
    echo -e "${YELLOW}Please ensure the benchmark file exists before running.${NC}"
    exit 1
fi

# Main benchmark options
echo -e "\n${BLUE}Available benchmark options:${NC}"
echo "1. Encoding strategies benchmark"
echo "2. Decoding strategies benchmark"
echo "3. Round-trip performance (encode + decode)"
echo "4. Specific dimension testing"
echo "5. WORM workload simulation"
echo "6. Real-time workload simulation"
echo "7. Balanced workload simulation"
echo "8. Compression algorithm comparison (decode)"
echo "9. Query pattern simulation"
echo "10. Memory efficiency during decode"
echo "11. Compression ratio analysis"
echo "12. All benchmarks (comprehensive)"
echo "13. Generate HTML reports"

read -p "Select benchmark option (1-13): " option

case $option in
    1)
        echo -e "\n${BLUE}Running encoding strategies benchmark...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- encoding_strategies
        ;;
    2)
        echo -e "\n${BLUE}Running decoding strategies benchmark...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- decoding_strategies
        ;;
    3)
        echo -e "\n${BLUE}Running round-trip performance (encode + decode)...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- roundtrip_performance
        ;;
    4)
        read -p "Enter dimension (384, 768, 1536, 3072): " dimension
        if [[ "$dimension" =~ ^(384|768|1536|3072)$ ]]; then
            run_dimension_benchmark "$dimension"
        else
            echo -e "${RED}Invalid dimension. Please use 384, 768, 1536, or 3072${NC}"
            exit 1
        fi
        ;;
    5)
        echo -e "\n${BLUE}Running WORM workload simulation...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- worm_workload
        ;;
    6)
        echo -e "\n${BLUE}Running real-time workload simulation...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- realtime_workload
        ;;
    7)
        echo -e "\n${BLUE}Running balanced workload simulation...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- balanced_workload
        ;;
    8)
        echo -e "\n${BLUE}Running compression algorithm comparison (decode)...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- decode_compression_algorithms
        ;;
    9)
        echo -e "\n${BLUE}Running query pattern simulation...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- query_pattern_simulation
        ;;
    10)
        echo -e "\n${BLUE}Running memory efficiency during decode...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- decode_memory_efficiency
        ;;
    11)
        echo -e "\n${BLUE}Running compression ratio analysis...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- compression_ratios
        ;;
    12)
        echo -e "\n${BLUE}Running comprehensive benchmark suite...${NC}"

        # Run all benchmark categories
        echo -e "${YELLOW}Running encoding strategies...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- encoding_strategies

        echo -e "${YELLOW}Running decoding strategies...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- decoding_strategies

        echo -e "${YELLOW}Running round-trip performance...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- roundtrip_performance

        echo -e "${YELLOW}Running workload simulations...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- worm_workload
        cargo bench --bench bench_15_encoding_strategies -- realtime_workload
        cargo bench --bench bench_15_encoding_strategies -- balanced_workload

        echo -e "${YELLOW}Running algorithm comparisons...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- compression_tradeoffs
        cargo bench --bench bench_15_encoding_strategies -- decode_compression_algorithms

        echo -e "${YELLOW}Running query simulations...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- query_pattern_simulation
        cargo bench --bench bench_15_encoding_strategies -- decode_memory_efficiency

        echo -e "${YELLOW}Running compression analysis...${NC}"
        cargo bench --bench bench_15_encoding_strategies -- compression_ratios

        echo -e "\n${GREEN}========================================${NC}"
        echo -e "${GREEN}All benchmarks completed successfully!${NC}"
        echo -e "${GREEN}========================================${NC}"
        ;;
    13)
        echo -e "\n${BLUE}Generating HTML reports...${NC}"

        # Create benchmark reports directory
        mkdir -p target/criterion

        # Run benchmarks with HTML output
        cargo bench --bench bench_15_encoding_strategies -- --output-format html

        echo -e "\n${GREEN}HTML reports generated in target/criterion/reports/index.html${NC}"
        echo -e "${YELLOW}Open the reports in your browser to view detailed performance analysis${NC}"
        ;;
    *)
        echo -e "${RED}Invalid option. Please select 1-13.${NC}"
        exit 1
        ;;
esac

# Performance summary
echo -e "\n${BLUE}========================================${NC}"
echo -e "${BLUE}Benchmark Summary${NC}"
echo -e "${BLUE}========================================${NC}"
echo -e "${YELLOW}Key Performance Insights:${NC}"
echo "• Columnar encoding: 17x compression, 38-77ms latency"
echo "• Row-wise encoding: <1ms latency, no compression overhead"
echo "• Auto mode: Intelligent dimension-based selection"
echo ""
echo -e "${YELLOW}Configuration Recommendations:${NC}"
echo "• WORM workloads: Use columnar encoding (94% storage reduction)"
echo "• Real-time workloads: Use row-wise encoding (<1ms latency)"
echo "• Balanced workloads: Use auto mode (dimension-based selection)"
echo ""
echo -e "${YELLOW}For detailed configuration guidance, see:${NC}"
echo "• docs/ENCODING_PERFORMANCE.adoc"
echo "• docs/PERFORMANCE.adoc"
echo ""
echo -e "${GREEN}Benchmark run completed!${NC}"