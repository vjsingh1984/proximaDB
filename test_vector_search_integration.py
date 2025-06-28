#!/usr/bin/env python3
"""
Integration test for ProximaDB vector search functionality
Tests the complete pipeline: insert -> search -> results
"""

import time
import asyncio
import aiohttp
import json
import random

async def test_vector_search_integration():
    """Test complete vector search integration"""
    base_url = "http://localhost:5678"
    
    print("🧪 Starting ProximaDB Vector Search Integration Test")
    
    # Wait for server to be ready
    print("⏳ Waiting for server to be ready...")
    for i in range(30):
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(f"{base_url}/health") as response:
                    if response.status == 200:
                        print("✅ Server is ready!")
                        break
        except:
            pass
        await asyncio.sleep(1)
        if i == 29:
            print("❌ Server failed to start within 30 seconds")
            return False
    
    async with aiohttp.ClientSession() as session:
        # Step 1: Create a test collection
        collection_name = "test_vectors"
        collection_data = {
            "name": collection_name,
            "dimension": 3,
            "distance_metric": "cosine",
            "description": "Test collection for vector search"
        }
        
        print(f"📝 Creating collection: {collection_name}")
        async with session.post(f"{base_url}/collections", json=collection_data) as response:
            if response.status != 201:
                print(f"❌ Failed to create collection: {response.status}")
                text = await response.text()
                print(f"Response: {text}")
                return False
            
            result = await response.json()
            print(f"✅ Collection created: {result}")
        
        # Step 2: Insert test vectors
        test_vectors = [
            {"id": "vec1", "vector": [1.0, 0.0, 0.0], "metadata": {"label": "red"}},
            {"id": "vec2", "vector": [0.0, 1.0, 0.0], "metadata": {"label": "green"}},
            {"id": "vec3", "vector": [0.0, 0.0, 1.0], "metadata": {"label": "blue"}},
            {"id": "vec4", "vector": [0.707, 0.707, 0.0], "metadata": {"label": "yellow"}},
            {"id": "vec5", "vector": [0.577, 0.577, 0.577], "metadata": {"label": "gray"}},
        ]
        
        print(f"📊 Inserting {len(test_vectors)} test vectors...")
        for vector in test_vectors:
            vector_data = {
                "vectors": [{
                    "id": vector["id"],
                    "vector": vector["vector"],
                    "metadata": vector["metadata"]
                }]
            }
            
            async with session.post(f"{base_url}/collections/{collection_name}/vectors", json=vector_data) as response:
                if response.status != 200:
                    print(f"❌ Failed to insert vector {vector['id']}: {response.status}")
                    text = await response.text()
                    print(f"Response: {text}")
                    return False
                
                result = await response.json()
                print(f"✅ Vector {vector['id']} inserted: {result.get('success', False)}")
        
        # Step 3: Wait a bit for vectors to be processed
        print("⏳ Waiting for vectors to be processed...")
        await asyncio.sleep(2)
        
        # Step 4: Search for similar vectors
        search_queries = [
            {"vector": [1.0, 0.0, 0.0], "expected": "vec1", "label": "exact match for red"},
            {"vector": [0.9, 0.1, 0.0], "expected": "vec1", "label": "close to red"},
            {"vector": [0.0, 0.9, 0.1], "expected": "vec2", "label": "close to green"},
            {"vector": [0.5, 0.5, 0.0], "expected": "vec4", "label": "similar to yellow"},
        ]
        
        print("🔍 Testing vector search...")
        all_searches_passed = True
        
        for i, query in enumerate(search_queries):
            search_data = {
                "vector": query["vector"],
                "k": 3,
                "include_vectors": True,
                "include_metadata": True
            }
            
            print(f"🔍 Search {i+1}: {query['label']}")
            async with session.post(f"{base_url}/collections/{collection_name}/search", json=search_data) as response:
                if response.status != 200:
                    print(f"❌ Search failed: {response.status}")
                    text = await response.text()
                    print(f"Response: {text}")
                    all_searches_passed = False
                    continue
                
                result = await response.json()
                print(f"📊 Search response: {json.dumps(result, indent=2)}")
                
                # Check if we got results
                if "results" in result and len(result["results"]) > 0:
                    top_result = result["results"][0]
                    top_id = top_result.get("id", "unknown")
                    score = top_result.get("score", 0.0)
                    
                    print(f"✅ Top result: {top_id} (score: {score:.3f})")
                    
                    # Check if expected result is in top results
                    result_ids = [r.get("id", "") for r in result["results"]]
                    if query["expected"] in result_ids:
                        print(f"✅ Expected vector {query['expected']} found in results")
                    else:
                        print(f"⚠️ Expected vector {query['expected']} not in top results: {result_ids}")
                        all_searches_passed = False
                else:
                    print("❌ No search results returned")
                    all_searches_passed = False
        
        # Step 5: Test collection listing
        print("📋 Testing collection listing...")
        async with session.get(f"{base_url}/collections") as response:
            if response.status == 200:
                collections = await response.json()
                collection_names = [c.get("name", "") for c in collections.get("collections", [])]
                if collection_name in collection_names:
                    print(f"✅ Collection {collection_name} found in list")
                else:
                    print(f"⚠️ Collection {collection_name} not found in list: {collection_names}")
            else:
                print(f"❌ Failed to list collections: {response.status}")
        
        # Step 6: Clean up - delete collection
        print(f"🧹 Cleaning up collection: {collection_name}")
        async with session.delete(f"{base_url}/collections/{collection_name}") as response:
            if response.status == 200:
                print("✅ Collection deleted successfully")
            else:
                print(f"⚠️ Failed to delete collection: {response.status}")
        
        if all_searches_passed:
            print("🎉 All vector search tests passed!")
            return True
        else:
            print("❌ Some vector search tests failed")
            return False

async def main():
    """Main test runner"""
    success = await test_vector_search_integration()
    exit(0 if success else 1)

if __name__ == "__main__":
    asyncio.run(main())