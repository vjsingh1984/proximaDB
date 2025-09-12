#!/usr/bin/env python3
"""Trace the exact issue with gRPC insert -> get flow"""

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python test_trace_issue.py

from proximadb import ProximaDBClient as ProximaDB
from proximadb.models import VectorRecord
import numpy as np

def main():
    grpc_client = ProximaDB(url="grpc://localhost:5679")
    
    try:
        # Delete and create collection
        try:
            grpc_client.delete_collection("trace_test")
        except:
            pass
        
        print("1. Creating collection...")
        collection = grpc_client.create_collection(
            name="trace_test",
            dimension=4,
            distance_metric="cosine"
        )
        print(f"   Created: name={collection.name}, id={collection.id}")
        
        # Insert with collection NAME
        print("\n2. Inserting vector with collection NAME...")
        vector = VectorRecord(
            id="v1",
            vector=[1.0, 2.0, 3.0, 4.0],
            metadata={"test": True}
        )
        
        result = grpc_client.insert_vectors(
            collection_id="trace_test",  # Using NAME
            vectors=[vector]
        )
        print(f"   Insert result: {result.success}")
        
        # Try get with collection NAME
        print("\n3. Getting vector with collection NAME...")
        try:
            vec = grpc_client.get_vector(
                collection_id="trace_test",  # Using NAME
                vector_id="v1"
            )
            print(f"   ✅ Found with NAME!")
        except Exception as e:
            print(f"   ❌ Failed with NAME: {e}")
        
        # Try get with collection ID
        print("\n4. Getting vector with collection ID...")
        try:
            vec = grpc_client.get_vector(
                collection_id=collection.id,  # Using ID
                vector_id="v1"
            )
            print(f"   ✅ Found with ID!")
        except Exception as e:
            print(f"   ❌ Failed with ID: {e}")
        
        # Check debug endpoint with NAME
        print("\n5. Debug endpoint with NAME...")
        import httpx
        with httpx.Client() as client:
            response = client.get(f"http://localhost:5678/debug/vectors/trace_test")
            if response.status_code == 200:
                data = response.json()
                print(f"   Unflushed vectors: {data.get('unflushed_vector_count', 0)}")
            else:
                print(f"   Failed: {response.status_code}")
        
        # Check debug endpoint with ID
        print("\n6. Debug endpoint with ID...")
        with httpx.Client() as client:
            response = client.get(f"http://localhost:5678/debug/vectors/{collection.id}")
            if response.status_code == 200:
                data = response.json()
                print(f"   Unflushed vectors: {data.get('unflushed_vector_count', 0)}")
            else:
                print(f"   Failed: {response.status_code}")
                
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    main()