#!/bin/bash
# Graph Database Benchmarks for ProximaDB
# Runs LDBC SNB (Social Network Benchmark)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"

# Create results directory with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$BENCH_DIR/results/graph/$TIMESTAMP"
mkdir -p "$RESULTS_DIR"

echo "🚀 ProximaDB Graph Benchmarks (LDBC SNB)"
echo "========================================"
echo "Results directory: $RESULTS_DIR"
echo ""

# Check if Java and Maven are available
if ! command -v java &> /dev/null || ! command -v mvn &> /dev/null; then
    echo "❌ Java and Maven required for LDBC"
    echo "   Install with: brew install openjdk maven (macOS)"
    exit 1
fi

# Check if ProximaDB is running
echo "🔍 Checking ProximaDB server..."
if ! curl -s http://localhost:5678/health > /dev/null; then
    echo "❌ ProximaDB server not running at http://localhost:5678"
    echo "   Start with: cargo run --bin proximadb-server"
    exit 1
fi
echo "✅ ProximaDB server is running"
echo ""

# Generate test data (if not exists)
LDBC_DIR="$BENCH_DIR/ldbc"
SCALE_FACTOR="SF1"  # 1GB dataset

echo "📊 Generating LDBC SNB test data ($SCALE_FACTOR)..."
cd "$LDBC_DIR/datagen"

if [ ! -d "social_network_$SCALE_FACTOR" ]; then
    mvn clean package > /dev/null 2>&1
    java -jar target/ldbc_snb_datagen-*.jar \
        -sf 1 \
        -o "$LDBC_DIR/social_network_$SCALE_FACTOR" \
        2>&1 | tee "$RESULTS_DIR/datagen.log"
    echo "✅ Test data generated"
else
    echo "✅ Test data already exists"
fi
echo ""

# Run LDBC SNB Interactive benchmark
echo "📊 Running LDBC SNB Interactive benchmark..."
cd "$LDBC_DIR/driver"

# Copy ProximaDB configuration
cp "$BENCH_DIR/configs/ldbc/proximadb.properties" config/proximadb.properties

# Build driver
mvn clean package > /dev/null 2>&1

# Run benchmark
java -jar target/ldbc_snb_driver-*.jar \
    -c config/proximadb.properties \
    -P workloads="$LDBC_DIR/implementations/interactive/interactive_workload.ini" \
    2>&1 | tee "$RESULTS_DIR/ldbc_snb.log"

echo "✅ LDBC SNB Interactive complete"
echo ""

# Generate summary
echo "📈 Generating summary..."
cat > "$RESULTS_DIR/summary.txt" << EOF
ProximaDB Graph Benchmark Results (LDBC SNB)
============================================
Timestamp: $TIMESTAMP
Scale Factor: SF1 (1GB dataset)

Interactive Workload Results:
EOF

# Extract key metrics from LDBC results
python - <<PYTHON
import re

log_file = '$RESULTS_DIR/ldbc_snb.log'
with open(log_file, 'r') as f:
    content = f.read()

# Extract operation counts and timings
operations = re.findall(r'Operation (\d+):.*?(\d+\.\d+) ms', content, re.DOTALL)
total_ops = len(operations)

if operations:
    total_time = sum(float(op[1]) for op in operations)
    avg_time = total_time / total_ops if total_ops > 0 else 0

    print(f"  Total operations: {total_ops}")
    print(f"  Total time: {total_time:.2f} ms")
    print(f"  Average latency: {avg_time:.2f} ms")
    print(f"  Throughput: {total_ops / (total_time / 1000):.2f} ops/sec")

# Extract memory usage if available
memory_match = re.search(r'Memory: (\d+) MB', content)
if memory_match:
    print(f"  Memory usage: {memory_match.group(1)} MB")
PYTHON

echo ""
echo "✅ Summary generated: $RESULTS_DIR/summary.txt"
echo ""

# Comparison with competitors
echo "📊 Competitor Comparison (LDBC SNB SF1):"
echo "  Neo4j:        ~1,000 ops/sec (average)"
echo "  TigerGraph:   ~10,000 ops/sec (analytical queries)"
echo "  Amazon Neptune: ~800 ops/sec (average)"
echo ""

# Show actual ProximaDB results
if [ -f "$RESULTS_DIR/summary.txt" ]; then
    echo "📈 ProximaDB Results:"
    grep -A 10 "Interactive Workload Results:" "$RESULTS_DIR/summary.txt" | tail -5
    echo ""
fi

echo "========================================"
echo "✅ Graph benchmarks complete!"
echo "Results saved to: $RESULTS_DIR"
echo ""
echo "View summary: cat $RESULTS_DIR/summary.txt"
echo "View logs: cat $RESULTS_DIR/ldbc_snb.log"
echo ""

# Additional graph analytics benchmarks
echo "📊 Running graph analytics benchmarks..."
cd "$BENCH_DIR"

# Test BFS performance
echo "Testing BFS traversal..."
python3 - <<PYTHON
import requests
import time

server_url = "http://localhost:5678"

# Create test graph
graph_data = {
    "graph_id": "benchmark_graph",
    "name": "LDBC Benchmark Graph"
}

try:
    # Create graph
    response = requests.post(f"{server_url}/v1/graphs", json=graph_data)
    if response.status_code == 200:
        print("✅ Test graph created")

    # Measure BFS traversal
    start = time.time()
    response = requests.post(
        f"{server_url}/v1/graphs/benchmark_graph/traverse",
        json={
            "start_node_id": "0",
            "algorithm": "BFS",
            "max_depth": 5,
            "limit": 1000
        }
    )
    end = time.time()

    if response.status_code == 200:
        result = response.json()
        traversal_time = (end - start) * 1000  # Convert to ms
        print(f"  BFS traversal time: {traversal_time:.2f} ms")
        print(f"  Nodes visited: {len(result.get('nodes', []))}")

        with open('$RESULTS_DIR/bfs_results.txt', 'w') as f:
            f.write(f"BFS Traversal Results\n")
            f.write(f"====================\n")
            f.write(f"Traversal time: {traversal_time:.2f} ms\n")
            f.write(f"Nodes visited: {len(result.get('nodes', []))}\n")
            f.write(f"Edges traversed: {result.get('edge_count', 'N/A')}\n")

        print("✅ BFS results saved")

except Exception as e:
    print(f"⚠️  BFS test failed: {e}")
PYTHON

echo ""
echo "========================================"
echo "✅ All graph benchmarks complete!"
echo "Results saved to: $RESULTS_DIR"
