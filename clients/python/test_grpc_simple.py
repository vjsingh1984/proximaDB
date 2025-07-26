#!/usr/bin/env python3
"""Simple test to debug gRPC insert issues"""

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python test_grpc_simple.py

from proximadb import ProximaDBClient as ProximaDB
from proximadb.models import VectorRecord
import numpy as np

def main():
    # Initialize clients
    grpc_client = ProximaDB(url="grpc://localhost:5679")
    rest_client = ProximaDB(url="http://localhost:5678")
    
    try:
        # Delete collection if it exists
        try:
            grpc_client.delete_collection("test_grpc")
            print("Deleted existing collection")
        except:
            pass
        
        # Create collection via gRPC
        print("\n1. Creating collection via gRPC...")
        collection = grpc_client.create_collection(
            name="test_grpc",
            dimension=128,
            distance_metric="cosine",
            storage_engine="lsm"
        )
        print(f"✅ Collection created: {collection.name} (ID: {collection.id})")
        
        # Insert ONE vector via gRPC
        print("\n2. Inserting ONE vector via gRPC...")
        
        # Create vector using VectorRecord model
        vector = VectorRecord(
            id="test_vec_1",
            vector=np.random.rand(128).tolist(),
            metadata={"source": "grpc", "index": 1}
        )
        
        result = grpc_client.insert_vectors(
            collection_id="test_grpc",
            vectors=[vector]
        )
        print(f"✅ Insert result: {result}")
        print(f"   Success: {result.success}")
        print(f"   Vector IDs: {result.vector_ids}")
        
        # Try to get the vector via gRPC
        print("\n3. Getting vector via gRPC...")
        try:
            vec_result = grpc_client.get_vector(
                collection_id="test_grpc",
                vector_id="test_vec_1"
            )
            print(f"✅ gRPC get: Found vector")
            print(f"   Response: {vec_result}")
        except Exception as e:
            print(f"❌ gRPC get failed: {e}")
        
        # Try to get the vector via REST
        print("\n4. Getting vector via REST...")
        try:
            vec_result = rest_client.get_vector(
                collection_id="test_grpc",
                vector_id="test_vec_1"
            )
            print(f"✅ REST get: Found vector")
            print(f"   Response: {vec_result}")
        except Exception as e:
            print(f"❌ REST get failed: {e}")
        
        # Check debug endpoint
        print("\n5. Checking debug endpoint...")
        import httpx
        with httpx.Client() as client:
            response = client.get("http://localhost:5678/debug/vectors/test_grpc")
            if response.status_code == 200:
                debug_data = response.json()
                print(f"✅ Debug info:")
                print(f"   Unflushed vectors: {debug_data.get('unflushed_vector_count', 0)}")
                print(f"   Vectors: {debug_data.get('vectors', [])[:1]}...")  # Show first vector
            else:
                print(f"❌ Debug endpoint failed: {response.status_code}")
                
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    main()