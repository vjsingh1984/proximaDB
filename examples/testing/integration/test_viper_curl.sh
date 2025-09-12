#!/bin/bash
# Simple test script for VIPER metadata filtering using curl

echo "🚀 Starting ProximaDB server with test config..."
cargo run --release --bin proximadb-server -- --config test-viper-config.toml > server.log 2>&1 &
SERVER_PID=$!

# Wait for server to start
echo "⏳ Waiting for server to start..."
sleep 10

# Check if server is running
if ! ps -p $SERVER_PID > /dev/null; then
    echo "❌ Server failed to start"
    cat server.log
    exit 1
fi

echo "✅ Server started with PID: $SERVER_PID"

# Test server health
echo -e "\n🏥 Checking server health..."
curl -s http://localhost:5678/health | jq .

# Create collection with filterable metadata
echo -e "\n📦 Creating collection with filterable metadata..."
curl -X POST http://localhost:5678/v1/collections \
  -H "Content-Type: application/json" \
  -d '{
    "id": "test_collection",
    "config": {
      "dimension": 128,
      "distance_metric": "COSINE",
      "filterable_columns": [
        {
          "name": "category",
          "data_type": "FILTERABLE_STRING",
          "indexed": false,
          "estimated_cardinality": 10
        }
      ]
    }
  }' | jq .

# Insert vectors with metadata
echo -e "\n📝 Inserting vectors with metadata..."
for i in 1 2 3 4 5; do
  CATEGORY="A"
  if [ $i -gt 3 ]; then
    CATEGORY="B"
  fi
  
  curl -X POST http://localhost:5678/v1/collections/test_collection/vectors \
    -H "Content-Type: application/json" \
    -d "{
      \"vectors\": [{
        \"id\": \"vec$i\",
        \"vector\": $(python3 -c "print([0.1] * 128)"),
        \"metadata\": {
          \"category\": \"$CATEGORY\",
          \"item_id\": $i
        }
      }]
    }"
  echo " ✅ Inserted vec$i with category=$CATEGORY"
done

# Wait for flush
echo -e "\n⏳ Waiting for flush to VIPER storage..."
sleep 5

# Insert more vectors to trigger additional flushes
echo -e "\n📝 Inserting more vectors to trigger multiple flushes..."
for i in 6 7 8 9 10; do
  CATEGORY="C"
  curl -X POST http://localhost:5678/v1/collections/test_collection/vectors \
    -H "Content-Type: application/json" \
    -d "{
      \"vectors\": [{
        \"id\": \"vec$i\",
        \"vector\": $(python3 -c "print([0.2] * 128)"),
        \"metadata\": {
          \"category\": \"$CATEGORY\",
          \"item_id\": $i
        }
      }]
    }"
  echo " ✅ Inserted vec$i with category=$CATEGORY"
  sleep 1
done

# Wait for all flushes
echo -e "\n⏳ Waiting for all flushes..."
sleep 5

# Test basic search
echo -e "\n🔍 Testing basic search (no filters)..."
curl -X POST http://localhost:5678/v1/collections/test_collection/search \
  -H "Content-Type: application/json" \
  -d "{
    \"vector\": $(python3 -c "print([0.1] * 128)"),
    \"top_k\": 5
  }" | jq .

# Test search with metadata filter
echo -e "\n🔍 Testing search with metadata filter (category=A)..."
curl -X POST http://localhost:5678/v1/collections/test_collection/search \
  -H "Content-Type: application/json" \
  -d "{
    \"vector\": $(python3 -c "print([0.1] * 128)"),
    \"top_k\": 5,
    \"filters\": {
      \"category\": \"A\"
    }
  }" | jq .

# Test search with different filter
echo -e "\n🔍 Testing search with metadata filter (category=B)..."
curl -X POST http://localhost:5678/v1/collections/test_collection/search \
  -H "Content-Type: application/json" \
  -d "{
    \"vector\": $(python3 -c "print([0.1] * 128)"),
    \"top_k\": 5,
    \"filters\": {
      \"category\": \"B\"
    }
  }" | jq .

# Clean up
echo -e "\n🧹 Cleaning up..."
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null

echo -e "\n✅ Test completed!"