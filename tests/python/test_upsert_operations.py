#!/usr/bin/env python3
"""
Python SDK Tests for Upsert Operations via gRPC and REST

Tests comprehensive upsert scenarios across both protocols:
- Basic upsert operations  
- ID conflict resolution (multiple updates to same vector)
- Cross-tier deduplication (unflushed -> flushed -> compacted)
- Metadata filtering with upserts
- Batch upsert operations
- Performance testing
"""

import pytest
import asyncio
import time
import json
import numpy as np
from typing import List, Dict, Any, Optional
import requests
import grpc
from concurrent.futures import ThreadPoolExecutor

# Import ProximaDB Python SDK (adjust import based on your SDK structure)
try:
    from proximadb.rest_client import ProximaDBRestClient
    from proximadb.grpc_client import ProximaDBGrpcClient
    from proximadb.types import VectorRecord, SearchRequest, UpsertRequest
except ImportError:
    # Mock imports for testing - replace with actual SDK imports
    class ProximaDBRestClient:
        def __init__(self, base_url: str): pass
    class ProximaDBGrpcClient:
        def __init__(self, host: str, port: int): pass
    class VectorRecord: pass
    class SearchRequest: pass
    class UpsertRequest: pass

# Test configuration
TEST_CONFIG = {
    "rest_base_url": "http://localhost:5678",
    "grpc_host": "localhost", 
    "grpc_port": 5679,
    "test_collection": "test_upsert_collection",
    "vector_dimension": 3,
}

