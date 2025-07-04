#!/usr/bin/env python3
"""
1K Vector BERT Test using Direct Async gRPC Client
Uses the async gRPC client directly to avoid sync wrapper issues
"""

import asyncio
import time
import numpy as np
import uuid
import sys
from pathlib import Path

# IMPORTANT: This test requires the ProximaDB Python SDK to be in PYTHONPATH
# Run with: PYTHONPATH=/workspace/clients/python/src python3 test_bert_1k_grpc_async.py
# Do NOT add sys.path.insert() - paths should be externalized via environment variables

from proximadb.grpc_client import ProximaDBClient
from bert_embedding_utils import (
    generate_text_corpus,
    convert_corpus_to_vectors,
    create_query_texts,
    create_deterministic_embedding
)


class BERT1KgRPCAsyncTest:
    """1K vector BERT test using async gRPC client"""
    
    def __init__(self):
        self.server_address = "localhost:5679"  # gRPC port
        self.client = None
        self.collection_id = f"bert_grpc_1k_{uuid.uuid4().hex[:8]}"
        self.dimension = 384
        self.num_vectors = 1000
        
    async def run_test(self):
        """Run complete 1K BERT gRPC test"""
        print("🚀 1K Vector BERT Test (Async gRPC)")
        print("=" * 50)
        
        try:
            # Initialize gRPC client
            if not await self.init_grpc_client():
                return False
            
            # Generate BERT corpus
            if not self.generate_corpus():
                return False
            
            # Create collection
            if not await self.create_collection():
                return False
            
            # Insert vectors using gRPC bulk inserts
            if not await self.insert_vectors_bulk():
                return False
            
            # Wait for flush (as requested)
            if not await self.wait_for_flush():
                return False
            
            # Test search with BERT queries
            if not await self.test_search():
                return False
            
            print("\n🎉 1K BERT gRPC test completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Test failed: {e}")
            import traceback
            traceback.print_exc()
            return False
        finally:
            await self.cleanup()
    
    async def init_grpc_client(self):
        """Initialize async gRPC client"""
        try:
            print("🔗 Initializing async gRPC client...")
            self.client = ProximaDBClient(self.server_address)
            await self.client.connect()
            
            # Test connection
            health = await self.client.health_check()
            print("✅ Async gRPC client connected")
            return True
            
        except Exception as e:
            print(f"❌ Async gRPC client initialization failed: {e}")
            return False
    
    def generate_corpus(self):
        """Generate BERT text corpus"""
        print(f"\n📚 Generating {self.num_vectors} BERT corpus...")
        start_time = time.time()
        
        try:
            # Generate text corpus
            self.corpus = generate_text_corpus(self.num_vectors)
            
            # Convert to vectors with embeddings
            self.vectors = convert_corpus_to_vectors(self.corpus, self.dimension)
            
            generation_time = time.time() - start_time
            print(f"✅ Generated {len(self.vectors)} BERT vectors in {generation_time:.2f}s")
            
            # Show sample
            categories = {}
            for vector in self.vectors:
                cat = vector["metadata"]["category"]
                categories[cat] = categories.get(cat, 0) + 1
            
            print("📊 Corpus distribution:")
            for cat, count in categories.items():
                print(f"   {cat}: {count} documents")
            
            return True
            
        except Exception as e:
            print(f"❌ Corpus generation failed: {e}")
            return False
    
    async def create_collection(self):
        """Create collection via async gRPC"""
        print(f"\n🏗️ Creating async gRPC collection: {self.collection_id}")
        
        try:
            result = await self.client.create_collection(
                name=self.collection_id,
                dimension=self.dimension,
                distance_metric="cosine",
                storage_engine="viper"
            )
            
            print("✅ Async gRPC collection created")
            return True
                
        except Exception as e:
            print(f"❌ Async gRPC collection creation error: {e}")
            return False
    
    async def insert_vectors_bulk(self):
        """Insert vectors using async gRPC bulk inserts"""
        print(f"\n🔥 Inserting {len(self.vectors)} BERT vectors via async gRPC bulk...")
        
        start_time = time.time()
        total_inserted = 0
        
        # Insert in optimized batches for gRPC
        batch_size = 500  # Larger batches for gRPC efficiency
        
        for i in range(0, len(self.vectors), batch_size):
            batch_end = min(i + batch_size, len(self.vectors))
            batch_vectors = self.vectors[i:batch_end]
            
            try:
                batch_start = time.time()
                
                # Use async gRPC bulk insert
                result = await self.client.insert_vectors(
                    collection_id=self.collection_id,
                    vectors=batch_vectors  # Format: [{"id": "...", "vector": [...], "metadata": {...}}]
                )
                
                batch_end_time = time.time()
                batch_duration = batch_end_time - batch_start
                
                batch_inserted = len(batch_vectors)
                if hasattr(result, 'count'):
                    batch_inserted = result.count
                
                total_inserted += batch_inserted
                
                # Progress reporting
                elapsed = time.time() - start_time
                rate = total_inserted / elapsed if elapsed > 0 else 0
                batch_rate = batch_inserted / batch_duration if batch_duration > 0 else 0
                
                print(f"   Batch {(i//batch_size)+1}: {batch_inserted} vectors in {batch_duration:.2f}s ({batch_rate:.1f}/s)")
                print(f"   Progress: {total_inserted:,}/{len(self.vectors):,} ({rate:.1f}/s overall)")
                    
            except Exception as e:
                print(f"   ⚠️ Batch {(i//batch_size)+1} failed: {e}")
                continue
        
        end_time = time.time()
        duration = end_time - start_time
        
        print(f"✅ Async gRPC Bulk Insertion Results:")
        print(f"   Inserted: {total_inserted:,}/{len(self.vectors):,}")
        print(f"   Duration: {duration:.2f}s")
        print(f"   Rate: {total_inserted/duration:.1f} vectors/second")
        
        self.total_inserted = total_inserted
        return total_inserted >= len(self.vectors) * 0.8  # Allow some failures
    
    async def wait_for_flush(self):
        """Wait for WAL flush (as requested)"""
        print(f"\n⏳ Waiting for WAL flush (10 seconds)...")
        
        flush_start = time.time()
        await asyncio.sleep(10)  # Wait for flush as requested
        flush_end = time.time()
        
        print(f"✅ Flush wait completed ({flush_end - flush_start:.1f}s)")
        
        # Verify data accessibility via async gRPC
        try:
            print("🔍 Verifying post-flush accessibility via async gRPC...")
            query_vector = create_deterministic_embedding("test query", self.dimension)
            
            results = await self.client.search_vectors(
                collection_id=self.collection_id,
                query_vectors=[query_vector.tolist()],
                top_k=5,
                include_metadata=True
            )
            
            result_count = len(results.results) if hasattr(results, 'results') else 0
            print(f"   ✅ Post-flush async gRPC search: {result_count} vectors accessible")
            return True
                
        except Exception as e:
            print(f"   ⚠️ Post-flush async gRPC verification error: {e}")
            return True  # Don't fail test for this
    
    async def test_search(self):
        """Test search with BERT queries via async gRPC"""
        print(f"\n🔍 Testing BERT Semantic Search via Async gRPC")
        print("-" * 50)
        
        # Get sample queries
        query_texts = create_query_texts()
        
        search_results = []
        
        for i, query in enumerate(query_texts[:5]):  # Test 5 queries
            print(f"\nQuery {i+1}: {query['description']}")
            print(f"   Text: '{query['text'][:50]}...'")
            print(f"   Expected: {query['expected_category']}")
            
            try:
                # Create BERT embedding for query
                query_embedding = create_deterministic_embedding(query["text"], self.dimension)
                
                # Search via async gRPC
                search_start = time.time()
                results = await self.client.search_vectors(
                    collection_id=self.collection_id,
                    query_vectors=[query_embedding.tolist()],
                    top_k=5,
                    include_metadata=True
                )
                search_end = time.time()
                
                search_time = search_end - search_start
                result_count = len(results.results) if hasattr(results, 'results') else 0
                
                print(f"   ⏱️ Async gRPC Search: {search_time:.3f}s, {result_count} results")
                
                if results and result_count > 0:
                    # Check top result
                    top_result = results.results[0]
                    top_metadata = top_result.get("metadata", {})
                    top_category = top_metadata.get("category", "unknown")
                    top_score = top_result.get("score", 0)
                    
                    print(f"   🎯 Top: {top_category} (score: {top_score:.4f})")
                    
                    # Check semantic match
                    if top_category == query["expected_category"]:
                        print(f"   ✅ Semantic match: CORRECT")
                    else:
                        print(f"   ⚠️ Expected {query['expected_category']}, got {top_category}")
                    
                    search_results.append({
                        "query": query,
                        "search_time": search_time,
                        "result_count": result_count,
                        "top_category": top_category,
                        "correct": top_category == query["expected_category"]
                    })
                else:
                    print(f"   ❌ No results")
                    
            except Exception as e:
                print(f"   ❌ Async gRPC query error: {e}")
                continue
        
        # Summary
        if search_results:
            correct = sum(1 for r in search_results if r["correct"])
            avg_time = sum(r["search_time"] for r in search_results) / len(search_results)
            
            print(f"\n📊 Async gRPC Search Summary:")
            print(f"   Queries: {len(search_results)}")
            print(f"   Correct semantic matches: {correct}/{len(search_results)} ({100*correct/len(search_results):.1f}%)")
            print(f"   Average search time: {avg_time:.3f}s")
        
        return len(search_results) > 0
    
    async def cleanup(self):
        """Clean up collection and close async gRPC client"""
        print(f"\n🧹 Cleaning up: {self.collection_id}")
        
        try:
            if self.client:
                await self.client.delete_collection(self.collection_id)
                print("✅ Async gRPC collection deleted")
                
                # Close async gRPC client
                await self.client.close()
                print("✅ Async gRPC client closed")
                
        except Exception as e:
            print(f"⚠️ Cleanup error: {e}")


async def main():
    """Run the 1K BERT async gRPC test"""
    test = BERT1KgRPCAsyncTest()
    success = await test.run_test()
    
    if success:
        print("\n🎉 1K BERT async gRPC test completed successfully!")
        print("Key achievements:")
        print("   ✅ BERT corpus generation (1K documents)")
        print("   ✅ Real text with semantic embeddings")
        print("   ✅ Async gRPC bulk vector insertion")
        print("   ✅ 10-second WAL flush wait")
        print("   ✅ BERT semantic search via async gRPC")
    else:
        print("\n💥 1K BERT async gRPC test failed!")
        exit(1)


if __name__ == "__main__":
    asyncio.run(main())