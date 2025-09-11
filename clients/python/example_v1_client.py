#!/usr/bin/env python3
"""
ProximaDB v1 Client Example

This example demonstrates how to use the v1 ProximaDB client that aligns with 
the server's v1 proto messages and unified handlers.

Run this example when the ProximaDB server is running on localhost:5678 (REST) or localhost:5679 (gRPC)
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

from proximadb import ProximaDBClientV1, VectorRecord, DistanceMetric, StorageEngine
import numpy as np

def main():
    """Example usage of the v1 ProximaDB client"""
    
    # Create client (automatically detects REST vs gRPC based on URL)
    print("Creating ProximaDB v1 client...")
    
    # For REST API (default server port)
    client_rest = ProximaDBClientV1(url="http://localhost:5678", protocol="rest")
    print(f"REST Client created: {client_rest.protocol}")
    
    # For gRPC API  
    client_grpc = ProximaDBClientV1(url="http://localhost:5679", protocol="grpc")
    print(f"gRPC Client created: {client_grpc.protocol}")
    
    # Use REST client for the example
    client = client_rest
    
    try:
        # Test server health
        print("\nTesting server health...")
        health = client.health_check()
        print(f"✅ Server is healthy: {health}")
        
        # Create a collection
        print("\nCreating collection...")
        collection_name = "example_v1_collection"
        
        collection = client.create_collection(
            name=collection_name,
            dimension=4,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.SST
        )
        print(f"✅ Collection created: {collection.name} (id: {collection.id})")
        
        # Insert some vectors
        print("\nInserting vectors...")
        vectors = [
            VectorRecord(
                id="vec_1",
                vector=[0.1, 0.2, 0.3, 0.4],
                metadata={"category": "A", "value": 100}
            ),
            VectorRecord(
                id="vec_2", 
                vector=[0.2, 0.3, 0.4, 0.5],
                metadata={"category": "B", "value": 200}
            ),
            VectorRecord(
                id="vec_3",
                vector=[0.3, 0.4, 0.5, 0.6],
                metadata={"category": "A", "value": 300}
            )
        ]
        
        result = client.insert_vectors(collection_name, vectors)
        print(f"✅ Vectors inserted: {result}")
        
        # Search for similar vectors
        print("\nSearching for similar vectors...")
        query_vector = [0.15, 0.25, 0.35, 0.45]
        
        search_result = client.search_vectors(
            collection_id=collection_name,
            vector=query_vector,
            top_k=2
        )
        print(f"✅ Search completed, found {len(search_result.results)} results:")
        for i, result in enumerate(search_result.results):
            print(f"   {i+1}. ID: {result['id']}, Score: {result['score']:.4f}")
        
        # Get a specific vector
        print("\nRetrieving specific vector...")
        vector = client.get_vector(collection_name, "vec_2")
        if vector:
            print(f"✅ Retrieved vector: {vector.id} with {len(vector.vector)} dimensions")
        else:
            print("❌ Vector not found")
        
        # Execute SQL query (if supported)
        print("\nExecuting SQL query...")
        try:
            sql_result = client.execute_sql(
                f"SELECT id, metadata FROM {collection_name} WHERE metadata.category = 'A'"
            )
            print(f"✅ SQL query executed, returned {len(sql_result.get('rows', []))} rows")
        except Exception as e:
            print(f"⚠️  SQL query failed (may not be implemented): {e}")
        
        # List collections
        print("\nListing collections...")
        collections = client.list_collections()
        print(f"✅ Found {len(collections)} collections:")
        for col in collections:
            print(f"   - {col.name} ({col.dimension}D, {col.distance_metric.value}, {col.storage_engine.value})")
    
    except Exception as e:
        print(f"❌ Example failed: {e}")
        print(f"Make sure ProximaDB server is running on localhost:5678")
    
    finally:
        # Clean up
        client.close()
        client_grpc.close()
        print("\n✅ Client connections closed")

if __name__ == "__main__":
    main()