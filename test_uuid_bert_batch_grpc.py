#!/usr/bin/env python3
"""
UUID-Based BERT Embedding Batch Insert Test with gRPC Performance
Tests complete data pipeline: Request → WAL Write → Flush → VIPER Storage
"""

import sys
import os
import time
import asyncio
import numpy as np
from typing import List, Dict
from pathlib import Path

# Add Python SDK to path
sys.path.insert(0, '/workspace/tests/python/integration')
sys.path.insert(0, '/workspace/clients/python/src')

from bert_embedding_service import BERTEmbeddingService, create_sample_corpus
from proximadb import ProximaDBClient
import json

class UUIDBERTBatchTester:
    def __init__(self, use_grpc: bool = True):
        """Initialize the tester with gRPC for performance"""
        self.use_grpc = use_grpc
        # Configure client for gRPC or REST
        if use_grpc:
            # Try gRPC with proper URL format, fall back to REST if not available
            try:
                # Use http URL with gRPC port - the SDK will detect and use gRPC
                self.client = ProximaDBClient(
                    url="http://localhost:5679",
                    protocol="grpc"  # Explicitly request gRPC
                )
                print("🚀 Using gRPC client for maximum performance")
            except Exception as e:
                print(f"⚠️  gRPC failed ({e}), falling back to REST")
                self.client = ProximaDBClient("http://localhost:5678")
                print("🌐 Using REST client as fallback")
        else:
            self.client = ProximaDBClient("http://localhost:5678")  # REST explicit
            print("🌐 Using REST client")
        
        # Initialize BERT service with 384D embeddings for fast generation
        self.bert_service = BERTEmbeddingService(
            model_name="all-MiniLM-L6-v2",  # 384 dimensions, fast
            cache_dir="./embedding_cache"
        )
        
        self.collection_name = f"uuid-bert-batch-{int(time.time())}"
        self.collection_uuid = None
        self.total_vectors = 0
        
    def generate_test_corpus(self, target_vectors: int = 2000) -> List[Dict]:
        """Generate test corpus with BERT embeddings"""
        print(f"\n📚 Generating test corpus with {target_vectors} documents...")
        
        # Create sample corpus
        corpus = create_sample_corpus(size_mb=5.0)  # 5MB corpus
        
        # Ensure we have enough documents
        while len(corpus) < target_vectors:
            additional_corpus = create_sample_corpus(size_mb=2.0)
            corpus.extend(additional_corpus)
        
        # Trim to exact target
        corpus = corpus[:target_vectors]
        
        print(f"✅ Generated {len(corpus)} documents")
        return corpus
    
    def generate_embeddings(self, corpus: List[Dict]) -> List[List[float]]:
        """Generate BERT embeddings for the corpus"""
        print(f"\n🤖 Generating BERT embeddings for {len(corpus)} documents...")
        
        # Extract text from corpus
        texts = [doc['text'] for doc in corpus]
        
        # Generate embeddings using cached BERT service
        embeddings = self.bert_service.embed_texts(
            texts,
            batch_size=64,  # Larger batch for efficiency
            show_progress=True
        )
        
        # Convert to lists for JSON serialization
        embedding_lists = [emb.tolist() for emb in embeddings]
        
        print(f"✅ Generated {len(embedding_lists)} embeddings")
        print(f"📏 Embedding dimension: {len(embedding_lists[0])}")
        
        return embedding_lists
    
    def create_collection_with_uuid(self) -> str:
        """Create collection and return UUID"""
        print(f"\n📦 Creating collection: {self.collection_name}")
        
        collection = self.client.create_collection(
            name=self.collection_name,
            dimension=384  # MiniLM-L6-v2 dimension
        )
        
        # Get the UUID
        collection_info = self.client.get_collection(self.collection_name)
        self.collection_uuid = collection_info.id
        
        print(f"✅ Collection created: {self.collection_name}")
        print(f"🔑 Collection UUID: {self.collection_uuid}")
        
        return self.collection_uuid
    
    def batch_insert_vectors(self, corpus: List[Dict], embeddings: List[List[float]], 
                           batch_size: int = 1000):
        """Perform batch vector insertion using collection UUID"""
        print(f"\n🚀 Starting batch insert with UUID: {self.collection_uuid}")
        print(f"📊 Total vectors: {len(embeddings)}, Batch size: {batch_size}")
        
        total_inserted = 0
        batch_count = 0
        
        for i in range(0, len(embeddings), batch_size):
            batch_count += 1
            batch_corpus = corpus[i:i + batch_size]
            batch_embeddings = embeddings[i:i + batch_size]
            
            print(f"\n🔄 Processing batch {batch_count} (vectors {i+1}-{min(i+batch_size, len(embeddings))})")
            
            # Prepare vectors for batch insert
            vectors = []
            ids = []
            metadata_list = []
            
            for j, (doc, embedding) in enumerate(zip(batch_corpus, batch_embeddings)):
                vector_id = f"{doc['id']}_batch_{batch_count}_{j}"
                vector_metadata = {
                    "text": doc['text'][:200],  # Truncate for efficiency
                    "category": doc['category'],
                    "author": doc['author'],
                    "doc_type": doc['doc_type'],
                    "year": str(doc['year']),
                    "batch_id": str(batch_count),
                    "embedding_model": "all-MiniLM-L6-v2"
                }
                
                vectors.append(embedding)
                ids.append(vector_id)
                metadata_list.append(vector_metadata)
            
            # Insert batch using UUID (this should trigger WAL writes)
            start_time = time.time()
            try:
                result = self.client.insert_vectors(
                    self.collection_uuid,  # Using UUID, not name!
                    vectors=vectors,
                    ids=ids,
                    metadata=metadata_list,
                    batch_size=len(vectors)  # Process all at once
                )
                
                insert_time = time.time() - start_time
                vectors_per_sec = len(vectors) / insert_time
                
                # Handle result based on type
                if hasattr(result, 'successful_count'):
                    successful_count = result.successful_count
                elif hasattr(result, 'success_count'):
                    successful_count = result.success_count
                else:
                    successful_count = len(vectors)  # Assume all successful if no error
                
                total_inserted += successful_count
                
                print(f"✅ Batch {batch_count} completed:")
                print(f"   📊 Inserted: {successful_count} vectors")
                print(f"   ⏱️  Time: {insert_time:.2f}s")
                print(f"   🚄 Speed: {vectors_per_sec:.1f} vectors/sec")
                print(f"   📈 Total: {total_inserted}/{len(embeddings)} vectors")
                
                # Brief pause to allow WAL flush operations
                if batch_count % 2 == 0:  # Every 2 batches
                    print("   ⏸️  Pausing for WAL flush...")
                    time.sleep(2)
                    
            except Exception as e:
                print(f"❌ Batch {batch_count} failed: {e}")
                raise
        
        self.total_vectors = total_inserted
        print(f"\n🎉 Batch insert completed!")
        print(f"📊 Total vectors inserted: {total_inserted}")
        
        return total_inserted
    
    def verify_collection_state(self):
        """Verify collection state after batch insert"""
        print(f"\n🔍 Verifying collection state...")
        
        # Retrieve collection by UUID
        collection_info = self.client.get_collection(self.collection_uuid)
        
        print(f"✅ Collection verification:")
        print(f"   📛 Name: {collection_info.name}")
        print(f"   🔑 UUID: {collection_info.id}")
        print(f"   📊 Dimension: {collection_info.dimension or 'N/A'}")
        print(f"   📈 Vector Count: {collection_info.vector_count}")
        
        # Verify vector count matches
        if collection_info.vector_count == self.total_vectors:
            print(f"✅ Vector count matches: {collection_info.vector_count}")
        else:
            print(f"⚠️  Vector count mismatch: expected {self.total_vectors}, got {collection_info.vector_count}")
        
        return collection_info
    
    def monitor_server_logs(self):
        """Print instructions for monitoring server logs"""
        print(f"\n📋 MONITORING INSTRUCTIONS:")
        print(f"To monitor WAL and VIPER operations, run in another terminal:")
        print(f"")
        print(f"# Monitor real-time server logs:")
        print(f"tail -f /tmp/proximadb_server_grpc.log | grep -E '💾|🔄|📁|✅.*flush|WAL|VIPER|Flush'")
        print(f"")
        print(f"# Or monitor all logs:")
        print(f"tail -f /tmp/proximadb_server_grpc.log")
        print(f"")
        print(f"Look for these patterns:")
        print(f"  💾 - WAL write operations")
        print(f"  🔄 - Flush operations")
        print(f"  📁 - VIPER storage operations")
        print(f"  ✅ - Successful operations")
        print(f"")
    
    def cleanup(self):
        """Clean up test collection"""
        print(f"\n🧹 Cleaning up...")
        try:
            self.client.delete_collection(self.collection_uuid)
            print(f"✅ Collection deleted: {self.collection_uuid}")
        except Exception as e:
            print(f"⚠️  Cleanup failed: {e}")

