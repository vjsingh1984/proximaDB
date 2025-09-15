#!/bin/bash
# Sustained Load Testing Script for ProximaDB
# Implements the sustained load testing framework from task_3_performance_validation_design.adoc

set -e

echo "🚀 ProximaDB Sustained Load Testing Framework"
echo "============================================="

# Configuration with defaults
TEST_DURATION_HOURS=${1:-24}  # Default 24 hour test
CONCURRENT_CLIENTS=${2:-100}  # Default 100 concurrent clients
QPS_TARGET=${3:-1000}        # Target QPS
RESULTS_DIR="sustained_load_results_$(date +%Y%m%d_%H%M%S)"

# Create results directory
mkdir -p "$RESULTS_DIR"

echo "Test Configuration:"
echo "- Duration: $TEST_DURATION_HOURS hours"
echo "- Concurrent Clients: $CONCURRENT_CLIENTS"
echo "- Target QPS: $QPS_TARGET"
echo "- Results Directory: $RESULTS_DIR"
echo ""

# Validate ProximaDB binaries exist
if [ ! -f "target/release/proximadb-server" ]; then
    echo "❌ ProximaDB server binary not found. Building release version..."
    cargo build --release --bin proximadb-server

    if [ ! -f "target/release/proximadb-server" ]; then
        echo "💥 Failed to build ProximaDB server"
        exit 1
    fi
fi

if [ ! -f "target/release/proximadb-bench" ]; then
    echo "📊 Building ProximaDB benchmark binary..."
    cargo build --release --bin proximadb-bench
fi

# Start ProximaDB server in background
echo "🖥️ Starting ProximaDB server..."
./target/release/proximadb-server --config config/config.toml > "$RESULTS_DIR/server.log" 2>&1 &
SERVER_PID=$!

# Wait for server to start and test connectivity
echo "⏳ Waiting for server startup..."
for i in {1..30}; do
    if curl -s -f http://localhost:5678/health > /dev/null 2>&1; then
        echo "✅ ProximaDB server started successfully (PID: $SERVER_PID)"
        break
    fi

    if [ $i -eq 30 ]; then
        echo "❌ Server failed to start within 30 seconds"
        if kill -0 $SERVER_PID 2>/dev/null; then
            echo "📋 Server process is running but not responding to health checks"
            echo "Server log (last 20 lines):"
            tail -20 "$RESULTS_DIR/server.log"
        else
            echo "💥 Server process died during startup"
            echo "Server log:"
            cat "$RESULTS_DIR/server.log"
        fi
        exit 1
    fi

    sleep 1
done

# Start performance monitoring in background
echo "📈 Starting performance monitoring..."
python3 -c "
import json
import time
import requests
import sys
from datetime import datetime

def monitor_performance(duration_hours, output_file):
    results = []
    start_time = time.time()
    end_time = start_time + (duration_hours * 3600)

    print(f'📊 Performance monitoring started for {duration_hours} hours')

    while time.time() < end_time:
        try:
            # Get current metrics from server
            response = requests.get('http://localhost:5678/metrics', timeout=5)
            if response.status_code == 200:
                metrics = response.json() if response.content else {}
            else:
                metrics = {'error': f'HTTP {response.status_code}'}
        except Exception as e:
            metrics = {'error': str(e)}

        # Add timestamp
        metrics['timestamp'] = datetime.utcnow().isoformat()
        metrics['elapsed_hours'] = (time.time() - start_time) / 3600

        results.append(metrics)

        # Log progress every hour
        elapsed_hours = (time.time() - start_time) / 3600
        if len(results) % 60 == 0:  # Log every 60 iterations (roughly every hour with 1min intervals)
            current_qps = metrics.get('queries_per_second', 0)
            print(f'⏱️ Hour {elapsed_hours:.1f}: QPS={current_qps}')

        time.sleep(60)  # Monitor every minute

    # Save results
    with open(output_file, 'w') as f:
        json.dump(results, f, indent=2)

    print(f'✅ Performance monitoring complete: {len(results)} data points')

