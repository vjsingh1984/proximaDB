#!/usr/bin/env python3
"""Debug script to trace gRPC insert flow"""

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python test_grpc_insert_debug.py

import asyncio
import json
import numpy as np
from proximadb import ProximaDBClient as ProximaDB

def main():
    # Initialize clients
    grpc_client = ProximaDB(url="grpc://localhost:5679")
    rest_client = ProximaDB(url="http://localhost:5678")
    
    try:
        # Delete collection if it exists
        try:
            grpc_client.delete_collection("debug_collection")
            print("Deleted existing collection")
        except:
            pass
        
        # Create collection via gRPC
        print("Creating collection via gRPC...")
        collection = grpc_client.create_collection(
            name="debug_collection",
            dimension=128,
            distance_metric="cosine",
            storage_engine="lsm"
        )
        print(f"✅ Collection created: {collection.name}")
        print(f"   ID: {collection.id}")
        
        # Insert vectors via gRPC with explicit IDs
        print("\nInserting 3 vectors via gRPC...")
        vectors = []
        for i in range(3):
            vectors.append({
                'id': f'vec_{i}',
                'vector': np.random.rand(128).tolist(),
                'metadata': {'index': i, 'source': 'grpc'}
            })
        
        print(f"Vector IDs being sent: {[v['id'] for v in vectors]}")
        
        insert_result = grpc_client.insert_vectors(
            collection_id="debug_collection",
            vectors=vectors
        )
        print(f"✅ gRPC Insert result: Success={insert_result.success}, Vector IDs={insert_result.vector_ids}")
        
        # Try to get each vector immediately via gRPC
        print("\nGetting vectors via gRPC immediately after insert...")
        for i in range(3):
            vector_id = f'vec_{i}'
            try:
                result = grpc_client.get_vector(
                    collection_id="debug_collection",
                    vector_id=vector_id
                )
                print(f"✅ gRPC get vec_{i}: Found")
                if result.get('result_payload', {}).get('single_vector'):
                    vec = result['result_payload']['single_vector']
                    print(f"   ID: {vec.get('id')}")
                    print(f"   Metadata: {vec.get('metadata')}")
            except Exception as e:
                print(f"❌ gRPC get vec_{i}: {str(e)}")
        
        # Try to get each vector via REST
        print("\nGetting vectors via REST...")
        for i in range(3):
            vector_id = f'vec_{i}'
            try:
                result = rest_client.get_vector(
                    collection_id="debug_collection",
                    vector_id=vector_id
                )
                print(f"✅ REST get vec_{i}: Found")
                if result.get('result_payload', {}).get('single_vector'):
                    vec = result['result_payload']['single_vector']
                    print(f"   ID: {vec.get('id')}")
                    print(f"   Metadata: {vec.get('metadata')}")
            except Exception as e:
                print(f"❌ REST get vec_{i}: {str(e)}")
        
        # Check unflushed vectors via REST debug endpoint
        print("\nChecking unflushed vectors via debug endpoint...")
        import httpx
        import httpx
        with httpx.Client() as client:
            response = client.get("http://localhost:5678/debug/vectors/debug_collection")
            if response.status_code == 200:
                debug_data = response.json()
                print(f"Debug info: {json.dumps(debug_data, indent=2)}")
            else:
                print(f"Debug endpoint failed: {response.status_code}")
        
        # Force flush and check again
        print("\nForcing flush...")
        with httpx.Client() as client:
            response = client.post("http://localhost:5678/internal/flush/debug_collection")
            if response.status_code == 200:
                print(f"Flush result: {response.json()}")
        
        # Try to get vectors again after flush
        print("\nGetting vectors via REST after flush...")
        for i in range(3):
            vector_id = f'vec_{i}'
            try:
                result = rest_client.get_vector(
                    collection_id="debug_collection",
                    vector_id=vector_id
                )
                print(f"✅ REST get vec_{i} after flush: Found")
            except Exception as e:
                print(f"❌ REST get vec_{i} after flush: {str(e)}")
                
    except Exception as e:
        print(f"❌ Error: {str(e)}")
        import traceback
        traceback.print_exc()
    finally:
        if hasattr(grpc_client, 'close'):
            grpc_client.close()
        if hasattr(rest_client, 'close'):
            rest_client.close()

if __name__ == "__main__":
    main()