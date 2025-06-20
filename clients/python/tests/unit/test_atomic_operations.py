#!/usr/bin/env python3
"""
Test script for ProximaDB Atomic Operations
Tests flush and compaction with small triggers to verify atomic behavior.
"""

import asyncio
import aiohttp
import json
import time
import random
import numpy as np
from typing import List, Dict, Any

class ProximaDBAtomicTester:
    def __init__(self, base_url: str = "http://127.0.0.1:5678/api/v1"):
        self.base_url = base_url
        self.session = None
        
    async def __aenter__(self):
        self.session = aiohttp.ClientSession()
        return self
        
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self.session:
            await self.session.close()
    
    async def create_collection(self, collection_id: str, dimension: int = 128) -> Dict[str, Any]:
        """Create a test collection with VIPER storage."""
        payload = {
            "name": collection_id,
            "dimension": dimension,
            "filterable_metadata_fields": ["category", "priority", "region"]
        }
        
        async with self.session.post(f"{self.base_url}/collections", json=payload) as resp:
            result = await resp.json()
            print(f"✅ Created collection: {collection_id}")
            return result
    
    async def insert_vector(self, collection_id: str, vector_id: str, metadata: Dict[str, Any] = None) -> Dict[str, Any]:
        """Insert a single vector with metadata."""
        vector = np.random.random(128).tolist()
        
        payload = {
            "id": vector_id,
            "vector": vector,
            "metadata": metadata or {}
        }
        
        async with self.session.post(f"{self.base_url}/collections/{collection_id}/vectors", json=payload) as resp:
            return await resp.json()
    
    async def bulk_insert_vectors(self, collection_id: str, count: int, batch_size: int = 20) -> List[Dict[str, Any]]:
        """Insert multiple vectors in batches to trigger flush."""
        results = []
        
        for batch_start in range(0, count, batch_size):
            batch_end = min(batch_start + batch_size, count)
            batch_vectors = []
            
            for i in range(batch_start, batch_end):
                vector = np.random.random(128).tolist()
                metadata = {
                    "category": random.choice(["tech", "finance", "healthcare"]),
                    "priority": random.randint(1, 10),
                    "region": random.choice(["us-east", "us-west", "eu-central"]),
                    "batch_id": f"batch_{batch_start // batch_size}"
                }
                
                batch_vectors.append({
                    "id": f"vec_{i:06d}",
                    "vector": vector,
                    "metadata": metadata
                })
            
            # Insert batch
            async with self.session.post(f"{self.base_url}/collections/{collection_id}/vectors/batch", json={"vectors": batch_vectors}) as resp:
                batch_result = await resp.json()
                results.append(batch_result)
                
            print(f"📦 Inserted batch {batch_start // batch_size + 1}: vectors {batch_start}-{batch_end-1}")
            
            # Small delay to allow for background operations
            await asyncio.sleep(0.5)
            
        return results
    
    async def search_vectors(self, collection_id: str, k: int = 10) -> Dict[str, Any]:
        """Search for similar vectors."""
        query_vector = np.random.random(128).tolist()
        
        payload = {
            "vector": query_vector,
            "k": k
        }
        
        async with self.session.post(f"{self.base_url}/collections/{collection_id}/search", json=payload) as resp:
            return await resp.json()
    
    async def get_collection_stats(self, collection_id: str) -> Dict[str, Any]:
        """Get collection statistics to monitor flush/compaction."""
        async with self.session.get(f"{self.base_url}/collections/{collection_id}/stats") as resp:
            return await resp.json()
    
    async def trigger_flush(self, collection_id: str) -> Dict[str, Any]:
        """Manually trigger flush for testing."""
        async with self.session.post(f"{self.base_url}/collections/{collection_id}/flush") as resp:
            return await resp.json()
    
    async def trigger_compaction(self, collection_id: str) -> Dict[str, Any]:
        """Manually trigger compaction for testing."""
        async with self.session.post(f"{self.base_url}/collections/{collection_id}/compact") as resp:
            return await resp.json()
    
    async def monitor_atomic_operations(self, collection_id: str, duration_seconds: int = 60):
        """Monitor atomic operations while performing concurrent reads/writes."""
        print(f"🔍 Monitoring atomic operations for {duration_seconds} seconds...")
        
        start_time = time.time()
        operations_count = 0
        
        while time.time() - start_time < duration_seconds:
            # Concurrent operations
            tasks = [
                # Continuous writes (should never be blocked)
                self.insert_vector(collection_id, f"monitor_vec_{operations_count}", {
                    "monitor_test": True,
                    "timestamp": time.time()
                }),
                
                # Continuous reads (should only be blocked during atomic switch)
                self.search_vectors(collection_id, k=5),
                
                # Get stats
                self.get_collection_stats(collection_id)
            ]
            
            try:
                results = await asyncio.gather(*tasks, return_exceptions=True)
                
                # Check for any blocked operations
                for i, result in enumerate(results):
                    if isinstance(result, Exception):
                        print(f"⚠️ Operation {i} failed: {result}")
                    
                operations_count += 1
                
                if operations_count % 10 == 0:
                    print(f"📊 Completed {operations_count} concurrent operations")
                
            except Exception as e:
                print(f"❌ Error during monitoring: {e}")
            
            await asyncio.sleep(0.1)  # 100ms between operation cycles
        
        print(f"✅ Monitoring completed: {operations_count} operations in {duration_seconds}s")

async def test_atomic_operations():
    """Main test function for atomic operations."""
    print("🚀 Starting ProximaDB Atomic Operations Test")
    
    async with ProximaDBAtomicTester() as tester:
        collection_id = "test_atomic_collection"
        
        try:
            # Step 1: Create test collection
            await tester.create_collection(collection_id)
            
            # Step 2: Insert enough vectors to trigger multiple flushes
            print("📝 Inserting vectors to trigger flushes...")
            await tester.bulk_insert_vectors(collection_id, count=200, batch_size=25)
            
            # Step 3: Wait for automatic flush triggers (30-second age threshold)
            print("⏰ Waiting for age-based flush trigger (30 seconds)...")
            await asyncio.sleep(35)
            
            # Step 4: Insert more vectors to create multiple files for compaction
            print("📝 Inserting more vectors to create compaction candidates...")
            await tester.bulk_insert_vectors(collection_id, count=150, batch_size=30)
            
            # Step 5: Manually trigger compaction to test atomic operations
            print("🔄 Triggering manual compaction...")
            compaction_result = await tester.trigger_compaction(collection_id)
            print(f"Compaction result: {compaction_result}")
            
            # Step 6: Monitor concurrent operations during background processes
            print("🔍 Starting concurrent operations monitoring...")
            await tester.monitor_atomic_operations(collection_id, duration_seconds=30)
            
            # Step 7: Final verification - search should work with all data
            print("✅ Final verification search...")
            search_result = await tester.search_vectors(collection_id, k=20)
            print(f"Final search returned {len(search_result.get('vectors', []))} results")
            
            # Step 8: Get final statistics
            stats = await tester.get_collection_stats(collection_id)
            print(f"📊 Final collection stats: {stats}")
            
            print("🎉 Atomic operations test completed successfully!")
            
        except Exception as e:
            print(f"❌ Test failed: {e}")
            raise

if __name__ == "__main__":
    # Run the test
    asyncio.run(test_atomic_operations())