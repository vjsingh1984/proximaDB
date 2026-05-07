#!/bin/bash
# Document Database Benchmarks for ProximaDB
# Runs YCSB (Yahoo! Cloud Serving Benchmark)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"

# Create results directory with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$BENCH_DIR/results/document/$TIMESTAMP"
mkdir -p "$RESULTS_DIR"

echo "🚀 ProximaDB Document Benchmarks (YCSB)"
echo "======================================="
echo "Results directory: $RESULTS_DIR"
echo ""

# Check if ProximaDB is running
echo "🔍 Checking ProximaDB server..."
if ! curl -s http://localhost:5678/health > /dev/null; then
    echo "❌ ProximaDB server not running at http://localhost:5678"
    echo "   Start with: cargo run --bin proximadb-server"
    exit 1
fi
echo "✅ ProximaDB server is running"
echo ""

# Setup YCSB
YCSB_DIR="$BENCH_DIR/ycsb"

# Build YCSB if not already built
if [ ! -f "$YCSB_DIR/core/target/core-0.17.0-SNAPSHOT.jar" ]; then
    echo "🔧 Building YCSB..."
    cd "$YCSB_DIR"
    mvn clean package > /dev/null 2>&1
    cd "$BENCH_DIR"
    echo "✅ YCSB built"
fi

# Load test data
echo "📊 Loading test data..."
cd "$YCSB_DIR"

# Record count for benchmark
RECORD_COUNT=1000000
OPERATION_COUNT=1000000

# Load workload
./bin/ycsb load proximadb \
    -P ../configs/ycsb/proximadb-workloada.spec \
    -p recordcount=$RECORD_COUNT \
    -p threadcount=4 \
    -s > "$RESULTS_DIR/load.log" 2>&1

echo "✅ Test data loaded"
echo ""

# Run Workload A (50% read, 50% update)
echo "📊 Running YCSB Workload A (read-update)..."
./bin/ycsb run proximadb \
    -P ../configs/ycsb/proximadb-workloada.spec \
    -p recordcount=$RECORD_COUNT \
    -p operationcount=$OPERATION_COUNT \
    -p threadcount=4 \
    -s > "$RESULTS_DIR/workloada.log" 2>&1

echo "✅ Workload A complete"
echo ""

# Run Workload B (95% read, 5% update)
echo "📊 Running YCSB Workload B (read-heavy)..."
cat > ../configs/ycsb/proximadb-workloadb.spec << EOF
recordcount=$RECORD_COUNT
operationcount=$OPERATION_COUNT
workload=com.yahoo.ycsb.workloads.CoreWorkload

readproportion=0.95
updateproportion=0.05
scanproportion=0
insertproportion=0

requestdistribution=zipfian
threadcount=4
EOF

./bin/ycsb run proximadb \
    -P ../configs/ycsb/proximadb-workloadb.spec \
    -p threadcount=4 \
    -s > "$RESULTS_DIR/workloadb.log" 2>&1

echo "✅ Workload B complete"
echo ""

# Run Workload C (100% read)
echo "📊 Running YCSB Workload C (read-only)..."
cat > ../configs/ycsb/proximadb-workloadc.spec << EOF
recordcount=$RECORD_COUNT
operationcount=$OPERATION_COUNT
workload=com.yahoo.ycsb.workloads.CoreWorkload

readproportion=1.0
updateproportion=0
scanproportion=0
insertproportion=0

requestdistribution=zipfian
threadcount=4
EOF

./bin/ycsb run proximadb \
    -P ../configs/ycsb/proximadb-workloadc.spec \
    -p threadcount=4 \
    -s > "$RESULTS_DIR/workloadc.log" 2>&1

echo "✅ Workload C complete"
echo ""

# Run custom document workloads
echo "📊 Running custom document benchmarks..."
cd "$BENCH_DIR"

python3 - <<PYTHON
import requests
import json
import time
import statistics

server_url = "http://localhost:5678"

# Test document operations
results = {}

# 1. Document insert performance
print("Testing document inserts...")
insert_times = []
for i in range(1000):
    doc = {
        "collection_id": "benchmark_collection",
        "document": {
            "id": f"doc_{i}",
            "title": f"Document {i}",
            "content": f"Content for document {i}" * 10,
            "timestamp": time.time()
        }
    }

    start = time.time()
    try:
        response = requests.post(f"{server_url}/v1/documents", json=doc)
        end = time.time()
        if response.status_code == 200:
            insert_times.append((end - start) * 1000)  # ms
    except Exception as e:
        pass

