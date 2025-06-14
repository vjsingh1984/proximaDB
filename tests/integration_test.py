#!/usr/bin/env python3
"""
ProximaDB Integration Test
Tests the client SDK against a running ProximaDB server
"""

import os
import sys
import subprocess
import time
import requests
import numpy as np
from typing import List, Dict, Any

# Add client library to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'clients', 'python', 'src'))

from proximadb import ProximaDBClient, connect
from proximadb.exceptions import ProximaDBError
from proximadb.models import CollectionConfig


class IntegrationTest:
    def __init__(self, server_url: str = "http://localhost:5678"):
        self.server_url = server_url
        self.client = None
        self.test_collection_id = None
        
    def setUp(self):
        """Set up test environment"""
        print(f"Setting up integration test with server: {self.server_url}")
        
        # Check if server is running
        if not self.wait_for_server():
            raise RuntimeError("ProximaDB server is not responding")
        
        # Initialize client
        from proximadb.config import ClientConfig
        config = ClientConfig(url=self.server_url, enable_http2=False)
        self.client = ProximaDBClient(config=config)
        print("✓ Connected to ProximaDB server")
        
    def wait_for_server(self, timeout: int = 30) -> bool:
        """Wait for server to be ready"""
        print("Waiting for ProximaDB server to be ready...")
        
        for i in range(timeout):
            try:
                response = requests.get(f"{self.server_url}/health", timeout=2)
                if response.status_code == 200:
                    print(f"✓ Server ready after {i} seconds")
                    return True
            except requests.exceptions.RequestException:
                pass
            
            time.sleep(1)
            print(f"  Waiting... ({i+1}/{timeout})")
        
        return False
    
    def test_health_check(self):
        """Test server health endpoint"""
        print("\n--- Testing Health Check ---")
        try:
            health = self.client.health()
            print(f"✓ Health check passed: {health}")
            assert health.status == "healthy"
        except Exception as e:
            print(f"✗ Health check failed: {e}")
            raise
    
    def test_create_collection(self):
        """Test collection creation"""
        print("\n--- Testing Collection Creation ---")
        try:
            # Create collection using the client
            config = CollectionConfig(
                dimension=128,
                distance_metric="cosine"
            )
            
            collection = self.client.create_collection(
                name="test_collection_integration",
                config=config
            )
            
            self.test_collection_id = collection.id
            print(f"✓ Collection created: {collection.id}")
            
            # Test getting collection using client
            retrieved = self.client.get_collection(collection.id)
            assert retrieved.id == collection.id
            print(f"✓ Collection retrieved: {retrieved.name}")
            
        except Exception as e:
            print(f"✗ Collection creation failed: {e}")
            raise
    
    def test_insert_vectors(self):
        """Test vector insertion"""
        print("\n--- Testing Vector Insertion ---")
        if not self.test_collection_id:
            raise RuntimeError("No test collection available")
        
        try:
            # Generate test vectors
            vectors = np.random.random((10, 128)).astype(np.float32)
            ids = [f"vec_{i}" for i in range(10)]
            metadata = [{"index": i, "type": "test"} for i in range(10)]
            
            # Insert vectors
            result = self.client.insert_vectors(
                collection_id=self.test_collection_id,
                vectors=vectors,
                ids=ids,
                metadata=metadata
            )
            
            print(f"✓ Inserted {result.successful_count} vectors")
            assert result.successful_count == 10
            
        except Exception as e:
            print(f"✗ Vector insertion failed: {e}")
            raise
    
    def test_search_vectors(self):
        """Test vector search"""
        print("\n--- Testing Vector Search ---")
        if not self.test_collection_id:
            raise RuntimeError("No test collection available")
        
        try:
            # Generate query vector
            query = np.random.random(128).astype(np.float32)
            
            # Search for similar vectors
            results = self.client.search(
                collection_id=self.test_collection_id,
                query=query,
                k=5,
                include_metadata=True
            )
            
            print(f"✓ Search returned {len(results)} results")
            assert len(results) <= 5
            
            for i, result in enumerate(results):
                print(f"  Result {i+1}: ID={result.id}, Score={result.score:.4f}")
            
        except Exception as e:
            print(f"✗ Vector search failed: {e}")
            raise
    
    def test_get_collection_stats(self):
        """Test collection statistics"""
        print("\n--- Testing Collection Stats ---")
        if not self.test_collection_id:
            raise RuntimeError("No test collection available")
        
        try:
            stats = self.client.get_collection_stats(self.test_collection_id)
            print(f"✓ Collection stats: {stats.vector_count} vectors, {stats.dimension}D")
            assert stats.vector_count == 10
            assert stats.dimension == 128
            
        except Exception as e:
            print(f"✗ Collection stats failed: {e}")
            raise
    
    def test_delete_vector(self):
        """Test vector deletion"""
        print("\n--- Testing Vector Deletion ---")
        if not self.test_collection_id:
            raise RuntimeError("No test collection available")
        
        try:
            # Delete a single vector
            result = self.client.delete_vector(
                collection_id=self.test_collection_id,
                vector_id="vec_0"
            )
            
            print(f"✓ Deleted vector: {result.deleted}")
            assert result.deleted
            
        except Exception as e:
            print(f"✗ Vector deletion failed: {e}")
            raise
    
    def tearDown(self):
        """Clean up test environment"""
        print("\n--- Cleaning Up ---")
        
        try:
            if self.test_collection_id and self.client:
                deleted = self.client.delete_collection(self.test_collection_id)
                if deleted:
                    print("✓ Test collection deleted")
                else:
                    print("⚠ Failed to delete test collection")
        except Exception as e:
            print(f"⚠ Cleanup error: {e}")
        
        if self.client:
            self.client.close()
            print("✓ Client connection closed")
    
    def run_all_tests(self):
        """Run all integration tests"""
        print("=" * 60)
        print("ProximaDB Integration Test Suite")
        print("=" * 60)
        
        try:
            self.setUp()
            
            # Run tests in order
            self.test_health_check()
            self.test_create_collection()
            self.test_insert_vectors()
            self.test_search_vectors()
            self.test_get_collection_stats()
            self.test_delete_vector()
            
            print("\n" + "=" * 60)
            print("✓ ALL TESTS PASSED!")
            print("=" * 60)
            return True
            
        except Exception as e:
            print(f"\n✗ TEST FAILED: {e}")
            return False
        
        finally:
            self.tearDown()


def run_server_integration_test():
    """Run integration test against live server"""
    server_url = os.environ.get("PROXIMADB_URL", "http://localhost:5678")
    
    test = IntegrationTest(server_url)
    success = test.run_all_tests()
    
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    run_server_integration_test()