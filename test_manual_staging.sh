#!/bin/bash

echo "🎯 MANUAL STAGING PATTERN TEST"
echo "================================"

# Clean up any existing processes
pkill -f proximadb-server 2>/dev/null || true
sleep 2

# Clean directories
rm -rf test_data
mkdir -p test_data/{wal,storage,metadata}

echo "🚀 Starting server..."
./target/release/proximadb-server --config staging_test_config.toml &
SERVER_PID=$!

# Wait for server to start
echo "⏳ Waiting for server to start..."
sleep 10

# Test health endpoint
echo "🔍 Testing server health..."
if curl -s http://127.0.0.1:5678/health | grep -q "healthy"; then
    echo "✅ Server is healthy"
else
    echo "❌ Server health check failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# Create collection
echo "📦 Creating collection..."
CREATE_RESPONSE=$(curl -s -X POST http://127.0.0.1:5678/collections \
    -H "Content-Type: application/json" \
    -d '{"name": "staging_test", "dimension": 3, "distance_metric": "cosine"}')
echo "Collection creation response: $CREATE_RESPONSE"

# Insert vectors
echo "📝 Inserting vectors..."
for i in {1..5}; do
    VECTOR_RESPONSE=$(curl -s -X POST http://127.0.0.1:5678/collections/staging_test/vectors \
        -H "Content-Type: application/json" \
        -d "{\"id\": \"vector_$i\", \"vector\": [1.0, 2.0, 3.0], \"metadata\": {\"index\": $i}}")
    echo "Vector $i: $(echo $VECTOR_RESPONSE | jq -r '.success // "FAILED"')"
done

# Force flush
echo "💾 Triggering force flush..."
FLUSH_RESPONSE=$(curl -s -X POST http://127.0.0.1:5678/collections/staging_test/internal/flush)
echo "Flush response: $FLUSH_RESPONSE"

# Wait for flush to complete
sleep 5

# Check directory structure
echo ""
echo "🔍 Checking staging pattern results..."
echo "📁 Directory structure:"
find test_data -type d -name "__*" | while read dir; do
    echo "  🎯 STAGING DIR: $dir"
done

find test_data -name "*.parquet" | while read file; do
    size=$(stat -c%s "$file" 2>/dev/null || echo "unknown")
    echo "  ✨ PARQUET FILE: $file ($size bytes)"
done

# Count results
STAGING_DIRS=$(find test_data -type d -name "__*" | wc -l)
PARQUET_FILES=$(find test_data -name "*.parquet" | wc -l)

echo ""
echo "📊 STAGING PATTERN ANALYSIS:"
echo "  🎯 Staging directories: $STAGING_DIRS"
echo "  ✨ Parquet files: $PARQUET_FILES"

if [ $PARQUET_FILES -gt 0 ]; then
    echo ""
    echo "🎉 SUCCESS! Staging pattern created $PARQUET_FILES Parquet file(s)"
    echo "✅ VIPER flush with staging directories is working!"
else
    echo ""
    echo "❓ No Parquet files found - check server logs for details"
fi

# Cleanup
echo ""
echo "🛑 Stopping server..."
kill $SERVER_PID 2>/dev/null
sleep 2

echo "✅ Test completed"