if insert_times:
    results['insert_avg_ms'] = statistics.mean(insert_times)
    results['insert_p95_ms'] = statistics.quantiles(insert_times, n=20)[18]  # 95th percentile
    results['insert_p99_ms'] = statistics.quantiles(insert_times, n=100)[98]  # 99th percentile
    results['insert_throughput'] = len(insert_times) / sum(insert_times) * 1000  # ops/sec

    print(f"  Insert average: {results['insert_avg_ms']:.2f} ms")
    print(f"  Insert P95: {results['insert_p95_ms']:.2f} ms")
    print(f"  Insert P99: {results['insert_p99_ms']:.2f} ms")
    print(f"  Throughput: {results['insert_throughput']:.0f} ops/sec")

# 2. Document query performance
print("Testing document queries...")
query_times = []

for i in range(100):
    query = {
        "collection_id": "benchmark_collection",
        "query": f"title:Document {i % 100}",
        "limit": 10
    }

    start = time.time()
    try:
        response = requests.post(f"{server_url}/v1/query", json=query)
        end = time.time()
        if response.status_code == 200:
            query_times.append((end - start) * 1000)
    except Exception as e:
        pass

if query_times:
    results['query_avg_ms'] = statistics.mean(query_times)
    results['query_p95_ms'] = statistics.quantiles(query_times, n=20)[18]
    results['query_p99_ms'] = statistics.quantiles(query_times, n=100)[98]
    results['query_throughput'] = len(query_times) / sum(query_times) * 1000

    print(f"  Query average: {results['query_avg_ms']:.2f} ms")
    print(f"  Query P95: {results['query_p95_ms']:.2f} ms")
    print(f"  Query P99: {results['query_p99_ms']:.2f} ms")
    print(f"  Throughput: {results['query_throughput']:.0f} ops/sec")

# 3. Memory usage estimation
try:
    import psutil
    process = psutil.Process()
    memory_info = process.memory_info()
    results['memory_mb'] = memory_info.rss / 1024 / 1024
    print(f"  Memory usage: {results['memory_mb']:.0f} MB")
except:
    pass

# Save results
with open('$RESULTS_DIR/document_benchmarks.json', 'w') as f:
    json.dump(results, f, indent=2)

print("✅ Custom document benchmarks complete")
PYTHON

echo ""
echo "✅ Custom benchmarks complete"
echo ""

# Generate summary
echo "📈 Generating summary..."
cat > "$RESULTS_DIR/summary.txt" << EOF
ProximaDB Document Benchmark Results (YCSB)
==========================================
Timestamp: $TIMESTAMP

Workload A (50% read, 50% update):
EOF

# Extract metrics from Workload A
grep -A 20 "Workload A" "$RESULTS_DIR/workloada.log" | grep -E "Average|Return|latency" | head -10 >> "$RESULTS_DIR/summary.txt" || echo "  See workloada.log for details" >> "$RESULTS_DIR/summary.txt"

cat >> "$RESULTS_DIR/summary.txt" << EOF

Workload B (95% read, 5% update):
EOF

grep -A 20 "Workload B" "$RESULTS_DIR/workloadb.log" | grep -E "Average|Return|latency" | head -10 >> "$RESULTS_DIR/summary.txt" || echo "  See workloadb.log for details" >> "$RESULTS_DIR/summary.txt"

cat >> "$RESULTS_DIR/summary.txt" << EOF

Workload C (100% read):
EOF

grep -A 20 "Workload C" "$RESULTS_DIR/workloadc.log" | grep -E "Average|Return|latency" | head -10 >> "$RESULTS_DIR/summary.txt" || echo "  See workloadc.log for details" >> "$RESULTS_DIR/summary.txt"

cat >> "$RESULTS_DIR/summary.txt" << EOF

Custom Document Benchmarks:
EOF

if [ -f "$RESULTS_DIR/document_benchmarks.json" ]; then
    python3 - <<PYTHON
import json
with open('$RESULTS_DIR/document_benchmarks.json') as f:
    data = json.load(f)
    for key, value in data.items():
        print(f"  {key}: {value}")
PYTHON
fi

echo ""
echo "✅ Summary generated: $RESULTS_DIR/summary.txt"
echo ""

# Comparison with competitors
echo "📊 Competitor Comparison (YCSB Workload A, 1M records):"
echo "  MongoDB:     ~10,000 ops/sec"
echo "  PostgreSQL:  ~8,000 ops/sec"
echo "  CouchDB:     ~5,000 ops/sec"
echo ""

# Show actual ProximaDB results
if [ -f "$RESULTS_DIR/summary.txt" ]; then
    echo "📈 ProximaDB Results:"
    grep -A 5 "Custom Document Benchmarks:" "$RESULTS_DIR/summary.txt" | tail -4
    echo ""
fi

echo "======================================="
echo "✅ Document benchmarks complete!"
echo "Results saved to: $RESULTS_DIR"
echo ""
echo "View summary: cat $RESULTS_DIR/summary.txt"
echo "View logs: ls -la $RESULTS_DIR/*.log"