monitor_performance($TEST_DURATION_HOURS, '$RESULTS_DIR/performance_metrics.json')
" &
MONITOR_PID=$!

# Generate realistic sustained load
echo "🔥 Starting sustained load generation..."
echo "Creating $CONCURRENT_CLIENTS concurrent client processes..."

CLIENT_PIDS=()

# Create load generation script for each client
for i in $(seq 1 $CONCURRENT_CLIENTS); do
    python3 -c "
import requests
import json
import time
import random
import sys
from datetime import datetime

def generate_sustained_load(client_id, duration_hours, qps_target, output_file):
    results = {
        'client_id': client_id,
        'duration_hours': duration_hours,
        'target_qps': qps_target,
        'total_queries': 0,
        'successful_queries': 0,
        'failed_queries': 0,
        'total_response_time': 0,
        'start_time': datetime.utcnow().isoformat(),
        'query_log': []
    }

    start_time = time.time()
    end_time = start_time + (duration_hours * 3600)
    query_interval = 1.0 / qps_target if qps_target > 0 else 1.0

    print(f'🤖 Client {client_id} starting sustained load: {qps_target} QPS for {duration_hours}h')

    while time.time() < end_time:
        query_start = time.time()

        try:
            # Generate realistic vector search query
            query_data = {
                'collection_id': f'test_collection_{client_id % 10}',
                'vector': [random.uniform(-1, 1) for _ in range(384)],  # 384-dim vector
                'k': random.choice([10, 20, 50]),
                'metadata_filters': {
                    'client_id': str(client_id),
                    'timestamp': str(int(time.time()))
                } if random.random() < 0.3 else None
            }

            # Execute vector search
            response = requests.post(
                'http://localhost:5678/api/v1/search',
                json=query_data,
                timeout=10
            )

            query_duration = time.time() - query_start
            results['total_queries'] += 1
            results['total_response_time'] += query_duration

            if response.status_code == 200:
                results['successful_queries'] += 1
            else:
                results['failed_queries'] += 1
                if results['failed_queries'] <= 10:  # Log first 10 failures
                    results['query_log'].append({
                        'timestamp': datetime.utcnow().isoformat(),
                        'error': f'HTTP {response.status_code}',
                        'response_time': query_duration
                    })

        except Exception as e:
            results['failed_queries'] += 1
            query_duration = time.time() - query_start
            results['total_response_time'] += query_duration

            if results['failed_queries'] <= 10:  # Log first 10 failures
                results['query_log'].append({
                    'timestamp': datetime.utcnow().isoformat(),
                    'error': str(e),
                    'response_time': query_duration
                })

        # Maintain target QPS
        elapsed = time.time() - query_start
        if elapsed < query_interval:
            time.sleep(query_interval - elapsed)

    # Calculate final metrics
    total_duration = time.time() - start_time
    results['end_time'] = datetime.utcnow().isoformat()
    results['actual_duration_seconds'] = total_duration
    results['actual_qps'] = results['total_queries'] / total_duration if total_duration > 0 else 0
    results['avg_response_time_ms'] = (results['total_response_time'] / results['total_queries'] * 1000) if results['total_queries'] > 0 else 0
    results['success_rate'] = (results['successful_queries'] / results['total_queries'] * 100) if results['total_queries'] > 0 else 0

    # Save results
    with open(output_file, 'w') as f:
        json.dump(results, f, indent=2)

    print(f'✅ Client {client_id} completed: {results[\"total_queries\"]} queries, {results[\"success_rate\"]:.1f}% success rate')

generate_sustained_load($i, $TEST_DURATION_HOURS, $((QPS_TARGET / CONCURRENT_CLIENTS)), '$RESULTS_DIR/client_$i.json')
" &

    CLIENT_PIDS+=($!)

    # Stagger client startup to avoid thundering herd
    sleep 0.1