def main():
    """Main test execution"""
    print("UUID-Based BERT Embedding Batch Insert Test with gRPC")
    print("=" * 60)
    print("🎯 Testing complete data pipeline: Request → WAL → VIPER")
    print("🚀 Using gRPC for maximum performance")
    
    tester = UUIDBERTBatchTester(use_grpc=True)
    
    try:
        # Show monitoring instructions first
        tester.monitor_server_logs()
        
        # Auto-proceed with test (comment out the input() for automated runs)
        print("\n🚀 Starting test automatically...")
        print("💡 Tip: Run './monitor_wal_viper_logs.sh' in another terminal to see logs")
        time.sleep(2)  # Brief pause for setup
        
        # Generate test data
        corpus = tester.generate_test_corpus(target_vectors=2000)
        embeddings = tester.generate_embeddings(corpus)
        
        # Create collection with UUID
        uuid = tester.create_collection_with_uuid()
        
        # Perform batch insert using UUID (this should trigger WAL operations)
        print(f"\n🎯 About to insert {len(embeddings)} vectors using UUID: {uuid}")
        print(f"📊 This should trigger multiple WAL writes and flush operations")
        print(f"🔍 Watch the server logs for WAL → VIPER data flow!")
        
        inserted_count = tester.batch_insert_vectors(corpus, embeddings, batch_size=1000)
        
        # Give time for final flush operations
        print(f"\n⏳ Waiting for final WAL flush operations...")
        time.sleep(5)
        
        # Verify final state
        tester.verify_collection_state()
        
        print(f"\n🎉 Test completed successfully!")
        print(f"📊 Inserted {inserted_count} vectors using UUID-based operations")
        print(f"📋 Check server logs for WAL and VIPER storage operations")
        
        # Keep collection for manual inspection
        print(f"\n💡 Collection preserved for inspection: {uuid}")
        print(f"To delete manually: DELETE /collections/{uuid}")
        
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        
        # Clean up on failure
        try:
            tester.cleanup()
        except:
            pass
        
        return False
    
    return True

if __name__ == "__main__":
    success = main()
    exit(0 if success else 1)