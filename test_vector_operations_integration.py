#!/usr/bin/env python3
"""
Test Vector Operations Integration
Tests all vector operations through the REST API to verify complete integration.
"""

import asyncio
import json
import uuid
import random
from typing import Dict, List, Any
import aiohttp
import sys
import traceback

class VectorOperationsTest:
    def __init__(self, base_url: str = "http://localhost:5678"):
        self.base_url = base_url
        self.session: aiohttp.ClientSession = None
        self.test_collection_id = f"test_collection_{uuid.uuid4().hex[:8]}"
        
    async def __aenter__(self):
        self.session = aiohttp.ClientSession()
        return self
    
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self.session:
            await self.session.close()
    
    def generate_test_vector(self, dimension: int = 384) -> List[float]:
        """Generate a normalized random vector."""
        vector = [random.uniform(-1, 1) for _ in range(dimension)]
        # Normalize the vector
        magnitude = sum(x ** 2 for x in vector) ** 0.5
        return [x / magnitude for x in vector]
    
    async def test_health_check(self) -> bool:
        """Test if the server is responding."""
        try:
            async with self.session.get(f"{self.base_url}/health") as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Health check passed: {data['data']['status']}")
                    return True
                else:
                    print(f"❌ Health check failed: HTTP {response.status}")
                    return False
        except Exception as e:
            print(f"❌ Health check failed: {e}")
            return False
    
    async def test_create_collection(self) -> bool:
        """Test collection creation."""
        try:
            collection_data = {
                "name": self.test_collection_id,
                "dimension": 384,
                "distance_metric": "cosine",
                "indexing_algorithm": "hnsw"
            }
            
            async with self.session.post(
                f"{self.base_url}/collections",
                json=collection_data
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Collection created: {data['data']}")
                    return True
                else:
                    text = await response.text()
                    print(f"❌ Collection creation failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ Collection creation failed: {e}")
            traceback.print_exc()
            return False
    
    async def test_list_collections(self) -> bool:
        """Test listing collections."""
        try:
            async with self.session.get(f"{self.base_url}/collections") as response:
                if response.status == 200:
                    data = await response.json()
                    collections = data['data']
                    print(f"✅ Listed {len(collections)} collections")
                    if self.test_collection_id in collections:
                        print(f"✅ Test collection found in list")
                        return True
                    else:
                        print(f"⚠️ Test collection not found in list: {collections}")
                        return False
                else:
                    text = await response.text()
                    print(f"❌ List collections failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ List collections failed: {e}")
            return False
    
    async def test_insert_vector(self) -> str:
        """Test vector insertion and return the vector ID."""
        try:
            vector_id = f"test_vector_{uuid.uuid4().hex[:8]}"
            vector_data = {
                "id": vector_id,
                "vector": self.generate_test_vector(384),
                "metadata": {
                    "test": "true",
                    "category": "integration_test",
                    "timestamp": "2025-01-01T00:00:00Z"
                }
            }
            
            async with self.session.post(
                f"{self.base_url}/collections/{self.test_collection_id}/vectors",
                json=vector_data
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Vector inserted: {data['data']}")
                    return vector_id
                else:
                    text = await response.text()
                    print(f"❌ Vector insertion failed: HTTP {response.status} - {text}")
                    return None
        except Exception as e:
            print(f"❌ Vector insertion failed: {e}")
            traceback.print_exc()
            return None
    
    async def test_batch_insert_vectors(self) -> List[str]:
        """Test batch vector insertion."""
        try:
            batch_size = 5
            vector_ids = []
            vectors = []
            
            for i in range(batch_size):
                vector_id = f"batch_vector_{i}_{uuid.uuid4().hex[:8]}"
                vector_ids.append(vector_id)
                vectors.append({
                    "id": vector_id,
                    "vector": self.generate_test_vector(384),
                    "metadata": {
                        "batch": "true",
                        "batch_index": i,
                        "category": "batch_test"
                    }
                })
            
            async with self.session.post(
                f"{self.base_url}/collections/{self.test_collection_id}/vectors/batch",
                json=vectors
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Batch vectors inserted: {len(data['data'])} vectors")
                    return vector_ids
                else:
                    text = await response.text()
                    print(f"❌ Batch vector insertion failed: HTTP {response.status} - {text}")
                    return []
        except Exception as e:
            print(f"❌ Batch vector insertion failed: {e}")
            traceback.print_exc()
            return []
    
    async def test_get_vector(self, vector_id: str) -> bool:
        """Test getting a specific vector."""
        try:
            async with self.session.get(
                f"{self.base_url}/collections/{self.test_collection_id}/vectors/{vector_id}"
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Vector retrieved: {vector_id}")
                    return True
                elif response.status == 404:
                    print(f"⚠️ Vector not found: {vector_id}")
                    return False
                else:
                    text = await response.text()
                    print(f"❌ Get vector failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ Get vector failed: {e}")
            return False
    
    async def test_search_vectors(self) -> bool:
        """Test vector search."""
        try:
            search_data = {
                "vector": self.generate_test_vector(384),
                "k": 5,
                "filters": {},
                "include_vectors": True,
                "include_metadata": True
            }
            
            async with self.session.post(
                f"{self.base_url}/collections/{self.test_collection_id}/search",
                json=search_data
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    results = data['data']
                    print(f"✅ Search returned {len(results)} results")
                    
                    # Verify result structure
                    if results:
                        first_result = results[0]
                        required_fields = ['id', 'score']
                        for field in required_fields:
                            if field not in first_result:
                                print(f"❌ Missing field in search result: {field}")
                                return False
                        print(f"✅ Search results have correct structure")
                    
                    return True
                else:
                    text = await response.text()
                    print(f"❌ Search failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ Search failed: {e}")
            traceback.print_exc()
            return False
    
    async def test_update_vector(self, vector_id: str) -> bool:
        """Test vector update."""
        try:
            updated_data = {
                "vector": self.generate_test_vector(384),
                "metadata": {
                    "test": "true",
                    "category": "updated_test",
                    "updated": "true",
                    "timestamp": "2025-01-01T12:00:00Z"
                }
            }
            
            async with self.session.put(
                f"{self.base_url}/collections/{self.test_collection_id}/vectors/{vector_id}",
                json=updated_data
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Vector updated: {data['data']}")
                    return True
                else:
                    text = await response.text()
                    print(f"❌ Vector update failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ Vector update failed: {e}")
            return False
    
    async def test_delete_vector(self, vector_id: str) -> bool:
        """Test vector deletion."""
        try:
            async with self.session.delete(
                f"{self.base_url}/collections/{self.test_collection_id}/vectors/{vector_id}"
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Vector deleted: {data['data']}")
                    return True
                elif response.status == 404:
                    print(f"⚠️ Vector not found for deletion: {vector_id}")
                    return False
                else:
                    text = await response.text()
                    print(f"❌ Vector deletion failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ Vector deletion failed: {e}")
            return False
    
    async def test_delete_collection(self) -> bool:
        """Test collection deletion."""
        try:
            async with self.session.delete(
                f"{self.base_url}/collections/{self.test_collection_id}"
            ) as response:
                if response.status == 200:
                    data = await response.json()
                    print(f"✅ Collection deleted: {data['data']}")
                    return True
                else:
                    text = await response.text()
                    print(f"❌ Collection deletion failed: HTTP {response.status} - {text}")
                    return False
        except Exception as e:
            print(f"❌ Collection deletion failed: {e}")
            return False
    
    async def run_comprehensive_test(self) -> bool:
        """Run all vector operations tests."""
        print("🚀 Starting Vector Operations Integration Test")
        print("=" * 60)
        
        # Track test results
        results = []
        
        # 1. Health check
        print("\n1. Testing Health Check...")
        results.append(await self.test_health_check())
        
        # 2. Create collection
        print("\n2. Testing Collection Creation...")
        results.append(await self.test_create_collection())
        
        # 3. List collections
        print("\n3. Testing Collection Listing...")
        results.append(await self.test_list_collections())
        
        # 4. Insert single vector
        print("\n4. Testing Single Vector Insertion...")
        single_vector_id = await self.test_insert_vector()
        results.append(single_vector_id is not None)
        
        # 5. Batch insert vectors
        print("\n5. Testing Batch Vector Insertion...")
        batch_vector_ids = await self.test_batch_insert_vectors()
        results.append(len(batch_vector_ids) > 0)
        
        # 6. Get vector
        if single_vector_id:
            print("\n6. Testing Vector Retrieval...")
            results.append(await self.test_get_vector(single_vector_id))
        else:
            print("\n6. Skipping Vector Retrieval (no vector to test)")
            results.append(False)
        
        # 7. Search vectors
        print("\n7. Testing Vector Search...")
        results.append(await self.test_search_vectors())
        
        # 8. Update vector
        if single_vector_id:
            print("\n8. Testing Vector Update...")
            results.append(await self.test_update_vector(single_vector_id))
        else:
            print("\n8. Skipping Vector Update (no vector to test)")
            results.append(False)
        
        # 9. Delete individual vectors
        vectors_to_delete = [single_vector_id] + batch_vector_ids
        vectors_to_delete = [v for v in vectors_to_delete if v]
        
        if vectors_to_delete:
            print(f"\n9. Testing Vector Deletion ({len(vectors_to_delete)} vectors)...")
            delete_results = []
            for vector_id in vectors_to_delete:
                delete_results.append(await self.test_delete_vector(vector_id))
            results.append(any(delete_results))  # At least one deletion should succeed
        else:
            print("\n9. Skipping Vector Deletion (no vectors to delete)")
            results.append(False)
        
        # 10. Delete collection
        print("\n10. Testing Collection Deletion...")
        results.append(await self.test_delete_collection())
        
        # Summary
        print("\n" + "=" * 60)
        print("📊 TEST SUMMARY")
        print("=" * 60)
        
        test_names = [
            "Health Check",
            "Collection Creation", 
            "Collection Listing",
            "Single Vector Insertion",
            "Batch Vector Insertion",
            "Vector Retrieval",
            "Vector Search",
            "Vector Update",
            "Vector Deletion", 
            "Collection Deletion"
        ]
        
        passed = sum(results)
        total = len(results)
        
        for i, (name, result) in enumerate(zip(test_names, results)):
            status = "✅ PASS" if result else "❌ FAIL"
            print(f"{i+1:2d}. {name:<25} {status}")
        
        print("-" * 60)
        print(f"Total: {passed}/{total} tests passed ({passed/total*100:.1f}%)")
        
        if passed == total:
            print("🎉 ALL TESTS PASSED! Vector operations integration is complete.")
            return True
        else:
            print(f"⚠️ {total - passed} tests failed. Vector operations need attention.")
            return False

async def main():
    """Main test runner."""
    try:
        async with VectorOperationsTest() as test:
            success = await test.run_comprehensive_test()
            sys.exit(0 if success else 1)
    except KeyboardInterrupt:
        print("\n🛑 Test interrupted by user")
        sys.exit(1)
    except Exception as e:
        print(f"\n💥 Test failed with exception: {e}")
        traceback.print_exc()
        sys.exit(1)

if __name__ == "__main__":
    asyncio.run(main())