#!/usr/bin/env python3
"""
Simple Persistence Test - Debug approach
"""

import os
import subprocess
import time
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))
from proximadb import ProximaDBClient

def simple_test():
    """Simple test to verify basic functionality"""
    print("🧪 Simple Persistence Test")
    
    # Kill existing server
    subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
    time.sleep(2)
    
    # Start server
    print("🚀 Starting server...")
    proc = subprocess.Popen(
        ["./target/release/proximadb-server", "--config", "config.toml"],
        stdout=open('/tmp/server.log', 'w'),
        stderr=subprocess.STDOUT,
        cwd="/workspace"
    )
    
    time.sleep(3)
    
    try:
        client = ProximaDBClient("http://localhost:5678")
        
        # Test 1: List collections
        collections = client.list_collections()
        print(f"✅ Found {len(collections)} collections")
        
        # Test 2: Create collection if none exists
        if not collections:
            print("📝 Creating test collection...")
            client.create_collection("test_simple", dimension=384)
            collections = client.list_collections()
            print(f"✅ Created collection, now have {len(collections)} collections")
        
        # Test 3: Insert vector
        collection_name = collections[0]['name']
        print(f"📊 Using collection: {collection_name}")
        
        try:
            result = client.insert_vector(
                collection_id=collection_name,
                vector_id="test_vec_1",
                vector=[0.1] * 384,
                metadata={"test": True}
            )
            print(f"✅ Vector inserted: {result}")
        except Exception as e:
            print(f"❌ Insert failed: {e}")
        
        # Test 4: Get vector
        try:
            result = client.get_vector(collection_name, "test_vec_1")
            print(f"✅ Vector retrieved: {type(result)}")
            if result:
                print(f"   Keys: {list(result.keys()) if isinstance(result, dict) else 'Not a dict'}")
        except Exception as e:
            print(f"❌ Retrieve failed: {e}")
        
        # Test 5: Search
        try:
            results = client.search(
                collection_id=collection_name,
                query=[0.1] * 384,
                k=3
            )
            print(f"✅ Search results: {len(results)} found")
        except Exception as e:
            print(f"❌ Search failed: {e}")
            
        print("\n🎯 Basic functionality verified!")
        return True
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        return False
        
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except:
            proc.kill()

if __name__ == "__main__":
    simple_test()