done

echo "✅ Started $CONCURRENT_CLIENTS load generation clients"
echo "🔍 Monitor progress: tail -f $RESULTS_DIR/server.log"
echo ""

# Monitor test progress
START_TIME=$(date +%s)
END_TIME=$((START_TIME + TEST_DURATION_HOURS * 3600))

echo "⏱️ Test Progress Monitoring:"
while [ $(date +%s) -lt $END_TIME ]; do
    ELAPSED_HOURS=$(( ($(date +%s) - START_TIME) / 3600 ))
    REMAINING_HOURS=$(( TEST_DURATION_HOURS - ELAPSED_HOURS ))

    echo "📊 Progress: ${ELAPSED_HOURS}/${TEST_DURATION_HOURS} hours complete (${REMAINING_HOURS}h remaining)"

    # Check server health
    if ! kill -0 $SERVER_PID 2>/dev/null; then
        echo "💥 Server crashed during sustained load test!"
        echo "📋 Server log (last 50 lines):"
        tail -50 "$RESULTS_DIR/server.log"
        break
    fi

    # Check current performance
    CURRENT_QPS=$(curl -s http://localhost:5678/metrics 2>/dev/null | jq -r '.queries_per_second // "N/A"' 2>/dev/null || echo "N/A")
    MEMORY_USAGE=$(ps -p $SERVER_PID -o %mem --no-headers 2>/dev/null || echo "N/A")
    CPU_USAGE=$(ps -p $SERVER_PID -o %cpu --no-headers 2>/dev/null || echo "N/A")

    echo "📈 Current Metrics: QPS=$CURRENT_QPS | Memory=$MEMORY_USAGE% | CPU=$CPU_USAGE%"

    # Check client health
    ACTIVE_CLIENTS=0
    for pid in "${CLIENT_PIDS[@]}"; do
        if kill -0 $pid 2>/dev/null; then
            ACTIVE_CLIENTS=$((ACTIVE_CLIENTS + 1))
        fi
    done

    echo "🤖 Active Clients: $ACTIVE_CLIENTS/$CONCURRENT_CLIENTS"
    echo ""

    sleep 3600  # Check every hour
done

# Stop all processes
echo "🛑 Stopping sustained load test..."

# Stop client processes
echo "🤖 Stopping $CONCURRENT_CLIENTS client processes..."
for pid in "${CLIENT_PIDS[@]}"; do
    kill $pid 2>/dev/null || true
done

# Stop performance monitor
echo "📈 Stopping performance monitor..."
kill $MONITOR_PID 2>/dev/null || true

# Stop server
echo "🖥️ Stopping ProximaDB server..."
kill $SERVER_PID 2>/dev/null || true

# Wait for graceful shutdown
sleep 5

# Force kill if necessary
kill -9 $SERVER_PID 2>/dev/null || true

echo "✅ All processes stopped"

# Generate comprehensive test report
echo ""
echo "📋 Generating sustained load test report..."
python3 scripts/performance_reporting/generate_performance_report.py \
    --results-dir "$RESULTS_DIR" \
    --output "$RESULTS_DIR/sustained_load_report.html" \
    --format html

# Generate summary statistics
echo ""
echo "📊 SUSTAINED LOAD TEST SUMMARY"
echo "=============================="

python3 -c "
import json
import glob
import os
import sys

results_dir = '$RESULTS_DIR'
client_files = glob.glob(os.path.join(results_dir, 'client_*.json'))

if not client_files:
    print('❌ No client result files found')
    sys.exit(1)

total_queries = 0
total_errors = 0
total_response_time = 0
client_results = []

print(f'📁 Processing {len(client_files)} client result files...')

for file in client_files:
    try:
        with open(file) as f:
            data = json.load(f)
            total_queries += data.get('total_queries', 0)
            total_errors += data.get('failed_queries', 0)
            total_response_time += data.get('total_response_time', 0)
            client_results.append(data)
    except Exception as e:
        print(f'⚠️ Error processing {file}: {e}')

if total_queries > 0:
    success_rate = ((total_queries - total_errors) / total_queries * 100)
    avg_response_time = (total_response_time / total_queries * 1000)
    actual_qps = total_queries / ($TEST_DURATION_HOURS * 3600)

    print()
    print('PERFORMANCE RESULTS:')
    print('==================')
    print(f'Total Queries Executed: {total_queries:,}')
    print(f'Total Errors: {total_errors:,}')
    print(f'Success Rate: {success_rate:.2f}%')
    print(f'Target QPS: $QPS_TARGET')
    print(f'Actual Average QPS: {actual_qps:.1f}')
    print(f'QPS Achievement: {(actual_qps / $QPS_TARGET * 100):.1f}%')
    print(f'Average Response Time: {avg_response_time:.2f}ms')
    print()

    # Performance assessment
    if success_rate >= 99.0 and actual_qps >= $QPS_TARGET * 0.9:
        print('🎉 EXCELLENT: Sustained load test passed with excellent performance!')
        assessment = 'EXCELLENT'
    elif success_rate >= 95.0 and actual_qps >= $QPS_TARGET * 0.8:
        print('✅ GOOD: Sustained load test passed with good performance.')
        assessment = 'GOOD'
    elif success_rate >= 90.0 and actual_qps >= $QPS_TARGET * 0.7:
        print('⚠️ ACCEPTABLE: Sustained load test passed but performance could be improved.')
        assessment = 'ACCEPTABLE'
    else:
        print('❌ NEEDS IMPROVEMENT: Sustained load test performance below expectations.')
        assessment = 'NEEDS_IMPROVEMENT'

    # Client performance distribution
    client_qps = [c.get('actual_qps', 0) for c in client_results if c.get('actual_qps', 0) > 0]
    if client_qps:
        print(f'Client QPS Distribution:')
        print(f'  Min: {min(client_qps):.1f} QPS')
        print(f'  Max: {max(client_qps):.1f} QPS')
        print(f'  Avg: {sum(client_qps) / len(client_qps):.1f} QPS')
        print(f'  Std Dev: {(sum([(x - sum(client_qps) / len(client_qps))**2 for x in client_qps]) / len(client_qps))**0.5:.1f}')

    # Save summary
    summary = {
        'test_duration_hours': $TEST_DURATION_HOURS,
        'concurrent_clients': $CONCURRENT_CLIENTS,
        'target_qps': $QPS_TARGET,
        'actual_qps': actual_qps,
        'total_queries': total_queries,
        'success_rate': success_rate,
        'avg_response_time_ms': avg_response_time,
        'performance_assessment': assessment,
        'timestamp': '$(date -u +%Y-%m-%dT%H:%M:%SZ)'
    }

    with open('$RESULTS_DIR/test_summary.json', 'w') as f:
        json.dump(summary, f, indent=2)

else:
    print('❌ No valid client data found - test failed')
    assessment = 'FAILED'

print()
print('📁 RESULTS LOCATION:')
print('==================')
print(f'Results Directory: $RESULTS_DIR')
print(f'Performance Report: $RESULTS_DIR/sustained_load_report.html')
print(f'Test Summary: $RESULTS_DIR/test_summary.json')
print(f'Server Log: $RESULTS_DIR/server.log')
print(f'Performance Metrics: $RESULTS_DIR/performance_metrics.json')
"

echo ""
echo "🏁 SUSTAINED LOAD TEST COMPLETE"
echo "==============================="
echo "Assessment: $(cat $RESULTS_DIR/test_summary.json 2>/dev/null | jq -r '.performance_assessment // \"UNKNOWN\"')"
echo "📊 View detailed report: open $RESULTS_DIR/sustained_load_report.html"