#!/bin/bash
# ProximaDB Comprehensive Benchmark Runner
# Runs all performance benchmarks with proper environment setup

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Set PYTHONPATH for imports
export PYTHONPATH="/home/vsingh/code/proximaDB/clients/python/src:/home/vsingh/code/proximaDB/demo:$PYTHONPATH"

# Check if server is running
echo -e "${BLUE}🔍 Checking ProximaDB server status...${NC}"
if ! curl -s http://localhost:5678/health > /dev/null; then
    echo -e "${RED}❌ ProximaDB REST server is not running on port 5678${NC}"
    echo "   Please start the server with: ./target/release/proximadb-server --config demo/local-demo-config.toml"
    exit 1
fi

if ! nc -z localhost 5679 2>/dev/null; then
    echo -e "${RED}❌ ProximaDB gRPC server is not running on port 5679${NC}"
    echo "   Please start the server with: ./target/release/proximadb-server --config demo/local-demo-config.toml"
    exit 1
fi

echo -e "${GREEN}✅ ProximaDB server is running${NC}\n"

# Create results directory
mkdir -p demo_results

# Get current timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)

# Run the benchmark suite
echo -e "${BLUE}🚀 Running ProximaDB Comprehensive Performance Benchmark Suite${NC}"
echo "============================================================"

# Default to comprehensive suite if not specified
SUITE="${1:-comprehensive}"

# Run performance suite and save results
python performance_suite.py --suite "$SUITE" "${@:2}" 2>&1 | tee "demo_results/benchmark_run_${TIMESTAMP}.log"

echo -e "\n${GREEN}✅ Benchmark completed!${NC}"
echo -e "📊 Results saved to: demo_results/benchmark_run_${TIMESTAMP}.log\n"

# Show summary of key metrics
echo -e "${BLUE}📈 Key Performance Metrics:${NC}"
tail -20 "demo_results/benchmark_run_${TIMESTAMP}.log" | grep -E "(Maximum Insert Rate|Minimum Search Latency|BY STORAGE ENGINE|BY PROTOCOL)" || true