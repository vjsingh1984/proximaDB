#!/bin/bash
# Vector Database Benchmarks for ProximaDB
# Runs VectorDBBench and ANN-Benchmarks

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"

# Activate virtual environment
source "$BENCH_DIR/venv/bin/activate"

# Create results directory with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$BENCH_DIR/results/vector/$TIMESTAMP"
mkdir -p "$RESULTS_DIR"

echo "🚀 ProximaDB Vector Benchmarks"
echo "=============================="
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

# Run VectorDBBench
echo "📊 Running VectorDBBench..."
cd "$BENCH_DIR/vectordbbench"

# Run with SIFT dataset
python run.py \
    --config "$BENCH_DIR/configs/vectordbbench/proximadb_sift.yaml" \
    --db proximadb \
    --output "$RESULTS_DIR/vectordbbench_sift.json" \
    2>&1 | tee "$RESULTS_DIR/vectordbbench_sift.log"

echo "✅ VectorDBBench complete"
echo ""

# Run ANN-Benchmarks
echo "📊 Running ANN-Benchmarks..."
cd "$BENCH_DIR/ann-benchmarks"

# Install ProximaDB adapter if it exists
if [ -f "$BENCH_DIR/adapters/ann-benchmarks/proximadb.py" ]; then
    cp "$BENCH_DIR/adapters/ann-benchmarks/proximadb.py" algorithms/proximadb.py
fi

# Run with SIFT dataset
python ann_benchmarks.py \
    --dataset sift-1m \
    --algorithm proximadb_hnsw \
    --runs 3 \
    --local \
    2>&1 | tee "$RESULTS_DIR/ann_benchmarks_sift.log"

echo "✅ ANN-Benchmarks complete"
echo ""

# Generate summary
echo "📈 Generating summary..."
cat > "$RESULTS_DIR/summary.txt" << EOF
ProximaDB Vector Benchmark Results
==================================
Timestamp: $TIMESTAMP

VectorDBBench Results:
EOF

# Extract key metrics from VectorDBBench results
if [ -f "$RESULTS_DIR/vectordbbench_sift.json" ]; then
    python - <<PYTHON
import json
with open('$RESULTS_DIR/vectordbbench_sift.json') as f:
    data = json.load(f)
    print(f"  QPS: {data.get('qps', 'N/A')}")
    print(f"  Latency P95: {data.get('latency_p95', 'N/A')} ms")
    print(f"  Latency P99: {data.get('latency_p99', 'N/A')} ms")
    print(f"  Recall: {data.get('recall', 'N/A')}")
    print(f"  Memory: {data.get('memory_mb', 'N/A')} MB")
PYTHON
fi

cat >> "$RESULTS_DIR/summary.txt" << EOF

ANN-Benchmarks Results:
EOF

# Extract key metrics from ANN-Benchmarks results
if [ -d "$RESULTS_DIR/ann-benchmarks_sift" ]; then
    python - <<PYTHON
import os
import json

results_dir = '$RESULTS_DIR/ann-benchmarks_sift'
for file in os.listdir(results_dir):
    if file.endswith('.json'):
        with open(os.path.join(results_dir, file)) as f:
            data = json.load(f)
            print(f"  Algorithm: {data.get('algorithm', 'N/A')}")
            print(f"  QPS: {data.get('qps', 'N/A')}")
            print(f"  Recall: {data.get('recall', 'N/A')}")
            print(f"  Build time: {data.get('build_time', 'N/A')} s")
PYTHON
fi

echo ""
echo "✅ Summary generated: $RESULTS_DIR/summary.txt"
echo ""

# Comparison with competitors
echo "📊 Competitor Comparison (SIFT-1M, 97% recall):"
echo "  Milvus:     ~12,000 QPS"
echo "  Qdrant:     ~10,000 QPS"
echo "  Weaviate:   ~8,000 QPS"
echo "  Pinecone:   ~15,000 QPS (cloud)"
echo ""

# Show actual ProximaDB results
if [ -f "$RESULTS_DIR/summary.txt" ]; then
    echo "📈 ProximaDB Results:"
    grep -A 10 "VectorDBBench Results:" "$RESULTS_DIR/summary.txt" | tail -6
    echo ""
fi

echo "=============================="
echo "✅ Vector benchmarks complete!"
echo "Results saved to: $RESULTS_DIR"
echo ""
echo "View summary: cat $RESULTS_DIR/summary.txt"
echo "View logs: ls -la $RESULTS_DIR/*.log"
