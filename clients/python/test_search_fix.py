#!/usr/bin/env python3
"""Quick test to verify search endpoint fix"""

import sys
sys.path.insert(0, 'src')

from proximadb import ProximaDBClient
import numpy as np

# Create client
client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

# Create collection
print("Creating collection...")
try:
    collection = client.create_collection(
        name="test_search_fix",
        dimension=128
    )
    print(f"✓ Collection created: {collection.name}")
except Exception as e:
    print(f"✗ Failed to create collection: {e}")
    sys.exit(1)

# Insert vectors
print("\nInserting vectors...")
try:
    vectors = [
        {
            "id": f"vec_{i}",
            "vector": np.random.rand(128).tolist(),
            "metadata": {"index": i}
        }
        for i in range(10)
    ]
    client.insert(collection_id="test_search_fix", vectors=vectors)
    print(f"✓ Inserted {len(vectors)} vectors")
except Exception as e:
    print(f"✗ Failed to insert vectors: {e}")
    sys.exit(1)

# Search (this should work now with empty filters)
print("\nSearching...")
try:
    query_vector = np.random.rand(128).tolist()
    results = client.search(
        collection_id="test_search_fix",
        vector=query_vector,
        top_k=5
    )
    print(f"✓ Search successful! Found {len(results)} results")
    if results:
        print(f"  Top result: {results[0].id} (score: {results[0].score:.4f})")
except Exception as e:
    print(f"✗ Search failed: {e}")
    import traceback
    traceback.print_exc()
    sys.exit(1)

print("\n✅ All tests passed! Search fix is working.")
