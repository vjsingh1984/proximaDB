#!/usr/bin/env python3
"""Debug metadata quote issue"""

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python test_metadata_debug.py

from proximadb import ProximaDBClient as ProximaDB
from proximadb.models import VectorRecord

# Test with gRPC
grpc_client = ProximaDB(url="grpc://localhost:5679")

# Clean start
try:
    grpc_client.delete_collection("metadata_debug")
except:
    pass

# Create collection
collection = grpc_client.create_collection(
    name="metadata_debug", dimension=4, distance_metric="cosine"
)

# Insert vector with metadata
vector = VectorRecord(
    id="test_vec",
    vector=[1.0, 2.0, 3.0, 4.0],
    metadata={"source": "grpc", "count": 42, "active": True},
)

result = grpc_client.insert_vectors(collection_id="metadata_debug", vectors=[vector])
print(f"Insert result: {result.success}")

# Get the vector back
retrieved = grpc_client.get_vector(collection_id="metadata_debug", vector_id="test_vec")

print(f"\nRetrieved metadata:")
for key, value in retrieved.get("metadata", {}).items():
    print(f"  {key}: {repr(value)} (type: {type(value).__name__})")

# Now check via REST
rest_client = ProximaDB(url="http://localhost:5678")
retrieved_rest = rest_client.get_vector(
    collection_id="metadata_debug", vector_id="test_vec"
)

print(f"\nRetrieved via REST:")
for key, value in retrieved_rest.get("metadata", {}).items():
    print(f"  {key}: {repr(value)} (type: {type(value).__name__})")
