#!/usr/bin/env python3
import requests
import json

# Test vector persistence after server restart
base_url = "http://localhost:5678"

print("=== Vector Persistence Test ===")
print()

# 1. Check collection status
collection_name = "recovery_test_collection"
print(f"1. Checking collection '{collection_name}':")
response = requests.get(f"{base_url}/api/v1/collection/{collection_name}")
if response.status_code == 200:
    col = response.json()
    print(f"   ✓ Collection found: {col['name']}")
    print(f"     - Vector count reported: {col['vector_count']}")
else:
    print(f"   ✗ Error getting collection: {response.status_code}")

print()

# 2. Try to search for vectors
print("2. Searching for vectors in recovered collection:")
search_data = {
    "vector": [0.1] * 128,  # Query vector
    "top_k": 10,
    "include_metadata": True,
    "filters": {}
}

response = requests.post(f"{base_url}/api/v1/search/{collection_name}", json=search_data)
print(f"   Search response status: {response.status_code}")
if response.status_code == 200:
    results = response.json()
    print(f"   ✓ Search successful!")
    if 'results' in results:
        print(f"   Found {len(results['results'])} vectors")
        for i, result in enumerate(results['results'][:5]):
            print(f"     - Result {i+1}: id={result.get('id')}, distance={result.get('distance')}, metadata={result.get('metadata')}")
    else:
        print(f"   Response format: {list(results.keys())}")
        print(f"   Raw response: {json.dumps(results, indent=2)}")
else:
    print(f"   ✗ Search failed: {response.text}")

print()

# 3. Try different search endpoints
print("3. Trying alternative search methods:")

# Try batch search
print("   a) Batch search:")
batch_search_data = {
    "searches": [{
        "vector": [0.1] * 128,
        "top_k": 5
    }]
}
response = requests.post(f"{base_url}/api/v1/batch_search/{collection_name}", json=batch_search_data)
print(f"      Status: {response.status_code}")
if response.status_code == 200:
    print(f"      ✓ Batch search successful")
else:
    print(f"      ✗ Error: {response.text[:100]}...")

# Try vector list
print("   b) List vectors:")
list_data = {
    "limit": 10,
    "offset": 0
}
response = requests.post(f"{base_url}/api/v1/vectors/{collection_name}/list", json=list_data)
print(f"      Status: {response.status_code}")
if response.status_code == 200:
    vectors = response.json()
    print(f"      Response: {json.dumps(vectors, indent=2)[:200]}...")

print()

# 4. Check other collections for vectors
print("4. Checking other collections for vectors:")
response = requests.get(f"{base_url}/api/v1/collections")
if response.status_code == 200:
    collections = response.json()
    for col in collections['collections']:
        col_name = col.get('name', col.get('id', 'unknown'))
        vector_count = col.get('vector_count', col.get('count', 0))
        if vector_count > 0:
            print(f"   ✓ Collection '{col_name}' has {vector_count} vectors")

            # Try to search this collection
            search_response = requests.post(f"{base_url}/api/v1/search/{col_name}", json=search_data)
            if search_response.status_code == 200:
                results = search_response.json()
                print(f"     - Search returned results: {len(results.get('results', []))} items")

print()

# 5. Check WAL stats
print("5. Checking server logs for WAL recovery info:")
# Read the server log for WAL recovery information
try:
    with open('server_recovery.log', 'r') as f:
        lines = f.readlines()
        for line in lines:
            if 'WAL' in line and ('recovery' in line or 'found' in line or 'vectors' in line):
                print(f"   - {line.strip()}")
except:
    print("   - Could not read server logs")

print()
print("=== Vector Persistence Test Complete ===")