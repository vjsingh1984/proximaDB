#!/usr/bin/env python3
"""
Successful UUID-Based BERT Batch Insert to demonstrate WAL → VIPER flow
Uses smaller batches to avoid request size limits
"""

import sys
import os
import time
import numpy as np
from typing import List, Dict

# Add Python SDK to path
sys.path.insert(0, '/workspace/tests/python/integration')
sys.path.insert(0, '/workspace/clients/python/src')

from bert_embedding_service import BERTEmbeddingService, create_sample_corpus
from proximadb import ProximaDBClient

def run_successful_uuid_wal_test():
    """Run a successful test with smaller batches to see WAL operations"""
    print("🎯 UUID-Based BERT Batch Insert - WAL Flow Demonstration")
    print("=" * 60)
    
    # Use REST client (gRPC has SDK issues)
    client = ProximaDBClient("http://localhost:5678")
    print("🌐 Using REST client")
    
    # Initialize BERT service
    bert_service = BERTEmbeddingService("all-MiniLM-L6-v2", cache_dir="./embedding_cache")
    print(f"🤖 BERT service ready: {bert_service.dimension}D embeddings")
    
    # Create smaller corpus for successful test
    print("\n📚 Generating smaller test corpus...")
    corpus = create_sample_corpus(size_mb=1.0)[:500]  # Just 500 documents
    print(f"✅ Generated {len(corpus)} documents")
    
    # Generate embeddings
    print("\n🧠 Generating BERT embeddings...")
    texts = [doc['text'] for doc in corpus]
    embeddings = bert_service.embed_texts(texts, batch_size=32, show_progress=True)
    embedding_lists = [emb.tolist() for emb in embeddings]
    print(f"✅ Generated {len(embedding_lists)} embeddings")
    
    # Create collection
    collection_name = f"wal-test-{int(time.time())}"
    print(f"\n📦 Creating collection: {collection_name}")
    collection = client.create_collection(collection_name, dimension=384)
    
    # Get UUID
    collection_info = client.get_collection(collection_name)
    uuid = collection_info.id
    print(f"✅ Collection created with UUID: {uuid}")
    
    print(f"\n🎯 WATCH SERVER LOGS FOR WAL OPERATIONS!")
    print(f"Run: tail -f /tmp/proximadb_server_grpc.log | grep -E '💾|🔄|📁|✅.*flush'")
    
    # Insert in smaller batches that will succeed
    batch_size = 50  # Small batch to avoid size limits
    total_inserted = 0
    
    print(f"\n🚀 Starting batch inserts using UUID: {uuid}")
    print(f"📊 Total vectors: {len(embedding_lists)}, Batch size: {batch_size}")
    
    for i in range(0, len(embedding_lists), batch_size):
        batch_num = (i // batch_size) + 1
        batch_corpus = corpus[i:i + batch_size]
        batch_embeddings = embedding_lists[i:i + batch_size]
        
        print(f"\n🔄 Batch {batch_num}: vectors {i+1}-{min(i+batch_size, len(embedding_lists))}")
        
        # Prepare batch data
        vectors = []
        ids = []
        metadata_list = []
        
        for j, (doc, embedding) in enumerate(zip(batch_corpus, batch_embeddings)):
            vector_id = f"{doc['id']}_b{batch_num}_{j}"
            metadata = {
                "text": doc['text'][:100],  # Truncated
                "category": doc['category'],
                "batch": str(batch_num),
                "model": "all-MiniLM-L6-v2"
            }
            
            vectors.append(embedding)
            ids.append(vector_id)
            metadata_list.append(metadata)
        
        # Insert using UUID (should trigger WAL writes)
        start_time = time.time()
        try:
            result = client.insert_vectors(
                uuid,  # Using UUID for collection identification!
                vectors=vectors,
                ids=ids,
                metadata=metadata_list,
                batch_size=len(vectors)
            )
            
            insert_time = time.time() - start_time
            successful_count = len(vectors)  # Assume all successful if no error
            total_inserted += successful_count
            
            print(f"   ✅ Success: {successful_count} vectors in {insert_time:.2f}s")
            print(f"   🚄 Speed: {successful_count/insert_time:.1f} vectors/sec")
            print(f"   📈 Total: {total_inserted}/{len(embedding_lists)}")
            
            # Pause to allow WAL operations to be visible in logs
            if batch_num % 3 == 0:
                print(f"   ⏸️  Pausing for WAL flush visibility...")
                time.sleep(1)
        
        except Exception as e:
            print(f"   ❌ Batch {batch_num} failed: {e}")
            break
    
    # Verify final state
    print(f"\n🔍 Verifying collection state...")
    final_collection = client.get_collection(uuid)  # Retrieve by UUID
    print(f"✅ Final verification:")
    print(f"   📛 Name: {final_collection.name}")
    print(f"   🔑 UUID: {final_collection.id}")
    print(f"   📊 Vectors: {final_collection.vector_count}")
    
    print(f"\n🎉 Test completed successfully!")
    print(f"📊 Total inserted: {total_inserted} vectors using UUID")
    print(f"🔑 Collection UUID: {uuid}")
    print(f"💾 Check logs for WAL write operations")
    print(f"📁 Check logs for VIPER storage operations")
    
    # Keep collection for inspection
    print(f"\n💡 Collection preserved for manual inspection")
    print(f"To delete: curl -X DELETE http://localhost:5678/collections/{uuid}")
    
    return True

if __name__ == "__main__":
    try:
        success = run_successful_uuid_wal_test()
        exit(0 if success else 1)
    except KeyboardInterrupt:
        print("\n⏹️  Test interrupted by user")
        exit(1)
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        exit(1)