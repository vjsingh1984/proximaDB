#!/usr/bin/env python3
import requests
import json
import time
import random

base_url = "http://localhost:5678"

print("=== Testing Vector Persistence with REST API ===")
print()

# 1. Use the recovery_test_collection that already exists
collection_name = "recovery_test_collection"
print(f"1. Using existing collection '{collection_name}'")

# 2. Insert vectors using REST API
print("\n2. Inserting vectors via REST API:")
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

insert_data = {
    "vectors": vectors
}

response = requests.post(f"{base_url}/api/v1/vectors/{collection_name}", json=insert_data)
print(f"   Insert response: {response.status_code}")
if response.status_code == 200:
    print(f"   ✓ Vectors inserted successfully")
    result = response.json()
    print(f"   Response: {json.dumps(result, indent=2)[:200]}...")
else:
    print(f"   ✗ Error: {response.text}")

print()

# 3. Try different vector endpoints to verify insertion
print("3. Checking vector operations:")

# Try search
print("   a) Search endpoint:")
search_data = {
    "vector": [0.5] * 128,
    "top_k": 10
}
response = requests.post(f"{base_url}/api/v1/search/{collection_name}", json=search_data)
print(f"      Status: {response.status_code}")

# Try list
print("   b) List endpoint:")
response = requests.post(f"{base_url}/api/v1/vectors/{collection_name}/list", json={})
print(f"      Status: {response.status_code}")

# Try get specific vector
print("   c) Get vector endpoint:")
if vectors:
    vector_id = vectors[0]["id"]
    response = requests.get(f"{base_url}/api/v1/vectors/{collection_name}/{vector_id}")
    print(f"      Status: {response.status_code}")

print()

# 4. Check collection stats again
print("4. Checking collection stats after insert:")
response = requests.get(f"{base_url}/api/v1/collection/{collection_name}")
if response.status_code == 200:
    col = response.json()
    print(f"   ✓ Collection stats:")
    print(f"     - Vector count: {col['vector_count']}")
    print(f"     - Updated at: {col['updated_at']}")

print()

# 5. Check WAL directory
print("5. Checking WAL files:")
import os
wal_dir = "./lsm_wal"
if os.path.exists(wal_dir):
    files = os.listdir(wal_dir)
    print(f"   WAL directory contains {len(files)} files")
    for f in files[:5]:
        print(f"   - {f}")
else:
    print(f"   WAL directory not found at {wal_dir}")

print()
print("=== Test Complete ===")
print("Vectors have been inserted. Now you can:")
print("1. Stop the server (kill the process)")
print("2. Restart the server")
print("3. Run this script again to see if vectors persist")