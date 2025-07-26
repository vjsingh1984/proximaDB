#!/usr/bin/env python3

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python test_recovery.py

from proximadb import ProximaDBClient, DistanceMetric, VectorRecord
import time

# Connect to the running server
client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

print("=== Testing Server Persistence ===")
print()

# List existing collections
print("1. Listing existing collections:")
try:
    collections = client.list_collections()
    print(f"   Found {len(collections)} collections")
    for col in collections:
        print(f"   - {col.name} (dimension: {col.dimension})")
except Exception as e:
    print(f"   Error listing collections: {e}")

print()

# Check specific collection we created earlier
collection_name = "recovery_test_collection"
print(f"2. Checking for collection '{collection_name}':")
try:
    collection = client.get_collection(collection_name)
    print(f"   ✓ Collection found: {collection.name}")
    print(f"     - Dimension: {collection.dimension}")
    print(f"     - Distance metric: {collection.distance_metric}")
except Exception as e:
    print(f"   ✗ Collection not found: {e}")

print()

# List vectors in the collection
print(f"3. Listing vectors in '{collection_name}':")
try:
    # First ensure we have the collection object
    if 'collection' not in locals():
        collection = client.get_collection(collection_name)
    
    # Use list_all_vectors to get all vectors
    vectors = collection.list_all_vectors()
    print(f"   Found {len(vectors)} vectors")
    for i, vec in enumerate(vectors[:5]):  # Show first 5
        print(f"   - Vector {i}: id={vec.id}, metadata={vec.metadata}")
except Exception as e:
    print(f"   Error listing vectors: {e}")

print()

# Try to search for vectors
print(f"4. Searching for similar vectors:")
try:
    if 'collection' in locals():
        query_vector = [0.1] * 128
        results = collection.search(
            vector=query_vector,
            top_k=3,
            distance_metric=DistanceMetric.COSINE
        )
        print(f"   Found {len(results)} similar vectors")
        for i, result in enumerate(results):
            print(f"   - Result {i}: id={result.id}, distance={result.distance}")
except Exception as e:
    print(f"   Error searching: {e}")

print()
print("=== Persistence Test Complete ===")