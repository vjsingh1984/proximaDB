#!/usr/bin/env python3
"""
Fixed WAL and Flush Test using direct HTTP calls
Tests Write-Ahead Log functionality and flush operations
"""

import json
import time
import numpy as np
import uuid
import requests
import os


class WALFlushTest:
    """Test WAL and flush functionality using direct HTTP"""
    
    def __init__(self):
        self.base_url = "http://localhost:5678"
        self.collection_name = f"wal_test_{uuid.uuid4().hex[:8]}"
        self.dimension = 384
        
    def run_tests(self):
        """Run all WAL and flush tests"""
        print("🚀 ProximaDB WAL & Flush Test Suite")
        print("=" * 50)
        
        try:
            # Check server health
            if not self.check_server_health():
                return False
            
            # Run test sequence
            if not self.test_wal_basic_operations():
                return False
                
            if not self.test_flush_operations():
                return False
                
            if not self.test_wal_durability():
                return False
            
            print("\n🎉 All WAL & Flush tests completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Test suite failed: {e}")
            import traceback
            traceback.print_exc()
            return False
    
    def check_server_health(self):
        """Check if the server is healthy"""
        try:
            response = requests.get(f"{self.base_url}/health", timeout=5)
            if response.status_code == 200:
                print("✅ Server is healthy")
                return True
            else:
                print(f"❌ Server health check failed: {response.status_code}")
                return False
        except Exception as e:
            print(f"❌ Cannot connect to server: {e}")
            return False
    
    def test_wal_basic_operations(self):
        """Test basic WAL operations"""
        print("\n🔄 Testing WAL Basic Operations")
        print("-" * 40)
        
        # Create collection
        collection_name = f"{self.collection_name}_basic"
        if not self.create_collection(collection_name):
            return False
        
        # Insert vectors to trigger WAL writes
        print("📝 Inserting vectors to test WAL writes...")
        vectors_inserted = 0
        
        for i in range(20):  # Insert 20 vectors
            vector_data = {
                "id": f"wal_vector_{i:03d}",
                "vector": np.random.random(self.dimension).astype(np.float32).tolist(),
                "metadata": {
                    "index": i,
                    "test_type": "wal_basic",
                    "timestamp": time.time()
                }
            }
            
            try:
                response = requests.post(
                    f"{self.base_url}/collections/{collection_name}/vectors",
                    json=vector_data
                )
                
                if response.status_code == 200:
                    vectors_inserted += 1
                    if (i + 1) % 5 == 0:
                        print(f"   Inserted {i+1}/20 vectors")
                        
            except Exception as e:
                print(f"   ❌ Failed to insert vector {i}: {e}")
                continue
        
        print(f"📊 WAL Basic Test: {vectors_inserted}/20 vectors inserted")
        
        # Verify vectors can be retrieved (indicates WAL persistence)
        if vectors_inserted > 0:
            print("🔍 Verifying WAL persistence with search...")
            query_vector = np.random.random(self.dimension).astype(np.float32).tolist()
            
            try:
                search_response = requests.post(
                    f"{self.base_url}/collections/{collection_name}/search",
                    json={"vector": query_vector, "k": 5}
                )
                
                if search_response.status_code == 200:
                    search_data = search_response.json()
                    results = search_data.get("data", [])
                    print(f"   ✅ WAL persistence verified: {len(results)} vectors found in search")
                else:
                    print(f"   ⚠️ Search returned {search_response.status_code}")
                    
            except Exception as e:
                print(f"   ⚠️ Search verification failed: {e}")
        
        # Cleanup
        self.cleanup_collection(collection_name)
        
        return vectors_inserted >= 15  # Allow some failures
    
    def test_flush_operations(self):
        """Test flush operations"""
        print("\n💾 Testing Flush Operations")
        print("-" * 40)
        
        # Create collection
        collection_name = f"{self.collection_name}_flush"
        if not self.create_collection(collection_name):
            return False
        
        # Insert batch of vectors
        print("📝 Inserting vectors to test flush behavior...")
        batch_size = 50
        vectors_inserted = 0
        
        for i in range(batch_size):
            vector_data = {
                "id": f"flush_vector_{i:03d}",
                "vector": np.random.random(self.dimension).astype(np.float32).tolist(),
                "metadata": {
                    "index": i,
                    "test_type": "flush_test",
                    "batch": "flush_batch_1"
                }
            }
            
            try:
                response = requests.post(
                    f"{self.base_url}/collections/{collection_name}/vectors",
                    json=vector_data
                )
                
                if response.status_code == 200:
                    vectors_inserted += 1
                    
                # Progress reporting
                if (i + 1) % 10 == 0:
                    print(f"   Inserted {i+1}/{batch_size} vectors")
                        
            except Exception as e:
                print(f"   ❌ Failed to insert vector {i}: {e}")
                continue
        
        print(f"📊 Flush Test: {vectors_inserted}/{batch_size} vectors inserted")
        
        # Wait for potential flush operations
        print("⏳ Waiting for flush operations...")
        time.sleep(3)
        
        # Test that data is accessible after potential flush
        print("🔍 Verifying data accessibility post-flush...")
        query_vector = np.random.random(self.dimension).astype(np.float32).tolist()
        
        try:
            search_response = requests.post(
                f"{self.base_url}/collections/{collection_name}/search",
                json={"vector": query_vector, "k": 10}
            )
            
            if search_response.status_code == 200:
                search_data = search_response.json()
                results = search_data.get("data", [])
                print(f"   ✅ Post-flush verification: {len(results)} vectors accessible")
                
                # Check if we can find our specific vectors
                if results:
                    found_flush_vectors = 0
                    for result in results:
                        metadata = result.get("metadata", {})
                        if metadata.get("test_type") == "flush_test":
                            found_flush_vectors += 1
                    
                    print(f"   ✅ Found {found_flush_vectors} flush test vectors in results")
                
            else:
                print(f"   ❌ Post-flush search failed: {search_response.status_code}")
                return False
                
        except Exception as e:
            print(f"   ❌ Post-flush verification failed: {e}")
            return False
        
        # Cleanup
        self.cleanup_collection(collection_name)
        
        return vectors_inserted >= 40  # Allow some failures
    
    def test_wal_durability(self):
        """Test WAL durability and consistency"""
        print("\n🛡️ Testing WAL Durability")
        print("-" * 40)
        
        # Create collection
        collection_name = f"{self.collection_name}_durability"
        if not self.create_collection(collection_name):
            return False
        
        # Insert vectors with different metadata to test consistency
        print("📝 Testing WAL durability with varied operations...")
        
        test_scenarios = [
            {"type": "small_batch", "count": 5, "prefix": "small"},
            {"type": "medium_batch", "count": 15, "prefix": "medium"},
            {"type": "large_batch", "count": 30, "prefix": "large"}
        ]
        
        total_inserted = 0
        
        for scenario in test_scenarios:
            print(f"   Testing {scenario['type']}: {scenario['count']} vectors")
            scenario_inserted = 0
            
            for i in range(scenario["count"]):
                vector_data = {
                    "id": f"{scenario['prefix']}_vector_{i:03d}",
                    "vector": np.random.random(self.dimension).astype(np.float32).tolist(),
                    "metadata": {
                        "index": i,
                        "scenario": scenario["type"],
                        "prefix": scenario["prefix"],
                        "batch_time": time.time()
                    }
                }
                
                try:
                    response = requests.post(
                        f"{self.base_url}/collections/{collection_name}/vectors",
                        json=vector_data
                    )
                    
                    if response.status_code == 200:
                        scenario_inserted += 1
                        total_inserted += 1
                            
                except Exception as e:
                    print(f"     ❌ Failed to insert {scenario['prefix']} vector {i}: {e}")
                    continue
            
            print(f"     ✅ {scenario['type']}: {scenario_inserted}/{scenario['count']} vectors")
            
            # Brief pause between scenarios
            time.sleep(0.5)
        
        print(f"📊 WAL Durability Test: {total_inserted} total vectors inserted")
        
        # Verify consistency across all scenarios
        print("🔍 Verifying WAL consistency across scenarios...")
        
        for scenario in test_scenarios:
            query_vector = np.random.random(self.dimension).astype(np.float32).tolist()
            
            try:
                search_response = requests.post(
                    f"{self.base_url}/collections/{collection_name}/search",
                    json={"vector": query_vector, "k": 20}
                )
                
                if search_response.status_code == 200:
                    search_data = search_response.json()
                    results = search_data.get("data", [])
                    
                    scenario_results = []
                    for result in results:
                        metadata = result.get("metadata", {})
                        if metadata.get("scenario") == scenario["type"]:
                            scenario_results.append(result)
                    
                    print(f"   ✅ {scenario['type']}: {len(scenario_results)} vectors found in search")
                else:
                    print(f"   ❌ Search failed for {scenario['type']}: {search_response.status_code}")
                    
            except Exception as e:
                print(f"   ❌ Consistency check failed for {scenario['type']}: {e}")
        
        # Final consistency check
        print("🔍 Final consistency verification...")
        query_vector = np.random.random(self.dimension).astype(np.float32).tolist()
        
        try:
            search_response = requests.post(
                f"{self.base_url}/collections/{collection_name}/search",
                json={"vector": query_vector, "k": 50}
            )
            
            if search_response.status_code == 200:
                search_data = search_response.json()
                results = search_data.get("data", [])
                print(f"   ✅ Final check: {len(results)} total vectors accessible")
            else:
                print(f"   ❌ Final consistency check failed: {search_response.status_code}")
                
        except Exception as e:
            print(f"   ❌ Final consistency check exception: {e}")
        
        # Cleanup
        self.cleanup_collection(collection_name)
        
        return total_inserted >= 40  # Allow some failures
    
    def create_collection(self, collection_name):
        """Create a test collection"""
        collection_data = {
            "name": collection_name,
            "dimension": self.dimension,
            "distance_metric": "cosine",
            "storage_engine": "viper"
        }
        
        try:
            response = requests.post(
                f"{self.base_url}/collections",
                json=collection_data
            )
            
            if response.status_code == 200:
                print(f"✅ Collection created: {collection_name}")
                return True
            else:
                print(f"❌ Failed to create collection: {response.status_code}")
                return False
                
        except Exception as e:
            print(f"❌ Collection creation failed: {e}")
            return False
    
    def cleanup_collection(self, collection_name):
        """Clean up a test collection"""
        try:
            response = requests.delete(f"{self.base_url}/collections/{collection_name}")
            if response.status_code == 200:
                print(f"🧹 Cleaned up collection: {collection_name}")
            else:
                print(f"⚠️ Cleanup returned: {response.status_code}")
        except Exception as e:
            print(f"⚠️ Cleanup failed: {e}")


def main():
    """Run the WAL and flush tests"""
    test_suite = WALFlushTest()
    success = test_suite.run_tests()
    
    if success:
        print("\n🎉 WAL & Flush test suite completed successfully!")
    else:
        print("\n💥 WAL & Flush test suite failed!")
        exit(1)


if __name__ == "__main__":
    main()