#!/usr/bin/env python3

import requests
import json
import time
import os

# Server configuration
BASE_URL = "http://localhost:5678"
COLLECTION_NAME = "debug_avro"

def make_request(method, endpoint, data=None):
    """Make HTTP request with error handling"""
    url = f"{BASE_URL}{endpoint}"
    try:
        if method == "GET":
            response = requests.get(url, timeout=10)
        elif method == "POST":
            response = requests.post(url, json=data, timeout=10)
        elif method == "DELETE":
            response = requests.delete(url, timeout=10)
        else:
            raise ValueError(f"Unsupported method: {method}")
        
        return response
    except requests.exceptions.RequestException as e:
        print(f"❌ Request failed: {e}")
        return None

def main():
    print("🔬 AVRO SERIALIZATION DEBUG TEST")
    print("=" * 60)
    
    # Clean up any existing collection
    print("🧹 Cleaning up existing collection...")
    response = make_request("DELETE", f"/collections/{COLLECTION_NAME}")
    
    # Create collection
    collection_config = {
        "name": COLLECTION_NAME,
        "dimension": 384,
        "distance_metric": "cosine",
        "storage_engine": "viper"
    }
    
    print(f"\n📁 Creating collection: {COLLECTION_NAME}")
    response = make_request("POST", "/collections", collection_config)
    if not response or response.status_code not in [200, 201]:
        print(f"❌ Failed to create collection: {response.status_code if response else 'No response'}")
        return
    print(f"✅ Created collection: {COLLECTION_NAME}")
    
    # Insert a single vector to trigger the Avro serialization issue
    print(f"\n📤 Inserting single test vector...")
    vector_data = {
        "id": "debug_vector_001",
        "vector": [0.1] * 384,  # Simple vector
        "metadata": {"test": "minimal"}
    }
    
    response = make_request("POST", f"/collections/{COLLECTION_NAME}/vectors", vector_data)
    if not response or response.status_code not in [200, 201]:
        print(f"❌ Failed to insert vector: {response.status_code if response else 'No response'}")
        if response:
            print(f"Response text: {response.text}")
        return
    
    print(f"✅ Vector inserted successfully")
    print(f"Response status: {response.status_code}")
    print(f"Response headers: {dict(response.headers)}")
    
    # Give time for WAL operations
    print(f"\n⏳ Waiting for WAL operations...")
    time.sleep(2)
    
    # Check if any WAL files were created
    print(f"\n🔍 Checking for WAL files...")
    wal_directories = [
        "/workspace/data/disk1/wal",
        "/workspace/data/disk2/wal", 
        "/workspace/data/disk3/wal"
    ]
    
    for wal_dir in wal_directories:
        collection_wal_dir = f"{wal_dir}/{COLLECTION_NAME}"
        if os.path.exists(collection_wal_dir):
            print(f"📂 Found directory: {collection_wal_dir}")
            files = os.listdir(collection_wal_dir)
            print(f"   Files: {files}")
        else:
            print(f"📂 Directory doesn't exist: {collection_wal_dir}")
    
    # Clean up
    print(f"\n🧹 Cleaning up...")
    response = make_request("DELETE", f"/collections/{COLLECTION_NAME}")
    if response and response.status_code == 200:
        print(f"✅ Cleanup successful")

if __name__ == "__main__":
    main()