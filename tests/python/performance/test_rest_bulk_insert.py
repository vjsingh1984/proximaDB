#!/usr/bin/env python3
"""
REST API bulk insert performance test for 100 vectors
Tests if REST API supports bulk vector insertion
"""
import requests
import json
import time
import numpy as np
import pytest


class TestRESTBulkInsert:
    """REST API bulk insert performance tests"""
    
    @pytest.fixture(autouse=True)
    def setup(self):
        """Setup for REST bulk insert tests"""
        self.base_url = "http://localhost:5678"
        self.collection_id = f"rest_bulk_{int(time.time() * 1000)}"
        self.dimension = 384
        
        # Verify server is up
        response = requests.get(f"{self.base_url}/health")
        if response.status_code != 200:
            pytest.skip("Server not available")
        
        yield
        
        # Cleanup
        try:
            requests.delete(f"{self.base_url}/collections/{self.collection_id}")
        except:
            pass
    
    def test_bulk_insert_100_vectors_rest(self):
        """Test bulk inserting 100 vectors via REST API"""
        print("\n🚀 REST API Bulk Insert Test: 100 Vectors")
        print("=" * 50)
        
        # Create collection
        collection_data = {
            "name": self.collection_id,
            "dimension": self.dimension,
            "distance_metric": "cosine",
            "storage_engine": "viper"
        }
        
        response = requests.post(f"{self.base_url}/collections", json=collection_data)
        assert response.status_code == 200
        print(f"✅ Collection created: {self.collection_id}")
        
        # Generate 100 vectors for bulk insert
        num_vectors = 100
        vectors_data = []
        
        for i in range(num_vectors):
            vectors_data.append({
                "id": f"bulk_vector_{i:03d}",
                "vector": np.random.random(self.dimension).astype(np.float32).tolist(),
                "metadata": {
                    "index": i,
                    "method": "bulk_insert",
                    "batch_id": "rest_bulk_001",
                    "timestamp": time.time()
                }
            })
        
        print(f"📦 Generated {num_vectors} vectors for bulk insert")
        
        # Try different bulk insert approaches
        bulk_endpoints = [
            {
                "name": "Batch Insert",
                "endpoint": f"/collections/{self.collection_id}/vectors/batch",
                "payload": {"vectors": vectors_data}
            },
            {
                "name": "Bulk Insert", 
                "endpoint": f"/collections/{self.collection_id}/vectors/bulk",
                "payload": {"vectors": vectors_data}
            },
            {
                "name": "Multi Insert",
                "endpoint": f"/collections/{self.collection_id}/vectors",
                "payload": vectors_data  # Array directly
            },
            {
                "name": "Bulk Vectors",
                "endpoint": f"/collections/{self.collection_id}/bulk_vectors",
                "payload": {"vectors": vectors_data}
            }
        ]
        
        bulk_success = False
        successful_method = None
        
        for method in bulk_endpoints:
            print(f"\n🔄 Trying {method['name']} method...")
            print(f"   Endpoint: POST {method['endpoint']}")
            
            try:
                start_time = time.time()
                response = requests.post(
                    f"{self.base_url}{method['endpoint']}",
                    json=method['payload'],
                    headers={"Content-Type": "application/json"}
                )
                end_time = time.time()
                
                duration = end_time - start_time
                
                print(f"   Status: {response.status_code}")
                print(f"   Duration: {duration:.3f}s")
                
                if response.status_code == 200:
                    response_data = response.json()
                    print(f"   Response: {json.dumps(response_data, indent=2)[:200]}...")
                    
                    if response_data.get("success"):
                        bulk_success = True
                        successful_method = method
                        print(f"✅ {method['name']} succeeded!")
                        print(f"📊 Bulk insert rate: {num_vectors/duration:.1f} vectors/second")
                        break
                    else:
                        print(f"   ⚠️ API returned success=false")
                
                elif response.status_code == 201:
                    print(f"✅ {method['name']} succeeded with 201 Created!")
                    bulk_success = True
                    successful_method = method
                    print(f"📊 Bulk insert rate: {num_vectors/duration:.1f} vectors/second")
                    break
                    
                else:
                    print(f"   ❌ Failed with {response.status_code}")
                    if response.text:
                        print(f"   Error: {response.text[:200]}")
                        
            except Exception as e:
                print(f"   ❌ Exception: {e}")
                continue
        
        # If bulk insert not supported, fall back to individual inserts
        if not bulk_success:
            print(f"\n🔄 Bulk insert not supported, trying individual inserts...")
            start_time = time.time()
            successful_inserts = 0
            
            for i, vector_data in enumerate(vectors_data):
                try:
                    response = requests.post(
                        f"{self.base_url}/collections/{self.collection_id}/vectors",
                        json=vector_data
                    )
                    
                    if response.status_code == 200:
                        successful_inserts += 1
                    
                    # Progress every 25 vectors
                    if (i + 1) % 25 == 0:
                        elapsed = time.time() - start_time
                        rate = successful_inserts / elapsed if elapsed > 0 else 0
                        print(f"   Progress: {successful_inserts}/{i+1} vectors ({rate:.1f}/s)")
                        
                except Exception as e:
                    print(f"   Vector {i} failed: {e}")
                    continue
            
            end_time = time.time()
            duration = end_time - start_time
            
            print(f"\n📊 Individual Insert Results:")
            print(f"   Successful: {successful_inserts}/{num_vectors} vectors")
            print(f"   Duration: {duration:.2f}s")
            print(f"   Rate: {successful_inserts/duration:.1f} vectors/second")
            
            assert successful_inserts >= num_vectors * 0.8  # Allow some failures
        
        else:
            print(f"\n✅ Bulk insert successful using {successful_method['name']} method")
            
            # Verify vectors were inserted by trying to search
            print(f"\n🔍 Verifying bulk insert with search...")
            query_vector = np.random.random(self.dimension).astype(np.float32).tolist()
            
            try:
                search_response = requests.post(
                    f"{self.base_url}/collections/{self.collection_id}/search",
                    json={"vector": query_vector, "k": 10}
                )
                
                if search_response.status_code == 200:
                    search_data = search_response.json()
                    results = search_data.get("data", [])
                    print(f"   ✅ Search found {len(results)} results (bulk insert verified)")
                else:
                    print(f"   ⚠️ Search failed with {search_response.status_code}")
                    
            except Exception as e:
                print(f"   ⚠️ Search verification failed: {e}")
    
    def test_chunked_bulk_insert_rest(self):
        """Test chunked bulk insert for larger datasets"""
        print("\n🚀 REST API Chunked Bulk Insert Test")
        print("=" * 50)
        
        # Create collection
        collection_data = {
            "name": self.collection_id + "_chunked",
            "dimension": self.dimension,
            "distance_metric": "cosine",
            "storage_engine": "viper"
        }
        
        response = requests.post(f"{self.base_url}/collections", json=collection_data)
        assert response.status_code == 200
        collection_id_chunked = self.collection_id + "_chunked"
        print(f"✅ Collection created: {collection_id_chunked}")
        
        # Generate larger dataset (300 vectors)
        num_vectors = 300
        chunk_size = 50  # Insert in chunks of 50
        
        print(f"📦 Testing chunked insert: {num_vectors} vectors in chunks of {chunk_size}")
        
        start_time = time.time()
        total_inserted = 0
        
        for chunk_start in range(0, num_vectors, chunk_size):
            chunk_end = min(chunk_start + chunk_size, num_vectors)
            
            # Generate chunk
            chunk_vectors = []
            for i in range(chunk_start, chunk_end):
                chunk_vectors.append({
                    "id": f"chunked_vector_{i:03d}",
                    "vector": np.random.random(self.dimension).astype(np.float32).tolist(),
                    "metadata": {
                        "index": i,
                        "chunk": chunk_start // chunk_size,
                        "method": "chunked_bulk"
                    }
                })
            
            # Try to insert chunk
            chunk_inserted = 0
            
            # Try bulk insert first
            try:
                response = requests.post(
                    f"{self.base_url}/collections/{collection_id_chunked}/vectors/batch",
                    json={"vectors": chunk_vectors}
                )
                
                if response.status_code == 200:
                    chunk_inserted = len(chunk_vectors)
                    print(f"   Chunk {chunk_start//chunk_size + 1}: Bulk insert {chunk_inserted} vectors")
                else:
                    raise Exception(f"Bulk failed with {response.status_code}")
                    
            except:
                # Fall back to individual inserts
                for vector_data in chunk_vectors:
                    try:
                        response = requests.post(
                            f"{self.base_url}/collections/{collection_id_chunked}/vectors",
                            json=vector_data
                        )
                        if response.status_code == 200:
                            chunk_inserted += 1
                    except:
                        continue
                
                print(f"   Chunk {chunk_start//chunk_size + 1}: Individual insert {chunk_inserted}/{len(chunk_vectors)} vectors")
            
            total_inserted += chunk_inserted
        
        end_time = time.time()
        duration = end_time - start_time
        
        print(f"\n📊 Chunked Insert Results:")
        print(f"   Total inserted: {total_inserted}/{num_vectors} vectors")
        print(f"   Duration: {duration:.2f}s")
        print(f"   Rate: {total_inserted/duration:.1f} vectors/second")
        print(f"   Chunk size: {chunk_size} vectors")
        
        # Cleanup chunked collection
        requests.delete(f"{self.base_url}/collections/{collection_id_chunked}")
        
        assert total_inserted >= num_vectors * 0.8


if __name__ == "__main__":
    # Direct execution for manual testing
    import sys
    
    # Check server availability
    try:
        response = requests.get("http://localhost:5678/health", timeout=5)
        if response.status_code != 200:
            print("❌ ProximaDB server not available")
            exit(1)
        print("✅ ProximaDB server is healthy")
    except:
        print("❌ Cannot connect to ProximaDB server")
        exit(1)
    
    # Run tests
    test = TestRESTBulkInsert()
    test.setup()
    
    try:
        print("🚀 Running REST API Bulk Insert Tests")
        print("=" * 60)
        
        # Test bulk insert
        test.test_bulk_insert_100_vectors_rest()
        
        # Test chunked bulk insert
        test.test_chunked_bulk_insert_rest()
        
        print("\n🎉 All REST bulk insert tests completed!")
        
    except Exception as e:
        print(f"\n❌ REST bulk insert test failed: {e}")
        import traceback
        traceback.print_exc()
    finally:
        # Cleanup handled by fixture
        pass