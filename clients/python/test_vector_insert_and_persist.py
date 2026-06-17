#!/usr/bin/env python3

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python test_vector_insert_and_persist.py

from proximadb import ProximaDBClient, DistanceMetric, VectorRecord
import time
import numpy as np

# Connect to the running server


def main():
    client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

    print("=== Testing Vector Insertion and Persistence ===")
    print()

    # 1. Create a new collection for testing
    collection_name = f"persistence_test_{int(time.time())}"
    print(f"1. Creating new collection '{collection_name}':")
    try:
        collection = client.create_collection(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            enable_two_stage_search=True,
        )
        print(f"   ✓ Collection created successfully")
    except Exception as e:
        print(f"   ✗ Error: {e}")
        exit(1)

    print()

    # 2. Insert vectors
    print("2. Inserting vectors:")
    vectors = []
    for i in range(10):
        vec = VectorRecord(
            id=f"vec_{i}",
            vector=np.random.rand(128).tolist(),
            metadata={"index": i, "test": "persistence", "timestamp": time.time()},
        )
        vectors.append(vec)

    try:
        response = collection.insert_batch(vectors)
        print(f"   ✓ Inserted {len(vectors)} vectors successfully")
        print(f"   Response: {response}")
    except Exception as e:
        print(f"   ✗ Error inserting vectors: {e}")

    print()

    # 3. Search to verify vectors are there
    print("3. Searching for vectors:")
    try:
        query_vector = np.random.rand(128).tolist()
        results = collection.search(
            vector=query_vector, top_k=5, distance_metric=DistanceMetric.COSINE
        )
        print(f"   ✓ Found {len(results)} results")
        for i, result in enumerate(results[:3]):
            print(f"     - Result {i}: id={result.id}, distance={result.distance}")
    except Exception as e:
        print(f"   ✗ Error searching: {e}")

    print()

    # 4. Check collection stats
    print("4. Checking collection stats:")
    try:
        col = client.get_collection(collection_name)
        print(f"   Collection info:")
        print(f"   - Name: {col.name}")
        print(f"   - Dimension: {col.dimension}")
        print(
            f"   - Vector count: {col.vector_count if hasattr(col, 'vector_count') else 'N/A'}"
        )
    except Exception as e:
        print(f"   ✗ Error: {e}")

    print()

    # 5. Force a flush if possible
    print("5. Waiting for potential flush to disk...")
    time.sleep(2)

    print()
    print(f"=== Test Complete ===")
    print(f"Collection '{collection_name}' created with {len(vectors)} vectors")
    print("Now restart the server and check if vectors persist!")


if __name__ == "__main__":
    main()
