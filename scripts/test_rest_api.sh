#!/bin/bash
# Test script for ProximaDB REST API

API_BASE="http://localhost:5678"
COLLECTION_NAME="test_collection"

echo "🧪 ProximaDB REST API Test Suite"
echo "================================="

# Test 1: Health Check
echo "1. Testing health check..."
curl -s -X GET "$API_BASE/health" | jq '.' || echo "Health check failed"
echo ""

# Test 2: Create Collection
echo "2. Creating collection '$COLLECTION_NAME'..."
curl -s -X POST "$API_BASE/collections" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "'$COLLECTION_NAME'",
    "dimension": 384,
    "distance_metric": "cosine",
    "indexing_algorithm": "hnsw"
  }' | jq '.' || echo "Collection creation failed"
echo ""

# Test 3: List Collections
echo "3. Listing collections..."
curl -s -X GET "$API_BASE/collections" | jq '.' || echo "List collections failed"
echo ""

# Test 4: Get Collection
echo "4. Getting collection details..."
curl -s -X GET "$API_BASE/collections/$COLLECTION_NAME" | jq '.' || echo "Get collection failed"
echo ""

# Test 5: Insert Vector
echo "5. Inserting vector..."
curl -s -X POST "$API_BASE/collections/$COLLECTION_NAME/vectors" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "test_vector_1",
    "vector": [0.1, 0.2, 0.3, 0.4, 0.5],
    "metadata": {
      "label": "test",
      "category": "sample"
    }
  }' | jq '.' || echo "Vector insertion failed"
echo ""

# Test 6: Search Vectors
echo "6. Searching vectors..."
curl -s -X POST "$API_BASE/collections/$COLLECTION_NAME/search" \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.1, 0.2, 0.3, 0.4, 0.5],
    "k": 5,
    "include_vectors": true,
    "include_metadata": true
  }' | jq '.' || echo "Vector search failed"
echo ""

# Test 7: Batch Insert
echo "7. Batch inserting vectors..."
curl -s -X POST "$API_BASE/collections/$COLLECTION_NAME/vectors/batch" \
  -H "Content-Type: application/json" \
  -d '[
    {
      "id": "batch_vector_1",
      "vector": [0.2, 0.3, 0.4, 0.5, 0.6],
      "metadata": {"type": "batch1"}
    },
    {
      "id": "batch_vector_2", 
      "vector": [0.3, 0.4, 0.5, 0.6, 0.7],
      "metadata": {"type": "batch2"}
    }
  ]' | jq '.' || echo "Batch insert failed"
echo ""

# Test 8: Update Vector
echo "8. Updating vector..."
curl -s -X PUT "$API_BASE/collections/$COLLECTION_NAME/vectors/test_vector_1" \
  -H "Content-Type: application/json" \
  -d '{
    "vector": [0.9, 0.8, 0.7, 0.6, 0.5],
    "metadata": {
      "label": "updated",
      "category": "modified"
    }
  }' | jq '.' || echo "Vector update failed"
echo ""

# Test 9: Get Vector
echo "9. Getting vector..."
curl -s -X GET "$API_BASE/collections/$COLLECTION_NAME/vectors/test_vector_1" | jq '.' || echo "Get vector failed"
echo ""

# Test 10: Delete Vector
echo "10. Deleting vector..."
curl -s -X DELETE "$API_BASE/collections/$COLLECTION_NAME/vectors/test_vector_1" | jq '.' || echo "Vector deletion failed"
echo ""

# Test 11: Delete Collection
echo "11. Deleting collection..."
curl -s -X DELETE "$API_BASE/collections/$COLLECTION_NAME" | jq '.' || echo "Collection deletion failed"
echo ""

echo "✅ REST API test suite completed!"