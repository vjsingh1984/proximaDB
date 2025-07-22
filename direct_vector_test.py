#!/usr/bin/env python3
"""
Direct Vector Test - Test vector operations without relying on collection creation
This uses a hardcoded collection ID to test the debug endpoint
"""

import sys
import time
import requests
import json
import numpy as np

def main():
    """Test vector operations with a dummy collection"""
    print("🔍 Direct Vector Debug Test")
    print("=" * 50)
    
    # Use a dummy collection ID for testing
    collection_id = "test_collection_123"
    
    # First, check the debug endpoint with empty collection
    print(f"\n1. Checking debug endpoint for collection: {collection_id}")
    debug_url = f"http://localhost:5678/debug/vectors/{collection_id}"
    
    try:
        response = requests.get(debug_url)
        print(f"Debug endpoint status: {response.status_code}")
        if response.status_code == 200:
            debug_data = response.json()
            print(f"Initial state: {debug_data}")
            print(f"Unflushed vector count: {debug_data.get('unflushed_vector_count', 0)}")
        else:
            print(f"Debug endpoint failed: {response.text}")
            return 1
    except Exception as e:
        print(f"Error calling debug endpoint: {e}")
        return 1
    
    # Now try to insert a vector directly via batch API
    print(f"\n2. Attempting to insert vector into collection: {collection_id}")
    
    vector_id = "debug_vector_001"
    vector = np.random.random(128).astype(np.float32).tolist()
    
    # Create proto-aligned vector record
    vector_record = {
        "id": vector_id,
        "collection_id": collection_id,
        "vector": vector,
        "metadata": [
            {"key": "test", "value": "debug_direct"},
            {"key": "timestamp", "value": str(time.time())}
        ],
        "timestamp": int(time.time() * 1000),
        "created_at": int(time.time() * 1000),
        "updated_at": int(time.time() * 1000),
        "version": 1
    }
    
    batch_request = {
        "collection_id": collection_id,
        "vectors": [vector_record]
    }
    
    print(f"Vector ID: {vector_id}")
    print(f"Vector dimensions: {len(vector)}")
    
    try:
        batch_url = "http://localhost:5678/api/v1/vector/batch"
        response = requests.post(batch_url, json=batch_request)
        print(f"\nBatch insert status: {response.status_code}")
        print(f"Batch insert response: {response.json()}")
        
        # Wait a bit for processing
        time.sleep(0.5)
        
        # Check debug endpoint again
        print(f"\n3. Checking debug endpoint after insertion...")
        response = requests.get(debug_url)
        if response.status_code == 200:
            debug_data = response.json()
            print(f"After insertion: {debug_data}")
            
            vectors = debug_data.get('vectors', [])
            if vectors:
                print(f"\n✅ SUCCESS! Found {len(vectors)} vectors in memtable")
                for i, vec in enumerate(vectors):
                    print(f"  Vector {i+1}: ID={vec.get('id', 'N/A')}")
                    if vec.get('id') == vector_id:
                        print(f"  ✅ Our vector {vector_id} is in memtable!")
            else:
                print(f"\n❌ FAILURE! No vectors found in memtable after insertion")
                print("This confirms the vector persistence issue!")
        
    except Exception as e:
        print(f"Error during test: {e}")
        return 1
    
    # Try to retrieve the vector via get API
    print(f"\n4. Attempting to retrieve vector via get API...")
    get_url = f"http://localhost:5678/api/v1/vector/get/{collection_id}/{vector_id}"
    try:
        response = requests.get(get_url)
        print(f"Get vector status: {response.status_code}")
        if response.status_code == 200:
            print(f"✅ Vector retrieved: {response.json()}")
        else:
            print(f"❌ Vector not found: {response.text}")
    except Exception as e:
        print(f"Get vector error: {e}")
    
    return 0

if __name__ == "__main__":
    sys.exit(main())