class UpsertTestFramework:
    """Unified test framework for both REST and gRPC protocols"""
    
    def __init__(self, protocol: str = "rest"):
        self.protocol = protocol
        if protocol == "rest":
            self.client = ProximaDBRestClient(TEST_CONFIG["rest_base_url"])
        else:
            self.client = ProximaDBGrpcClient(TEST_CONFIG["grpc_host"], TEST_CONFIG["grpc_port"])
        
        self.collection_id = None
        
    async def setup_collection(self, collection_name: str, storage_engine: str = "WAL") -> str:
        """Create test collection and return collection ID"""
        collection_config = {
            "name": collection_name,
            "dimension": TEST_CONFIG["vector_dimension"],
            "distance_metric": "cosine",
            "storage_engine": storage_engine,
            "indexing_algorithm": "hnsw"
        }
        
        if self.protocol == "rest":
            response = requests.post(
                f"{TEST_CONFIG['rest_base_url']}/collections",
                json=collection_config
            )
            assert response.status_code == 200, f"Failed to create collection: {response.text}"
            self.collection_id = response.json()["data"]
        else:
            # gRPC collection creation
            result = await self.client.create_collection(collection_config)
            assert result.success, f"Failed to create collection: {result.error}"
            self.collection_id = collection_name
            
        return self.collection_id
    
    async def upsert_vectors(self, vectors: List[Dict[str, Any]], upsert_mode: bool = True) -> Dict[str, Any]:
        """Upsert vectors via REST or gRPC"""
        if self.protocol == "rest":
            # REST bulk upsert
            if len(vectors) == 1:
                # Single vector upsert
                vector = vectors[0]
                endpoint = f"/collections/{self.collection_id}/vectors"
                if upsert_mode and "id" in vector:
                    # Use PUT for update/upsert
                    endpoint = f"/collections/{self.collection_id}/vectors/{vector['id']}"
                    response = requests.put(f"{TEST_CONFIG['rest_base_url']}{endpoint}", json=vector)
                else:
                    # Use POST for insert
                    response = requests.post(f"{TEST_CONFIG['rest_base_url']}{endpoint}", json=vector)
            else:
                # Batch upsert
                endpoint = f"/collections/{self.collection_id}/vectors/batch"
                response = requests.post(f"{TEST_CONFIG['rest_base_url']}{endpoint}", json=vectors)
                
            assert response.status_code == 200, f"Upsert failed: {response.text}"
            return response.json()
        else:
            # gRPC upsert
            upsert_request = UpsertRequest(
                collection_id=self.collection_id,
                vectors=vectors,
                upsert_mode=upsert_mode
            )
            result = await self.client.upsert_vectors(upsert_request)
            assert result.success, f"Upsert failed: {result.error}"
            return {"success": True, "data": result.data}
    
    async def search_vectors(self, query_vector: List[float], k: int = 10, 
                           filters: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """Search vectors via REST or gRPC"""
        search_request = {
            "vector": query_vector,
            "k": k,
            "filters": filters or {},
            "include_vectors": True,
            "include_metadata": True
        }
        
        if self.protocol == "rest":
            response = requests.post(
                f"{TEST_CONFIG['rest_base_url']}/collections/{self.collection_id}/search",
                json=search_request
            )
            assert response.status_code == 200, f"Search failed: {response.text}"
            return response.json()
        else:
            search_req = SearchRequest(
                collection_id=self.collection_id,
                vector=query_vector,
                k=k,
                filters=filters or {},
                include_vectors=True,
                include_metadata=True
            )
            result = await self.client.search_vectors(search_req)
            assert result.success, f"Search failed: {result.error}"
            return {"success": True, "data": {"results": result.results}}
    
    async def force_flush(self) -> None:
        """Force flush collection to test cross-tier scenarios"""
        if self.protocol == "rest":
            response = requests.post(
                f"{TEST_CONFIG['rest_base_url']}/collections/{self.collection_id}/internal/flush"
            )
            assert response.status_code == 200, f"Flush failed: {response.text}"
        else:
            result = await self.client.force_flush_collection(self.collection_id)
            assert result.success, f"Flush failed: {result.error}"
    
    async def cleanup_collection(self) -> None:
        """Clean up test collection"""
        if self.collection_id:
            if self.protocol == "rest":
                requests.delete(f"{TEST_CONFIG['rest_base_url']}/collections/{self.collection_id}")
            else:
                await self.client.delete_collection(self.collection_id)

@pytest.fixture(params=["rest", "grpc"])
async def test_framework(request):
    """Fixture providing test framework for both protocols"""
    framework = UpsertTestFramework(request.param)
    yield framework
    await framework.cleanup_collection()

class TestBasicUpsertOperations:
    """Test basic upsert functionality"""
    
    @pytest.mark.asyncio
    async def test_basic_vector_upsert(self, test_framework):
        """Test basic vector insert and upsert operations"""
        await test_framework.setup_collection("test_basic_upsert")
        
        # Initial insert
        vectors = [{
            "id": "user_123",
            "vector": [0.1, 0.2, 0.3],
            "metadata": {"version": "v1", "category": "user"}
        }]
        
        result = await test_framework.upsert_vectors(vectors, upsert_mode=False)
        assert result["success"], "Initial insert should succeed"
        
        # Search to verify insert
        search_result = await test_framework.search_vectors([0.1, 0.2, 0.3])
        results = search_result["data"]["results"]
        assert len(results) == 1, "Should find one result"
        assert results[0]["id"] == "user_123"
        assert results[0]["metadata"]["version"] == "v1"
        
        # Update via upsert
        updated_vectors = [{
            "id": "user_123",
            "vector": [0.11, 0.21, 0.31],
            "metadata": {"version": "v2", "category": "user", "updated": True}
        }]
        
        result = await test_framework.upsert_vectors(updated_vectors, upsert_mode=True)
        assert result["success"], "Upsert should succeed"
        
        # Search to verify upsert
        search_result = await test_framework.search_vectors([0.11, 0.21, 0.31])
        results = search_result["data"]["results"]
        assert len(results) == 1, "Should still find one result (deduplicated)"
        assert results[0]["id"] == "user_123"
        assert results[0]["metadata"]["version"] == "v2", "Should have updated metadata"
        assert results[0]["metadata"]["updated"] == True
        
        # Verify vector values updated
        vector = results[0]["vector"]
        assert abs(vector[0] - 0.11) < 0.001
        assert abs(vector[1] - 0.21) < 0.001
        assert abs(vector[2] - 0.31) < 0.001

class TestIDConflictResolution:
    """Test ID conflict resolution scenarios"""
    
    @pytest.mark.asyncio
    async def test_multiple_upserts_same_id(self, test_framework):
        """Test multiple upserts to the same vector ID"""
        await test_framework.setup_collection("test_id_conflicts")
        
        base_vector = {
            "id": "conflict_test",
            "vector": [1.0, 2.0, 3.0],
            "metadata": {"iteration": 0}
        }
        
        # Initial insert
        await test_framework.upsert_vectors([base_vector], upsert_mode=False)
        
        # Multiple updates
        num_updates = 5
        for i in range(1, num_updates + 1):
            updated_vector = {
                "id": "conflict_test", 
                "vector": [1.0 + i * 0.1, 2.0 + i * 0.1, 3.0 + i * 0.1],
                "metadata": {"iteration": i, "timestamp": time.time()}
            }
            
            result = await test_framework.upsert_vectors([updated_vector], upsert_mode=True)
            assert result["success"], f"Update {i} should succeed"
            
            # Small delay to ensure timestamp ordering
            await asyncio.sleep(0.01)
        
        # Verify only latest version exists
        search_result = await test_framework.search_vectors([1.0, 2.0, 3.0])
        results = search_result["data"]["results"]
        
        assert len(results) == 1, "Should find only one result (deduplicated)"
        assert results[0]["id"] == "conflict_test"
        assert results[0]["metadata"]["iteration"] == num_updates, "Should have latest iteration"
        
        # Verify vector values are from latest update
        vector = results[0]["vector"]
        expected_value = 1.0 + num_updates * 0.1
        assert abs(vector[0] - expected_value) < 0.001, "Vector should be latest version"

    @pytest.mark.asyncio
    async def test_concurrent_upserts(self, test_framework):
        """Test concurrent upserts to same ID"""
        await test_framework.setup_collection("test_concurrent")
        
        async def perform_upsert(iteration: int):
            vector = {
                "id": "concurrent_test",
                "vector": [float(iteration), float(iteration + 1), float(iteration + 2)],
                "metadata": {"worker": iteration, "timestamp": time.time()}
            }
            return await test_framework.upsert_vectors([vector], upsert_mode=True)
        
        # Perform concurrent upserts
        num_workers = 10
        tasks = [perform_upsert(i) for i in range(num_workers)]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        
        # Verify all upserts succeeded (or failed gracefully)
        successful_upserts = sum(1 for r in results if isinstance(r, dict) and r.get("success"))
        assert successful_upserts > 0, "At least some upserts should succeed"
        
        # Verify only one final result exists
        search_result = await test_framework.search_vectors([1.0, 2.0, 3.0])
        results = search_result["data"]["results"]
        
        assert len(results) == 1, "Should find only one result despite concurrent upserts"
        assert results[0]["id"] == "concurrent_test"

class TestCrossTierDeduplication:
    """Test deduplication across storage tiers"""
    
    @pytest.mark.asyncio
    async def test_unflushed_flushed_deduplication(self, test_framework):
        """Test deduplication between unflushed and flushed data"""
        await test_framework.setup_collection("test_cross_tier", storage_engine="LSM")
        
        # Insert initial vector
        initial_vector = {
            "id": "tier_test",
            "vector": [0.5, 0.6, 0.7],
            "metadata": {"tier": "initial", "version": 1}
        }
        
        await test_framework.upsert_vectors([initial_vector], upsert_mode=False)
        
        # Force flush to move to flushed tier
        await test_framework.force_flush()
        await asyncio.sleep(0.1)  # Allow flush to complete
        
        # Insert updated vector (will be in unflushed tier)
        updated_vector = {
            "id": "tier_test",
            "vector": [0.51, 0.61, 0.71],
            "metadata": {"tier": "updated", "version": 2}
        }
        
        await test_framework.upsert_vectors([updated_vector], upsert_mode=True)
        
        # Search should return the unflushed (latest) version
        search_result = await test_framework.search_vectors([0.5, 0.6, 0.7])
        results = search_result["data"]["results"]
        
        assert len(results) == 1, "Should find only one result (cross-tier deduplicated)"
        assert results[0]["id"] == "tier_test"
        assert results[0]["metadata"]["tier"] == "updated", "Should return unflushed version"
        assert results[0]["metadata"]["version"] == 2
        
        # Verify vector values are from updated version
        vector = results[0]["vector"]
        assert abs(vector[0] - 0.51) < 0.001
        assert abs(vector[1] - 0.61) < 0.001
        assert abs(vector[2] - 0.71) < 0.001

class TestMetadataFiltering:
    """Test metadata filtering with upserts"""
    
    @pytest.mark.asyncio
    async def test_upsert_with_metadata_filters(self, test_framework):
        """Test metadata filtering during upsert search"""
        await test_framework.setup_collection("test_metadata_filter")
        
        # Insert vectors with different metadata
        vectors = [
            {
                "id": "doc_1",
                "vector": [0.1, 0.2, 0.3],
                "metadata": {"category": "important", "status": "active", "author": "alice"}
            },
            {
                "id": "doc_2", 
                "vector": [0.4, 0.5, 0.6],
                "metadata": {"category": "normal", "status": "active", "author": "bob"}
            },
            {
                "id": "doc_3",
                "vector": [0.7, 0.8, 0.9],
                "metadata": {"category": "important", "status": "inactive", "author": "charlie"}
            }
        ]
        
        await test_framework.upsert_vectors(vectors, upsert_mode=False)
        
        # Update doc_1 to change status
        updated_doc1 = {
            "id": "doc_1",
            "vector": [0.11, 0.21, 0.31],
            "metadata": {"category": "important", "status": "updated", "author": "alice"}
        }
        
        await test_framework.upsert_vectors([updated_doc1], upsert_mode=True)
        
        # Search with metadata filter for important + active status
        search_result = await test_framework.search_vectors(
            [0.1, 0.2, 0.3],
            filters={"category": "important", "status": "updated"}
        )
        results = search_result["data"]["results"]
        
        assert len(results) == 1, "Should find only doc_1 with updated status"
        assert results[0]["id"] == "doc_1"
        assert results[0]["metadata"]["status"] == "updated"
        
        # Verify vector values are from updated version
        vector = results[0]["vector"]
        assert abs(vector[0] - 0.11) < 0.001

class TestBatchUpserts:
    """Test batch upsert operations"""
    
    @pytest.mark.asyncio
    async def test_mixed_batch_upserts(self, test_framework):
        """Test batch with mix of new and existing vectors"""
        await test_framework.setup_collection("test_batch_mixed")
        
        # Initial batch
        initial_vectors = [
            {"id": "item_1", "vector": [1.0, 1.0, 1.0], "metadata": {"version": 1}},
            {"id": "item_2", "vector": [2.0, 2.0, 2.0], "metadata": {"version": 1}},
        ]
        
        await test_framework.upsert_vectors(initial_vectors, upsert_mode=False)
        
        # Mixed batch: update existing + add new
        mixed_batch = [
            {"id": "item_1", "vector": [1.1, 1.1, 1.1], "metadata": {"version": 2, "updated": True}},  # Update
            {"id": "item_3", "vector": [3.0, 3.0, 3.0], "metadata": {"version": 1, "new": True}},      # New
        ]
        
        await test_framework.upsert_vectors(mixed_batch, upsert_mode=True)
        
        # Verify results
        search_result = await test_framework.search_vectors([1.0, 1.0, 1.0])
        results = search_result["data"]["results"]
        
        assert len(results) == 3, "Should find all three items"
        
        # Check item_1 was updated
        item_1 = next(r for r in results if r["id"] == "item_1")
        assert item_1["metadata"]["version"] == 2
        assert item_1["metadata"]["updated"] == True
        assert abs(item_1["vector"][0] - 1.1) < 0.001
        
        # Check item_2 unchanged
        item_2 = next(r for r in results if r["id"] == "item_2")
        assert item_2["metadata"]["version"] == 1
        assert abs(item_2["vector"][0] - 2.0) < 0.001
        
        # Check item_3 was added
        item_3 = next(r for r in results if r["id"] == "item_3")
        assert item_3["metadata"]["new"] == True

class TestPerformance:
    """Performance tests for upsert operations"""
    
    @pytest.mark.asyncio
    async def test_bulk_upsert_performance(self, test_framework):
        """Test performance of bulk upsert operations"""
        await test_framework.setup_collection("test_bulk_performance")
        
        num_vectors = 1000
        batch_size = 100
        
        # Generate test vectors
        def generate_vectors(start_idx: int, count: int, version: int = 1):
            return [
                {
                    "id": f"perf_vector_{i}",
                    "vector": [float(i), float(i + 1), float(i + 2)],
                    "metadata": {"index": i, "version": version}
                }
                for i in range(start_idx, start_idx + count)
            ]
        
        # Initial bulk insert
        start_time = time.time()
        for i in range(0, num_vectors, batch_size):
            batch = generate_vectors(i, min(batch_size, num_vectors - i))
            await test_framework.upsert_vectors(batch, upsert_mode=False)
        insert_time = time.time() - start_time
        
        # Bulk update (upsert existing)
        start_time = time.time()
        for i in range(0, num_vectors, batch_size):
            batch = generate_vectors(i, min(batch_size, num_vectors - i), version=2)
            await test_framework.upsert_vectors(batch, upsert_mode=True)
        update_time = time.time() - start_time
        
        # Verify deduplication worked
        search_result = await test_framework.search_vectors([0.0, 1.0, 2.0], k=num_vectors)
        results = search_result["data"]["results"]
        
        assert len(results) == num_vectors, f"Should find all {num_vectors} vectors"
        
        # Verify all are version 2 (updated)
        for result in results:
            assert result["metadata"]["version"] == 2, "All vectors should be updated version"
        
        print(f"Performance results ({test_framework.protocol}):")
        print(f"  Insert: {insert_time:.2f}s for {num_vectors} vectors ({num_vectors/insert_time:.0f} vec/s)")
        print(f"  Update: {update_time:.2f}s for {num_vectors} vectors ({num_vectors/update_time:.0f} vec/s)")

# Test runner
if __name__ == "__main__":
    # Run tests for both protocols
    import subprocess
    import sys
    
    print("Running Python SDK Upsert Tests...")
    print("=" * 50)
    
    # Run pytest with both REST and gRPC
    result = subprocess.run([
        sys.executable, "-m", "pytest", 
        __file__, 
        "-v", 
        "--tb=short",
        "-k", "not test_bulk_upsert_performance"  # Skip perf test in quick run
    ])
    
    if result.returncode == 0:
        print("\n✅ All upsert tests passed!")
    else:
        print("\n❌ Some tests failed!")
        sys.exit(1)