#!/bin/bash
# Setup ProximaDB Test Environment
# Clears stale metadata to allow fresh collection creation

set -e  # Exit on error

echo "🧪 ProximaDB Test Environment Setup"
echo "===================================="
echo ""

# Stop any running servers
echo "1. Stopping any running ProximaDB servers..."
pkill -9 proximadb-server 2>/dev/null || true
sleep 2
echo "   ✅ Servers stopped"

# Clear stale metadata and data
echo ""
echo "2. Clearing stale metadata and data..."
rm -rf /tmp/proximadb/metadata/current/* 2>/dev/null || true
rm -rf /tmp/proximadb/data/* 2>/dev/null || true
rm -rf /tmp/proximadb/d1/* 2>/dev/null || true
rm -rf /tmp/proximadb/d2/* 2>/dev/null || true
rm -rf /tmp/proximadb/d3/* 2>/dev/null || true
rm -rf /tmp/proximadb/manifest/* 2>/dev/null || true
echo "   ✅ Metadata and data cleared"

# Clear Python cache
echo ""
echo "3. Clearing Python cache files..."
find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
find . -name "*.pyc" -delete 2>/dev/null || true
find . -name "*.pyo" -delete 2>/dev/null || true
rm -rf .pytest_cache 2>/dev/null || true
echo "   ✅ Python cache cleared"

# Start fresh server in background
echo ""
echo "4. Starting fresh ProximaDB server..."
cd /Users/vijaysingh/code/proximaDB
cargo run --bin proximadb-server -- --config config/config.toml > /tmp/server.log 2>&1 &
SERVER_PID=$!

echo "   Server PID: $SERVER_PID"
echo "   Logs: /tmp/server.log"
echo "   Waiting for server to be ready..."

# Wait for server to be ready
MAX_WAIT=30
WAITED=0
while [ $WAITED -lt $MAX_WAIT ]; do
    if curl -s http://localhost:5678/health >/dev/null 2>&1; then
        echo "   ✅ Server is ready!"
        break
    fi
    sleep 1
    WAITED=$((WAITED + 1))
    echo -n "."
done

if [ $WAITED -ge $MAX_WAIT ]; then
    echo ""
    echo "   ❌ Server failed to start within ${MAX_WAIT} seconds"
    echo "   Check logs: tail -50 /tmp/server.log"
    exit 1
fi

echo ""
echo ""
echo "✅ Test environment ready!"
echo ""
echo "Server Status:"
echo "  - REST API: http://localhost:5678"
echo "  - gRPC API: localhost:5679"
echo "  - Health: http://localhost:5678/health"
echo "  - Logs: /tmp/server.log"
echo ""
echo "Run tests with:"
echo "  cd clients/python"
echo "  ./run_tests_no_cache.sh tests/unit/ -v"
echo ""
echo "Or:"
echo "  PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=src python -m pytest tests/unit/ -v"
echo ""
echo "Stop server with:"
echo "  kill $SERVER_PID"
echo "  # or"
echo "  pkill -9 proximadb-server"
echo ""
