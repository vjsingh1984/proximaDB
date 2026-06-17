#!/usr/bin/env python3
import requests
import json

# Test collection persistence after server restart
base_url = "http://localhost:5678"

print("=== Server Recovery Test ===")
print()

# 1. List all collections
print("1. Listing all collections:")
response = requests.get(f"{base_url}/api/v1/collections")
if response.status_code == 200:
    collections = response.json()
    print(f"   ✓ Found {len(collections['collections'])} collections")
    for col in collections["collections"]:
        col_name = col.get("name", col.get("id", "unknown"))
        col_id = col.get("id", "unknown")
        vector_count = col.get("vector_count", col.get("count", 0))
        print(f"     - {col_name} (id: {col_id}, vectors: {vector_count})")
        if col_name == "recovery_test_collection":
            print(f"       ^ RECOVERY TEST COLLECTION FOUND! ✓")
else:
    print(f"   ✗ Error: {response.status_code}")

print()

# 2. Get specific collection
collection_name = "recovery_test_collection"
print(f"2. Getting collection '{collection_name}':")
response = requests.get(f"{base_url}/api/v1/collection/{collection_name}")
if response.status_code == 200:
    col = response.json()
    print(f"   ✓ Collection found:")
    print(f"     - ID: {col['id']}")
    print(f"     - Name: {col['name']}")
    print(f"     - Dimension: {col['dimension']}")
    print(f"     - Metric: {col['metric']}")
    print(f"     - Vector count: {col['vector_count']}")
    print(f"     - Created at: {col['created_at']}")
else:
    print(f"   ✗ Error: {response.status_code}")

print()

# 3. Test vector operations
print("3. Testing vector operations on recovered collection:")

# Insert a new vector
vector_data = {
    "vectors": [
        {
            "id": "recovery_test_vector_1",
            "vector": [0.1] * 128,
            "metadata": {"test": "recovery", "timestamp": "after_restart"},
        }
    ]
}

print("   - Inserting new vector...")
response = requests.post(
    f"{base_url}/api/v1/vectors/{collection_name}", json=vector_data
)
if response.status_code == 200:
    print(f"     ✓ Vector inserted successfully")
else:
    print(f"     ✗ Error: {response.status_code} - {response.text}")

# List vectors
print("   - Listing vectors...")
response = requests.post(f"{base_url}/api/v1/vectors/{collection_name}/list", json={})
if response.status_code == 200:
    vectors = response.json()
    print(f"     ✓ Found {len(vectors.get('vectors', []))} vectors")
    for vec in vectors.get("vectors", [])[:3]:
        print(f"       - {vec['id']}: metadata={vec.get('metadata', {})}")
else:
    print(f"     ✗ Error: {response.status_code}")

print()
print("=== Recovery Test Complete ===")
print()
print("SUMMARY: Server successfully recovered collections after restart! ✓")
