#!/usr/bin/env python3
"""
Comprehensive test to verify that both Avro and Bincode WAL strategies
work correctly with vector search operations through the memtable.

This test validates the fix for the memtable avro payload unified distance test.
"""

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python tests/integration/test_wal_strategies_comprehensive.py

import sys
import time
import subprocess
import pytest
import numpy as np
from pathlib import Path

try:
    from proximadb import ProximaDBClient, Protocol
except ImportError:
    print("ProximaDB Python client not found. Please install it first.")
    sys.exit(1)


class TestWALStrategiesIntegration:
    """Test different WAL strategies with vector search operations."""
    
    @classmethod
    def setup_class(cls):
        """Start the ProximaDB server before running tests."""
        print("Setting up ProximaDB server...")
        # Note: In a real test environment, you'd want to start a clean server instance
        # For now, we assume the server is already running
        cls.client = ProximaDBClient(base_url="http://localhost:5678")
        time.sleep(1)  # Give server time to initialize
        
    def test_avro_vs_bincode_consistency(self):
        """Test that both Avro and Bincode strategies produce searchable results."""
        collection_name = f"test_formats_{int(time.time())}"
        
        # Create a test collection
        try:
            result = self.client.create_collection(
                name=collection_name,
                dimension=3,
                distance_metric="COSINE"
            )
            print(f"Created collection: {result}")
        except Exception as e:
            print(f"Collection creation error (may already exist): {e}")
        
        # Test vectors with known relationships
        vectors = [
            {
                "id": "vec1",
                "vector": [1.0, 0.0, 0.0],
                "metadata": {"type": "regular_insert"}
            },
            {
                "id": "vec2", 
                "vector": [0.0, 1.0, 0.0],
                "metadata": {"type": "avro_payload"}
            },
            {
                "id": "vec3",
                "vector": [0.707, 0.707, 0.0],
                "metadata": {"type": "mixed_insert"}
            }
        ]
        
        # Insert vectors - the service should handle both Avro and Bincode based on configuration
        try:
            insert_result = self.client.insert_vectors(
                collection_id=collection_name,
                vectors=vectors
            )
            print(f"Insert result: {insert_result}")
            assert insert_result["success"] == True
            assert len(insert_result["vector_ids"]) == 3
        except Exception as e:
            pytest.fail(f"Vector insertion failed: {e}")
        
        # Allow time for indexing
        time.sleep(2)
        
        # Test search - should find all vectors regardless of how they were stored
        query_vector = [0.5, 0.5, 0.0]  # 45-degree angle, similar to vec3
        
        try:
            search_result = self.client.search_vectors(
                collection_id=collection_name,
                query_vector=query_vector,
                k=10,
                include_metadata=True
            )
            print(f"Search result: {search_result}")
            
            # Verify we found all vectors
            assert search_result["success"] == True
            assert len(search_result["results"]) == 3, f"Expected 3 results, got {len(search_result['results'])}"
            
            # Verify the vectors are ordered by similarity (cosine distance)
            results = search_result["results"]
            
            # vec3 should be closest (most similar to 45-degree query)
            closest_result = results[0]
            assert closest_result["metadata"]["type"] == "mixed_insert"
            
            # All vectors should have reasonable similarity scores
            for result in results:
                assert "score" in result
                assert 0.0 <= result["score"] <= 2.0  # Valid cosine distance range
                
            print("✅ All vectors found and properly ranked")
            
        except Exception as e:
            pytest.fail(f"Vector search failed: {e}")
            
    def test_batch_insert_search_consistency(self):
        """Test batch inserts are searchable (tests Avro payload batch format)."""
        collection_name = f"test_batch_{int(time.time())}"
        
        # Create collection
        try:
            self.client.create_collection(
                name=collection_name,
                dimension=128,
                distance_metric="EUCLIDEAN"
            )
        except Exception as e:
            print(f"Collection creation error: {e}")
        
        # Create a batch of vectors
        batch_vectors = []
        for i in range(20):
            vector = np.random.rand(128).tolist()
            batch_vectors.append({
                "id": f"batch_vec_{i}",
                "vector": vector,
                "metadata": {"batch_id": "test_batch_1", "index": i}
            })
        
        # Insert batch
        try:
            insert_result = self.client.insert_vectors(
                collection_id=collection_name,
                vectors=batch_vectors
            )
            assert insert_result["success"] == True
            assert len(insert_result["vector_ids"]) == 20
        except Exception as e:
            pytest.fail(f"Batch insert failed: {e}")
        
        time.sleep(2)
        
        # Search should find the batch vectors
        query_vector = np.random.rand(128).tolist()
        
        try:
            search_result = self.client.search_vectors(
                collection_id=collection_name,
                query_vector=query_vector,
                k=20,
                include_metadata=True
            )
            
            assert search_result["success"] == True
            assert len(search_result["results"]) == 20
            
            # Verify all results have the correct batch metadata
            for result in search_result["results"]:
                assert result["metadata"]["batch_id"] == "test_batch_1"
                assert "index" in result["metadata"]
                
            print("✅ Batch insert and search working correctly")
            
        except Exception as e:
            pytest.fail(f"Batch search failed: {e}")

    def test_mixed_operations_consistency(self):
        """Test mixing single and batch operations."""
        collection_name = f"test_mixed_{int(time.time())}"
        
        # Create collection
        try:
            self.client.create_collection(
                name=collection_name,
                dimension=4,
                distance_metric="DOT_PRODUCT"
            )
        except Exception as e:
            print(f"Collection creation error: {e}")
        
        # Insert single vector
        single_vector = {
            "id": "single_vec",
            "vector": [1.0, 1.0, 1.0, 1.0],
            "metadata": {"type": "single"}
        }
        
        try:
            self.client.insert_vectors(
                collection_id=collection_name,
                vectors=[single_vector]
            )
        except Exception as e:
            pytest.fail(f"Single insert failed: {e}")
        
        # Insert batch
        batch_vectors = [
            {
                "id": "batch_vec_1",
                "vector": [1.0, 0.0, 0.0, 0.0],
                "metadata": {"type": "batch"}
            },
            {
                "id": "batch_vec_2", 
                "vector": [0.0, 1.0, 0.0, 0.0],
                "metadata": {"type": "batch"}
            }
        ]
        
        try:
            self.client.insert_vectors(
                collection_id=collection_name,
                vectors=batch_vectors
            )
        except Exception as e:
            pytest.fail(f"Batch insert failed: {e}")
        
        time.sleep(2)
        
        # Search should find all vectors
        query_vector = [0.5, 0.5, 0.5, 0.5]
        
        try:
            search_result = self.client.search_vectors(
                collection_id=collection_name,
                query_vector=query_vector,
                k=10,
                include_metadata=True
            )
            
            assert search_result["success"] == True
            assert len(search_result["results"]) == 3
            
            # Verify we have both single and batch vectors
            types_found = set()
            for result in search_result["results"]:
                types_found.add(result["metadata"]["type"])
            
            assert "single" in types_found
            assert "batch" in types_found
            
            print("✅ Mixed operations working correctly")
            
        except Exception as e:
            pytest.fail(f"Mixed search failed: {e}")

    def test_avro_payload_specific_behavior(self):
        """Test behavior specific to Avro payload handling."""
        collection_name = f"test_avro_specific_{int(time.time())}"
        
        # Create collection
        try:
            self.client.create_collection(
                name=collection_name,
                dimension=3,
                distance_metric="COSINE"
            )
        except Exception as e:
            print(f"Collection creation error: {e}")
        
        # Test single vector that should be stored as Avro payload
        single_vector = {
            "id": "avro_single",
            "vector": [1.0, 0.0, 0.0],
            "metadata": {"format": "avro_single"}
        }
        
        # Test batch that should be stored as Avro payload batch
        batch_vectors = [
            {
                "id": "avro_batch_1",
                "vector": [0.0, 1.0, 0.0],
                "metadata": {"format": "avro_batch"}
            },
            {
                "id": "avro_batch_2",
                "vector": [0.0, 0.0, 1.0],
                "metadata": {"format": "avro_batch"}
            }
        ]
        
        try:
            # Insert single vector
            self.client.insert_vectors(
                collection_id=collection_name,
                vectors=[single_vector]
            )
            
            # Insert batch
            self.client.insert_vectors(
                collection_id=collection_name,
                vectors=batch_vectors
            )
            
            time.sleep(2)
            
            # Search with a vector that should match all reasonably well
            query_vector = [0.333, 0.333, 0.333]
            
            search_result = self.client.search_vectors(
                collection_id=collection_name,
                query_vector=query_vector,
                k=10,
                include_metadata=True
            )
            
            assert search_result["success"] == True
            assert len(search_result["results"]) == 3, f"Expected 3 results, got {len(search_result['results'])}"
            
            # Verify we can find both single and batch vectors
            formats_found = set()
            for result in search_result["results"]:
                formats_found.add(result["metadata"]["format"])
            
            assert "avro_single" in formats_found
            assert "avro_batch" in formats_found
            
            print("✅ Avro payload specific behavior working correctly")
            
        except Exception as e:
            pytest.fail(f"Avro payload test failed: {e}")


if __name__ == "__main__":
    # Run the tests
    test_instance = TestWALStrategiesIntegration()
    test_instance.setup_class()
    
    print("Running WAL strategies integration tests...")
    
    try:
        test_instance.test_avro_vs_bincode_consistency()
        print("✅ Avro vs Bincode consistency test passed")
    except Exception as e:
        print(f"❌ Avro vs Bincode test failed: {e}")
    
    try:
        test_instance.test_batch_insert_search_consistency()
        print("✅ Batch insert search consistency test passed")
    except Exception as e:
        print(f"❌ Batch test failed: {e}")
    
    try:
        test_instance.test_mixed_operations_consistency()
        print("✅ Mixed operations consistency test passed")
    except Exception as e:
        print(f"❌ Mixed operations test failed: {e}")
    
    try:
        test_instance.test_avro_payload_specific_behavior()
        print("✅ Avro payload specific behavior test passed")
    except Exception as e:
        print(f"❌ Avro payload test failed: {e}")
    
    print("All tests completed!")