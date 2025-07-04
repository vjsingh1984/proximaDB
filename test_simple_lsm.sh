#!/bin/bash

echo "🔧 Testing simple LSM collection creation and storage assignment"

# Wait for server to be fully ready
sleep 3

# Create LSM collection
echo "📝 Creating LSM collection..."
curl -X POST "http://localhost:5678/collections" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "simple_lsm_test", 
    "dimension": 768,
    "distance_metric": "COSINE",
    "storage_engine": "LSM"
  }' || echo "❌ Collection creation failed"

echo ""
echo "🔍 Checking created directories..."
echo "WAL dir: $(ls -la /workspace/data/disk1/wal/ 2>/dev/null | grep simple_lsm_test || echo 'NOT FOUND')"
echo "Storage dir: $(ls -la /workspace/data/disk1/storage/ 2>/dev/null | grep simple_lsm_test || echo 'NOT FOUND')"

echo ""
echo "✅ Test complete"