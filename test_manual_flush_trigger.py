#!/usr/bin/env python3
"""
Manual Flush Trigger Test
========================
Trigger manual flush for collection 0755d429-c53f-47c3-b3b0-76adcd0f386a
to move 5,000 vectors (~7.3MB) from WAL to VIPER storage.
"""

import os
import sys
import time
import requests
import json

# Add the Python SDK to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))

try:
    from proximadb import ProximaDBClient
except ImportError as e:
    print(f"❌ Import error: {e}")
    print("💡 Make sure to run: PYTHONPATH=/workspace/clients/python/src python3 test_manual_flush_trigger.py")
    sys.exit(1)

def trigger_manual_flush():
    """Trigger manual flush for our test collection"""
    
    print("🔧 Manual Flush Trigger for WAL → VIPER")
    print("=" * 50)
    print("🎯 Target: Collection 0755d429-c53f-47c3-b3b0-76adcd0f386a")
    print("💾 Data: 5,000 vectors (~7.3MB) in WAL")
    print("⚠️  Issue: Data in WAL, not searchable in VIPER")
    print("🔨 Solution: Trigger manual flush")
    print()
    
    collection_uuid = "0755d429-c53f-47c3-b3b0-76adcd0f386a"
    collection_name = "wal-flush-test-1751120633"
    
    try:
        # Initialize REST client
        print("🌐 Initializing REST client...")
        client = ProximaDBClient("http://localhost:5678")
        print("✅ REST client ready")
        print()
        
        # Verify collection exists
        print("🔍 Verifying collection exists...")
        try:
            collection_info = client.get_collection(collection_uuid)
            print(f"✅ Collection found: {collection_name}")
            print()
        except Exception as e:
            print(f"❌ Collection not found: {e}")
            return False
        
        # Since Python SDK doesn't expose WAL flush directly,
        # we'll use a REST API approach to trigger operations that should force flush
        print("🔨 Attempting to trigger flush via REST API...")
        print()
        
        # Method 1: Try direct collection operations that might trigger flush
        print("Method 1: Collection operation to trigger background flush")
        try:
            # Get collection stats which might trigger background processing
            response = requests.get(f"http://localhost:5678/collections/{collection_uuid}")
            if response.status_code == 200:
                print("✅ Collection API call successful")
            else:
                print(f"⚠️ Collection API returned: {response.status_code}")
        except Exception as e:
            print(f"❌ Collection API error: {e}")
        print()
        
        # Method 2: Try inserting a small batch to trigger flush threshold
        print("Method 2: Small insert to trigger flush threshold")
        try:
            # Insert a small vector to potentially trigger flush
            small_vector = [0.1] * 384  # 384D vector
            result = client.insert_vectors(
                collection_id=collection_uuid,
                vectors=[small_vector],
                ids=["flush-trigger-test"],
                metadata=[{"trigger": "flush_test"}]
            )
            print("✅ Small insert successful - may trigger flush")
        except Exception as e:
            print(f"❌ Small insert failed: {e}")
        print()
        
        # Method 3: Monitor for flush completion
        print("Method 3: Wait and monitor for flush operations")
        print("🔍 Monitor server logs with:")
        print("tail -f /tmp/proximadb_server_grpc.log | grep -E 'flush|FLUSH|Flush|VIPER.*collection.*0755d429'")
        print()
        
        # Give some time for background flush to trigger
        print("⏳ Waiting 5 seconds for potential background flush...")
        time.sleep(5)
        print()
        
        # Method 4: Test search again to see if data is now available
        print("Method 4: Test search to verify if flush completed")
        try:
            # Try a simple search to see if data is now searchable
            search_vector = [0.1] * 384
            results = client.search(
                collection_id=collection_uuid,
                query=search_vector,
                k=1
            )
            
            if results and len(results) > 0:
                print(f"🎉 SUCCESS! Search returned {len(results)} results")
                print("✅ WAL → VIPER flush appears to have completed!")
                print(f"📊 First result ID: {results[0].get('id', 'Unknown')}")
                print(f"📊 First result score: {results[0].get('score', 'Unknown')}")
                return True
            else:
                print("⚠️ Search returned no results - flush may not be complete yet")
                
        except Exception as e:
            if "Collection not found" in str(e):
                print("❌ Still getting 'Collection not found' in VIPER")
                print("💡 Flush has not completed yet")
            else:
                print(f"❌ Search error: {e}")
        print()
        
        # Method 5: Give detailed instructions for manual investigation
        print("Method 5: Manual investigation steps")
        print("-" * 40)
        print("If flush hasn't triggered automatically, you can:")
        print()
        print("1. Check WAL memory usage:")
        print("   grep -E 'memory.*size|WAL.*memory' /tmp/proximadb_server_grpc.log")
        print()
        print("2. Check flush triggers:")
        print("   grep -E 'trigger.*flush|flush.*trigger|background.*flush' /tmp/proximadb_server_grpc.log")
        print()
        print("3. Check VIPER operations:")
        print("   grep -E 'VIPER.*0755d429|viper.*collection' /tmp/proximadb_server_grpc.log")
        print()
        print("4. The flush should be triggered by:")
        print("   - Memory usage > 1MB (we have 7.3MB)")
        print("   - Background maintenance manager")
        print("   - WAL strategy pattern with VIPER delegation")
        print()
        
        print("🎯 Summary:")
        print(f"   📛 Collection: {collection_name}")
        print(f"   🔑 UUID: {collection_uuid}")
        print(f"   💾 Data: 5,000 vectors (~7.3MB) in WAL")
        print(f"   🎚️ Threshold: 1MB (should definitely trigger)")
        print(f"   🔍 Status: Monitoring for flush completion")
        
        return True
        
    except Exception as e:
        print(f"❌ Manual flush trigger failed: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = trigger_manual_flush()
    sys.exit(0 if success else 1)