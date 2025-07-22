#!/usr/bin/env python3
import requests
import json
import time
import random

base_url = "http://localhost:5678"

print("=== Testing Vector Persistence with Correct REST API ===")
print()

# 1. Use the recovery_test_collection that already exists
collection_name = "recovery_test_collection"
print(f"1. Using existing collection '{collection_name}'")

# 2. Insert vectors using the correct batch endpoint
print("\n2. Inserting vectors via /api/v1/vector/batch:")
vectors = []
for i in range(5):
    vectors.append({
        "id": f"persist_test_{i}_{int(time.time())}",
        "vector": [random.random() for _ in range(128)],
        "metadata": {
            "test": "persistence",
            "index": i,
            "timestamp": time.time()
        }
    })

batch_data = {
    "collection_id": collection_name,
    "vectors": vectors
}

response = requests.post(f"{base_url}/api/v1/vector/batch", json=batch_data)
print(f"   Insert response: {response.status_code}")
if response.status_code == 200:
    print(f"   ✓ Vectors inserted successfully")
    result = response.json()
    print(f"   Response: {json.dumps(result, indent=2)[:200]}...")
else:
    print(f"   ✗ Error: {response.text[:200]}")

print()

# 3. Search for vectors using the correct endpoint
print("3. Searching for vectors via /api/v1/vector/search:")
search_data = {
    "collection_id": collection_name,
    "vector": [0.5] * 128,
    "top_k": 10,
    "include_metadata": True
}
response = requests.post(f"{base_url}/api/v1/vector/search", json=search_data)
print(f"   Search response: {response.status_code}")
if response.status_code == 200:
    results = response.json()
    print(f"   ✓ Search successful!")
    if 'results' in results:
        print(f"   Found {len(results['results'])} results")
        for i, res in enumerate(results['results'][:3]):
            print(f"     - Result {i}: id={res.get('id')}, distance={res.get('distance'):.4f}")
    else:
        print(f"   Response: {json.dumps(results, indent=2)[:300]}...")
else:
    print(f"   ✗ Error: {response.text[:200]}")

print()

# 4. Get a specific vector
if vectors:
    print("4. Getting specific vector via /api/v1/vector/get:")
    vector_id = vectors[0]["id"]
    response = requests.get(f"{base_url}/api/v1/vector/get/{collection_name}/{vector_id}")
    print(f"   Get response: {response.status_code}")
    if response.status_code == 200:
        print(f"   ✓ Vector retrieved successfully")
        vec = response.json()
        print(f"   Vector: id={vec.get('id')}, metadata={vec.get('metadata')}")
    else:
        print(f"   ✗ Error: {response.text[:100]}")

print()

# 5. Check collection stats again
print("5. Checking collection stats after insert:")
response = requests.get(f"{base_url}/api/v1/collection/{collection_name}")
if response.status_code == 200:
    col = response.json()
    print(f"   ✓ Collection stats:")
    print(f"     - Vector count: {col['vector_count']}")
    print(f"     - Updated at: {col['updated_at']}")

print()

# 6. List unflushed vectors (debug endpoint)
print("6. Checking unflushed vectors (debug endpoint):")
response = requests.get(f"{base_url}/debug/vectors/{collection_name}")
print(f"   Debug response: {response.status_code}")
if response.status_code == 200:
    debug_info = response.json()
    print(f"   Debug info: {json.dumps(debug_info, indent=2)[:300]}...")

print()

# 7. Force flush to ensure persistence
print("7. Forcing flush to ensure persistence:")
response = requests.post(f"{base_url}/internal/flush/{collection_name}")
print(f"   Flush response: {response.status_code}")
if response.status_code == 200:
    print(f"   ✓ Collection flushed successfully")

print()

# 8. Check WAL files again
print("8. Checking WAL files after operations:")
import os
wal_dir = "./lsm_wal"
if os.path.exists(wal_dir):
    files = os.listdir(wal_dir)
    print(f"   WAL directory contains {len(files)} files")
    for f in files[:5]:
        size = os.path.getsize(os.path.join(wal_dir, f))
        print(f"   - {f} ({size} bytes)")
else:
    print(f"   WAL directory not found at {wal_dir}")

print()
print("=== Test Complete ===")
print(f"Inserted {len(vectors)} vectors into collection '{collection_name}'")
print("Now you can restart the server to test persistence!")