#!/usr/bin/env python3

import requests
import json
import random
import time
import os

# Configuration
BASE_URL = "http://localhost:5678"
COLLECTION_NAME = "debug_disk_test"

def create_collection():
    """Create test collection"""
    payload = {
        "name": COLLECTION_NAME,
        "dimension": 768,
        "distance_metric": "cosine",
        "storage_engine": "viper"
    }
    
    response = requests.post(f"{BASE_URL}/collections", json=payload)
    print(f"Collection creation status: {response.status_code}")
    return response.status_code == 200

def insert_single_batch():
    """Insert a single batch to trigger disk writing with detailed logs"""
    vectors = []
    for i in range(3):  # Small batch to isolate the issue
        vector = [random.random() for _ in range(768)]
        vectors.append({
            "id": f"debug_test_{i:03d}",
            "vector": vector,
            "metadata": {
                "debug": "true",
                "index": str(i),
                "timestamp": str(int(time.time()))
            }
        })
    
    print(f"\n🚀 Inserting {len(vectors)} vectors to trigger WAL disk writing...")
    response = requests.post(f"{BASE_URL}/collections/{COLLECTION_NAME}/vectors/batch", json=vectors)
    print(f"Insert status: {response.status_code}")
    
    if response.status_code != 200:
        print(f"❌ Insert failed: {response.text}")
        return False
    
    return True

def check_wal_files():
    """Check for WAL files to see if disk writing worked"""
    wal_paths = [
        f"/workspace/data/disk1/wal/{COLLECTION_NAME}",
        f"/workspace/data/disk2/wal/{COLLECTION_NAME}", 
        f"/workspace/data/disk3/wal/{COLLECTION_NAME}"
    ]
    
    total_files = 0
    for path in wal_paths:
        if os.path.exists(path):
            files = [f for f in os.listdir(path) if f.endswith('.avro')]
            if files:
                total_files += len(files)
                print(f"  ✅ {path}: {len(files)} WAL files")
                for f in sorted(files):
                    full_path = os.path.join(path, f)
                    size = os.path.getsize(full_path)
                    print(f"    📄 {f} ({size} bytes)")
            else:
                print(f"  ⚪ {path}: directory exists but no .avro files")
    
    return total_files

def main():
    print("🐛 DEBUG: WAL Disk Writing Analysis")
    print("=" * 45)
    print("🎯 Goal: Identify exact point of WAL disk writing failure")
    print("📋 Expected: Detailed logs showing where Avro serialization fails")
    
    # Create collection
    print("\n📁 Creating collection...")
    if not create_collection():
        print("❌ Failed to create collection")
        return
    
    # Wait for collection to be ready
    time.sleep(1)
    
    # Insert single batch and check immediately
    if not insert_single_batch():
        print("❌ Failed to insert vectors")
        return
    
    # Check immediately for disk files
    print(f"\n🔍 Checking for WAL files on disk...")
    wal_files = check_wal_files()
    
    # Wait a moment for async operations to complete
    print(f"\n⏳ Waiting 3 seconds for async disk operations...")
    time.sleep(3)
    
    # Check again
    print(f"\n🔍 Final check for WAL files...")
    final_files = check_wal_files()
    
    print(f"\n📊 ANALYSIS:")
    if final_files > 0:
        print(f"✅ SUCCESS: WAL disk writing is working! Found {final_files} files")
    else:
        print(f"❌ ISSUE: No WAL files found on disk")
        print(f"📋 Check debug_wal_disk.log for detailed error information")
        print(f"🔍 Look for these log patterns:")
        print(f"   - 💾 [DISK] Starting Avro serialization")
        print(f"   - ❌ Failed to convert WAL entry")
        print(f"   - ❌ Failed to serialize WAL entries")

if __name__ == "__main__":
    main()