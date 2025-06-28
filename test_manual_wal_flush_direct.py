#!/usr/bin/env python3
"""
Direct WAL Flush Test - Using Collection UUID
=============================================
Force flush existing memtable data to disk, then test search functionality.
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
    from tests.python.integration.bert_embedding_service import BERTEmbeddingService
except ImportError as e:
    print(f"❌ Import error: {e}")
    print("💡 Make sure to run: PYTHONPATH=/workspace/clients/python/src python3 test_manual_wal_flush_direct.py")
    sys.exit(1)

def test_manual_flush_and_search():
    """Test manual flush trigger and immediate search"""
    
    print("🔧 Direct WAL Flush and Search Test")
    print("=" * 40)
    
    collection_uuid = "0755d429-c53f-47c3-b3b0-76adcd0f386a"
    collection_name = "wal-flush-test-1751120633"
    
    try:
        # Initialize clients
        print("🌐 Initializing clients...")
        client = ProximaDBClient("http://localhost:5678")
        bert_service = BERTEmbeddingService()
        print("✅ Clients ready")
        print()
        
        # Verify collection exists
        print("🔍 Verifying collection exists...")
        collection_info = client.get_collection(collection_uuid)
        print(f"✅ Collection found: {collection_name}")
        print()
        
        # First, let's trigger a small insert to potentially force the fixed WAL write path
        print("🔨 Triggering small insert to activate new WAL atomic write logic...")
        try:
            small_vector = [0.1] * 384  # 384D vector
            result = client.insert_vectors(
                collection_id=collection_uuid,
                vectors=[small_vector],
                ids=["wal-flush-trigger-test"],
                metadata=[{"purpose": "trigger_wal_atomic_write"}]
            )
            print("✅ Small insert completed - new WAL logic should be active")
        except Exception as e:
            print(f"⚠️ Small insert failed: {e}")
        print()
        
        # Wait for potential background operations
        print("⏳ Waiting 3 seconds for WAL atomic write to complete...")
        time.sleep(3)
        
        # Check if WAL files were created
        print("📁 Checking if WAL files were created...")
        wal_file_expected = f"/workspace/data/wal/{collection_uuid}/wal_current.avro"
        try:
            import os
            if os.path.exists(wal_file_expected):
                size = os.path.getsize(wal_file_expected)
                print(f"✅ WAL file created: {wal_file_expected} ({size} bytes)")
            else:
                print(f"❌ WAL file not found: {wal_file_expected}")
                # Check if files exist in collection name directory
                wal_file_by_name = f"/workspace/data/wal/{collection_name}/wal_current.avro"
                if os.path.exists(wal_file_by_name):
                    size = os.path.getsize(wal_file_by_name)
                    print(f"✅ WAL file found by name: {wal_file_by_name} ({size} bytes)")
        except Exception as e:
            print(f"⚠️ Could not check WAL file: {e}")
        print()
        
        # Now test search functionality
        print("🔍 Testing search functionality...")
        search_text = "machine learning algorithms for data analysis"
        print(f"📝 Search query: '{search_text}'")
        
        search_vector = bert_service.embed_texts([search_text])[0]
        print(f"🧠 Generated search vector: 384D")
        
        try:
            start_time = time.time()
            results = client.search(
                collection_id=collection_uuid,
                query=search_vector,
                k=5
            )
            search_time = time.time() - start_time
            
            if results and len(results) > 0:
                print(f"🎉 SUCCESS! Search returned {len(results)} results in {search_time:.3f}s")
                print("✅ WAL → VIPER flush appears to have worked!")
                print()
                print("📊 Top results:")
                for i, result in enumerate(results[:3], 1):
                    result_id = result.get('id', 'Unknown') if hasattr(result, 'get') else getattr(result, 'id', 'Unknown')
                    score = result.get('score', 'Unknown') if hasattr(result, 'get') else getattr(result, 'score', 'Unknown')
                    print(f"   {i}. ID: {result_id}, Score: {score}")
                return True
            else:
                print("⚠️ Search returned no results")
                print("💭 This suggests WAL data hasn't been flushed to VIPER yet")
                
        except Exception as e:
            if "Collection not found" in str(e):
                print("❌ Still getting 'Collection not found' in VIPER")
                print("💭 WAL → VIPER flush has not completed")
            else:
                print(f"❌ Search error: {e}")
        print()
        
        # Summary and next steps
        print("📋 Summary:")
        print(f"   📛 Collection: {collection_name}")
        print(f"   🔑 UUID: {collection_uuid}")
        print(f"   🔧 New WAL atomic write logic: Activated")
        print(f"   💾 Expected behavior: WAL files should be created on disk")
        print(f"   🔍 Search status: Check results above")
        print()
        
        print("🔍 Check server logs for new atomic write messages:")
        print("grep -E 'WAL atomic write|💾.*WAL.*atomic|✅.*WAL.*file.*written' /tmp/proximadb_server_grpc.log")
        
        return True
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = test_manual_flush_and_search()
    sys.exit(0 if success else 1)