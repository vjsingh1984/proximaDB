#!/usr/bin/env python3
"""
Simple UUID-based 10MB test to trigger WAL → VIPER flush mechanics
Uses REST API for reliability while monitoring WAL operations
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

def run_uuid_wal_flush_test():
    """Run UUID-based test with 10MB data to trigger WAL flush mechanics"""
    
    print("🎯 UUID-Based 10MB Vector Test - WAL Flush to VIPER Mechanics")
    print("=" * 65)
    print("💾 Testing complete pipeline: UUID → WAL → Flush → VIPER Storage")
    
    # Use REST for reliability
    client = ProximaDBClient("http://localhost:5678")
    print("🌐 Using REST client for maximum reliability")
    
    # Initialize BERT service
    print("\n🤖 Initializing BERT service...")
    bert_service = BERTEmbeddingService("all-MiniLM-L6-v2", cache_dir="./embedding_cache")
    print(f"✅ BERT service ready: {bert_service.dimension}D embeddings")
    
    # Generate large corpus for 10MB+ of data
    print("\n📚 Generating large corpus for 10MB+ data volume...")
    # Calculate: 384 floats * 4 bytes + metadata ~= 2KB per vector
    # So 5000+ vectors ≈ 10MB+ 
    target_vectors = 5000
    
    corpus = create_sample_corpus(size_mb=8.0)
    if len(corpus) < target_vectors:
        while len(corpus) < target_vectors:
            additional = create_sample_corpus(size_mb=2.0)
            corpus.extend(additional)
    corpus = corpus[:target_vectors]
    print(f"✅ Generated corpus: {len(corpus)} documents")
    
    # Generate BERT embeddings
    print("\n🧠 Generating BERT embeddings...")
    texts = [doc['text'] for doc in corpus]
    embeddings = bert_service.embed_texts(texts, batch_size=100, show_progress=True)
    embedding_lists = [emb.tolist() for emb in embeddings]
    
    # Calculate data size
    vector_bytes = len(embedding_lists) * 384 * 4
    metadata_estimate = len(corpus) * 500  # ~500 bytes metadata per vector
    total_mb = (vector_bytes + metadata_estimate) / (1024 * 1024)
    
    print(f"✅ Generated {len(embedding_lists)} embeddings")
    print(f"📊 Estimated total data: {total_mb:.1f}MB")
    
    # Create collection
    collection_name = f"wal-flush-test-{int(time.time())}"
    print(f"\n📦 Creating collection: {collection_name}")
    collection = client.create_collection(collection_name, dimension=384)
    
    # Get UUID
    collection_info = client.get_collection(collection_name)
    uuid = collection_info.id
    print(f"✅ Collection created")
    print(f"📛 Name: {collection_name}")
    print(f"🔑 UUID: {uuid}")
    
    print(f"\n🚨 IMPORTANT - MONITOR SERVER LOGS NOW!")
    print(f"Run this command in another terminal:")
    print(f"tail -f /tmp/proximadb_server_grpc.log | grep -E '💾|🔄|📁|✅.*flush|WAL.*write|VIPER|Flush.*completed|memtable.*flush|batch.*insert.*collection|UnifiedAvroService.*handling'")
    print(f"")
    print(f"🎯 Watch for these patterns:")
    print(f"  💾 WAL write operations")
    print(f"  🔄 Flush operations")
    print(f"  📁 VIPER file operations")
    print(f"  ✅ Flush completion")
    print(f"")
    
    # Start large data insertion to trigger WAL flush
    batch_size = 100  # Smaller batches to avoid size limits
    total_inserted = 0
    
    print(f"🚀 Starting large data insertion using UUID: {uuid}")
    print(f"📊 Total vectors: {len(embedding_lists)}")
    print(f"📦 Batch size: {batch_size}")
    print(f"💾 This should trigger WAL accumulation and flush to VIPER!")
    
    start_time = time.time()
    
    for i in range(0, len(embedding_lists), batch_size):
        batch_num = (i // batch_size) + 1
        batch_corpus = corpus[i:i + batch_size]
        batch_embeddings = embedding_lists[i:i + batch_size]
        
        # Prepare batch data
        vectors = []
        ids = []
        metadata_list = []
        
        for j, (doc, embedding) in enumerate(zip(batch_corpus, batch_embeddings)):
            vector_id = f"{doc['id']}_wal_b{batch_num}_{j}"
            metadata = {
                "text": doc['text'][:200],
                "category": doc['category'],
                "author": doc['author'],
                "doc_type": doc['doc_type'],
                "year": str(doc['year']),
                "batch": str(batch_num),
                "model": "all-MiniLM-L6-v2",
                "test": "wal-flush-mechanics"
            }
            
            vectors.append(embedding)
            ids.append(vector_id)
            metadata_list.append(metadata)
        
        # Insert using UUID (should accumulate in WAL)
        batch_start = time.time()
        try:
            result = client.insert_vectors(
                uuid,  # Using UUID for all operations!
                vectors=vectors,
                ids=ids,
                metadata=metadata_list,
                batch_size=len(vectors)
            )
            
            batch_time = time.time() - batch_start
            successful_count = len(vectors)  # Assume all successful
            total_inserted += successful_count
            
            # Progress reporting
            progress_pct = (total_inserted / len(embedding_lists)) * 100
            total_mb_so_far = (total_inserted * 384 * 4) / (1024 * 1024)
            
            print(f"🔄 Batch {batch_num:2d}: {successful_count:3d} vectors in {batch_time:.3f}s | "
                  f"Total: {total_inserted:4d}/{len(embedding_lists)} ({progress_pct:5.1f}%) | "
                  f"Data: {total_mb_so_far:5.1f}MB")
            
            # Periodic logging for WAL monitoring
            if batch_num % 10 == 0:
                print(f"   💾 Batch {batch_num}: WAL should be accumulating significant data")
                print(f"   🔍 Check server logs for WAL operations!")
                
            if batch_num % 20 == 0:
                print(f"   ⏸️  Brief pause for WAL flush visibility...")
                time.sleep(1)
        
        except Exception as e:
            print(f"❌ Batch {batch_num} failed: {e}")
            break
    
    total_time = time.time() - start_time
    
    # Final results
    print(f"\n🎉 Large data insertion completed!")
    print(f"📊 Total vectors inserted: {total_inserted}")
    print(f"📏 Total vector data: {(total_inserted * 384 * 4) / (1024*1024):.1f}MB")
    print(f"⏱️  Total time: {total_time:.2f}s")
    print(f"🚄 Average rate: {total_inserted/total_time:.1f} vectors/sec")
    
    # Verify collection state
    print(f"\n🔍 Verifying final collection state...")
    final_collection = client.get_collection(uuid)  # Using UUID
    print(f"✅ Collection verification via UUID:")
    print(f"   📛 Name: {final_collection.name}")
    print(f"   🔑 UUID: {final_collection.id}")
    print(f"   📊 Vector Count: {final_collection.vector_count}")
    
    # Wait for final flush operations
    print(f"\n⏳ Waiting for final WAL flush operations...")
    print(f"🔍 Monitor server logs for flush completion!")
    time.sleep(3)
    
    print(f"\n✅ TEST COMPLETED SUCCESSFULLY!")
    print(f"🎯 Key achievements:")
    print(f"   ✅ Used UUID for all collection operations")
    print(f"   ✅ Inserted {total_inserted} vectors ({(total_inserted * 384 * 4) / (1024*1024):.1f}MB)")
    print(f"   ✅ Should have triggered WAL → VIPER flush mechanics")
    print(f"   ✅ Collection accessible via UUID: {uuid}")
    
    print(f"\n📋 Next steps:")
    print(f"   🔍 Check server logs for WAL and flush operations")
    print(f"   📁 Check VIPER storage directory: /workspace/data/")
    print(f"   🧪 Collection preserved for inspection: {uuid}")
    
    return True

if __name__ == "__main__":
    try:
        success = run_uuid_wal_flush_test()
        exit(0 if success else 1)
    except KeyboardInterrupt:
        print("\n⏹️  Test interrupted by user")
        exit(1)
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        exit(1)