#!/usr/bin/env python3
import requests
import json

base_url = "http://localhost:5678"

print("=== Testing Search After Insert ===")
print()

collection_name = "recovery_test_collection"

# 1. Check collection stats
print("1. Current collection stats:")
response = requests.get(f"{base_url}/api/v1/collection/{collection_name}")
if response.status_code == 200:
    col = response.json()
    print(f"   - Vector count: {col['vector_count']}")
    print(f"   - Collection ID: {col['id']}")

print()

# 2. Try batch search with correct format
print("2. Searching with batch format:")
batch_search_data = {
    "collection_id": collection_name,
    "queries": [{"vector": [0.5] * 128, "top_k": 10, "include_metadata": True}],
}
response = requests.post(f"{base_url}/api/v1/vector/search", json=batch_search_data)
print(f"   Batch search response: {response.status_code}")
if response.status_code == 200:
    results = response.json()
    print(f"   ✓ Search successful!")
    print(f"   Response: {json.dumps(results, indent=2)[:500]}...")

    # Parse results
    if "results" in results and len(results["results"]) > 0:
        batch_results = results["results"][0]
        if "results" in batch_results:
            print(f"\n   Found {len(batch_results['results'])} vectors:")
            for i, res in enumerate(batch_results["results"][:5]):
                print(
                    f"     - Vector {i}: id={res.get('id')}, distance={res.get('distance'):.4f}"
                )
                if "metadata" in res:
                    print(f"       metadata: {res['metadata']}")
else:
    print(f"   ✗ Error: {response.text[:200]}")

print()
print("=== Search Test Complete ===")
