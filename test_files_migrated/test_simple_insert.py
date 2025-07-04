#!/usr/bin/env python3

import requests
import json
import numpy as np

# Server configuration
SERVER_URL = "http://localhost:5678"
COLLECTION_NAME = "simple_test"

def create_collection():
    """Create a test collection"""
    response = requests.post(f"{SERVER_URL}/collections", json={
        "name": COLLECTION_NAME,
        "dimension": 768,
        "distance_metric": "cosine"
    })
    print(f"Collection creation: {response.status_code} - {response.text}")
    return response.status_code == 200

def insert_single_vector():
    """Insert a single vector to test the format"""
    vector = np.random.random(768).astype(np.float32).tolist()
    
    # Try different formats to find the correct one
    formats_to_try = [
        # Format 1: Direct vector object
        {
            "vector": vector,
            "metadata": {"test": "value"}
        },
        # Format 2: With ID field  
        {
            "id": "test_vec_001",
            "vector": vector,
            "metadata": {"test": "value"}
        },
        # Format 3: Nested in data field
        {
            "data": {
                "id": "test_vec_001", 
                "vector": vector,
                "metadata": {"test": "value"}
            }
        },
        # Format 4: Array format
        [{
            "id": "test_vec_001",
            "vector": vector,
            "metadata": {"test": "value"}
        }]
    ]
    
    for i, format_data in enumerate(formats_to_try):
        print(f"\n🧪 Trying format {i+1}...")
        response = requests.post(f"{SERVER_URL}/collections/{COLLECTION_NAME}/vectors", json=format_data)
        print(f"Response: {response.status_code} - {response.text[:200]}")
        
        if response.status_code == 200:
            print(f"✅ Format {i+1} worked!")
            return True
    
    return False

def cleanup():
    """Clean up collection"""
    response = requests.delete(f"{SERVER_URL}/collections/{COLLECTION_NAME}")
    print(f"Cleanup: {response.status_code}")

def main():
    if not create_collection():
        return
    
    try:
        insert_single_vector()
    finally:
        cleanup()

if __name__ == "__main__":
    main()