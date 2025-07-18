#!/usr/bin/env python3
import requests
import json
import time

base_url = "http://localhost:5678"

print("=== Testing Immediate Search After Insert ===")
print()

collection_name = "recovery_test_collection"

# 1. Insert fresh vectors
print("1. Inserting fresh vectors:")
vectors = []
for i in range(3):
    vectors.append({
        "id": f"immediate_test_{i}_{int(time.time())}",
        "vector": [0.1 + i*0.1] * 128,  # Different vectors
        "metadata": {
            "test": "immediate",
            "index": i
        }
    })

batch_data = {
    "collection_id": collection_name,
    "vectors": vectors
}

response = requests.post(f"{base_url}/api/v1/vector/batch", json=batch_data)
print(f"   Insert response: {response.status_code}")
if response.status_code == 200:
    result = response.json()
    print(f"   ✓ Inserted {result['metrics']['successful_count']} vectors")

print()

# 2. Immediately search for them
print("2. Searching immediately after insert:")
search_data = {
    "collection_id": collection_name,
    "queries": [{
        "vector": [0.15] * 128  # Should be close to our vectors
    }],
    "top_k": 10
}

response = requests.post(f"{base_url}/api/v1/vector/search", json=search_data)
if response.status_code == 200:
    results = response.json()
    print(f"   ✓ Search successful!")
    if results['results']:
        print(f"   Found {len(results['results'])} batch results")
        # The results are nested - batch results contain individual query results
        for batch_idx, batch_result in enumerate(results['results']):
            print(f"   Batch {batch_idx}: {batch_result}")
    else:
        print("   ✗ No results found!")
        print(f"   Full response: {json.dumps(results, indent=2)}")

print()

# 3. Check debug endpoint
print("3. Checking debug endpoint for unflushed vectors:")
response = requests.get(f"{base_url}/debug/vectors/{collection_name}")
if response.status_code == 200:
    debug_info = response.json()
    print(f"   Unflushed vectors: {debug_info['unflushed_vector_count']}")
    if debug_info['vectors']:
        print("   Vectors in memory:")
        for vec in debug_info['vectors'][:3]:
            print(f"     - {vec['id']}")

print()
print("=== Test Complete ===")