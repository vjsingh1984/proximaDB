#!/usr/bin/env python3
import requests
import json

base_url = "http://localhost:5678"

print("=== Testing Vector Search with Correct Format ===")
print()

collection_name = "recovery_test_collection"

# Search with correct format - top_k at top level
print("1. Searching with correct format:")
search_data = {
    "collection_id": collection_name,
    "queries": [{
        "vector": [0.5] * 128,
        "include_metadata": True
    }],
    "top_k": 10,  # top_k at request level, not query level
    "include_fields": {
        "include_metadata": True,
        "include_vector": False
    }
}

response = requests.post(f"{base_url}/api/v1/vector/search", json=search_data)
print(f"   Search response: {response.status_code}")

if response.status_code == 200:
    results = response.json()
    print(f"   ✓ Search successful!")
    
    # Pretty print the response structure
    print(f"\n   Response structure:")
    print(f"   - success: {results.get('success')}")
    print(f"   - operation: {results.get('operation')}")
    
    if 'results' in results:
        print(f"   - results: {len(results['results'])} batch(es)")
        
        # Each batch result
        for batch_idx, batch_result in enumerate(results['results']):
            if 'results' in batch_result:
                print(f"\n   Batch {batch_idx}: Found {len(batch_result['results'])} vectors")
                for i, res in enumerate(batch_result['results'][:5]):
                    print(f"     Vector {i}:")
                    print(f"       - id: {res.get('id')}")
                    print(f"       - distance: {res.get('distance', 0):.6f}")
                    if 'metadata' in res and res['metadata']:
                        print(f"       - metadata: {res['metadata']}")
            else:
                print(f"\n   Batch {batch_idx}: No results field")
                print(f"   Batch data: {json.dumps(batch_result, indent=2)[:200]}...")
    
    if 'metrics' in results:
        print(f"\n   Metrics:")
        metrics = results['metrics']
        print(f"   - processing_time_us: {metrics.get('processing_time_us', 0)}")
        print(f"   - total_candidates: {metrics.get('total_candidates', 0)}")
else:
    print(f"   ✗ Error {response.status_code}: {response.text[:400]}")

print()
print("=== Search Test Complete ===")
print("\nIf no vectors were found, they may not have been persisted to disk yet.")
print("The vectors were inserted but might still be in memory only.")