#!/usr/bin/env python3
import requests
import json

base_url = "http://localhost:5678"

print("=== Testing Vector Search - Simplified ===")
print()

collection_name = "recovery_test_collection"

# 1. Simplest possible search
print("1. Simplest search request:")
search_data = {
    "collection_id": collection_name,
    "queries": [{
        "vector": [0.5] * 128
    }],
    "top_k": 10
}

response = requests.post(f"{base_url}/api/v1/vector/search", json=search_data)
print(f"   Response: {response.status_code}")

if response.status_code == 200:
    results = response.json()
    print(f"   ✓ Search successful!")
    print(f"   Full response: {json.dumps(results, indent=2)}")
else:
    print(f"   ✗ Error: {response.text}")

print()

# 2. Let's check if WAL has any data now
print("2. Checking storage directories:")
import os

# Check LSM WAL
wal_dir = "./lsm_wal"
if os.path.exists(wal_dir):
    files = os.listdir(wal_dir)
    print(f"   LSM WAL: {len(files)} files")

# Check main data directory
data_dir = "/tmp/proximadb-test"
if os.path.exists(data_dir):
    for root, dirs, files in os.walk(data_dir):
        if files:
            print(f"   {root}: {len(files)} files")
            for f in files[:3]:
                print(f"     - {f}")

print()
print("=== Test Complete ===")