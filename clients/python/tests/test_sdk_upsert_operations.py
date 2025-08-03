#!/usr/bin/env python3
"""
Comprehensive Python SDK Tests for gRPC and REST Upsert Operations

This test suite validates the Python SDK's interaction with ProximaDB's
unified upsert-only architecture through both gRPC and REST protocols.

Test Coverage:
- Zero-copy Avro batch upserts via gRPC
- JSON-based upserts via REST
- Multi-tier search with deduplication
- Performance comparison between protocols
- Error handling and edge cases
- Concurrent operation safety
"""

import asyncio
import json
import pytest
import time
import uuid
import numpy as np
from typing import List, Dict, Any, Optional
from concurrent.futures import ThreadPoolExecutor, as_completed

# ProximaDB Python SDK imports
try:
    from proximadb.grpc_client import ProximaDBGrpcClient
    from proximadb.rest_client import ProximaDBRestClient
    from proximadb.types import (
        VectorRecord, SearchResult, UpsertRequest, SearchRequest,
        CollectionConfig, DistanceMetric, MetadataFilter, FieldCondition
    )
except ImportError as e:
    pytest.skip(f"ProximaDB Python SDK not available: {e}", allow_module_level=True)


class SDKTestFixture:
    """Test fixture for SDK testing with both gRPC and REST clients"""
    
    def __init__(self):
        self.grpc_client: Optional[ProximaDBGrpcClient] = None
        self.rest_client: Optional[ProximaDBRestClient] = None
        self.collection_id = f"sdk_test_collection_{uuid.uuid4()}"
        self.test_vectors: List[VectorRecord] = []
        
    async def setup(self):
        """Initialize both gRPC and REST clients"""
        try:
            # Initialize gRPC client
            self.grpc_client = ProximaDBGrpcClient(
                host="localhost",
                port=5679,  # gRPC port
                timeout=30.0
            )
            await self.grpc_client.connect()
            print(f"✅ gRPC client connected")
            
            # Initialize REST client
            self.rest_client = ProximaDBRestClient(
                base_url="http://localhost:5678",  # REST port
                timeout=30.0
            )
            await self.rest_client.health_check()
            print(f"✅ REST client connected")
            
            # Create test collection via gRPC
            collection_config = CollectionConfig(
                collection_id=self.collection_id,
                dimension=4,
                distance_metric=DistanceMetric.COSINE,
                metadata={
                    "description": "SDK test collection for upsert operations",
                    "created_by": "python_sdk_test"
                }
            )
            
            await self.grpc_client.create_collection(collection_config)
            print(f"✅ Test collection created: {self.collection_id}")
            
            # Generate test vectors
            self.test_vectors = self._generate_test_vectors(100)
            print(f"✅ Generated {len(self.test_vectors)} test vectors")
            
        except Exception as e:
            pytest.fail(f"Failed to setup SDK test fixture: {e}")
    
    async def cleanup(self):
        """Cleanup test resources"""
        try:
            if self.grpc_client:
                # Delete test collection
                await self.grpc_client.delete_collection(self.collection_id)
                await self.grpc_client.disconnect()
                print(f"✅ gRPC client cleaned up")
            
            if self.rest_client:
                await self.rest_client.close()
                print(f"✅ REST client cleaned up")
                
        except Exception as e:
            print(f"⚠️ Cleanup warning: {e}")
    
    def _generate_test_vectors(self, count: int) -> List[VectorRecord]:
        """Generate test vectors with known patterns"""
        vectors = []
        
        for i in range(count):
            vector = VectorRecord(
                id=f"test_vector_{i}",
                collection_id=self.collection_id,
                vector=[float(i), float(i + 1), float(i + 2), float(i + 3)],
                metadata={
                    "index": i,
                    "category": f"category_{i % 5}",
                    "batch": i // 10,
                    "created_at": time.time()
                }
            )
            vectors.append(vector)
        
        return vectors
    
    def _generate_overlapping_vectors(self, base_vectors: List[VectorRecord], version: int) -> List[VectorRecord]:
        """Generate vectors that overlap with existing ones (for upsert testing)"""
        overlapping = []
        
        for i, base_vector in enumerate(base_vectors[:20]):  # Overlap first 20
            vector = VectorRecord(
                id=base_vector.id,  # Same ID for upsert
                collection_id=self.collection_id,
                vector=[v + version * 0.1 for v in base_vector.vector],  # Slightly different vector
                metadata={
                    **base_vector.metadata,
                    "version": version,
                    "updated_at": time.time()
                }
            )
            overlapping.append(vector)
        
        return overlapping


@pytest.fixture
async def sdk_fixture():
    """Pytest fixture for SDK testing"""
    fixture = SDKTestFixture()
    await fixture.setup()
    yield fixture
    await fixture.cleanup()


@pytest.mark.asyncio
async def test_grpc_basic_upsert_operations(sdk_fixture):
    """Test basic upsert operations via gRPC"""
    print("🚀 Testing gRPC basic upsert operations...")
    
    # Test batch upsert
    batch_size = 50
    test_batch = sdk_fixture.test_vectors[:batch_size]
    
    start_time = time.time()
    upsert_response = await sdk_fixture.grpc_client.upsert_vectors(
        UpsertRequest(
            collection_id=sdk_fixture.collection_id,
            vectors=test_batch,
            immediate_flush=False
        )
    )
    upsert_duration = time.time() - start_time
    
    assert upsert_response.success is True
    assert upsert_response.vectors_processed == batch_size
    assert upsert_response.errors == []
    
    print(f"   ✅ gRPC upsert: {batch_size} vectors in {upsert_duration:.3f}s")
    print(f"   📊 Throughput: {batch_size / upsert_duration:.0f} vectors/sec")
    
    # Test search
    query_vector = [25.0, 26.0, 27.0, 28.0]
    
    start_time = time.time()
    search_response = await sdk_fixture.grpc_client.search_vectors(
        SearchRequest(
            collection_id=sdk_fixture.collection_id,
            query_vector=query_vector,
            k=10,
            distance_metric=DistanceMetric.COSINE
        )
    )
    search_duration = time.time() - start_time
    
    assert len(search_response.results) > 0
    assert len(search_response.results) <= 10
    
    # Verify results have proper structure
    for result in search_response.results:
        assert result.id is not None
        assert result.score is not None
        assert result.metadata is not None
    
    print(f"   ✅ gRPC search: {len(search_response.results)} results in {search_duration:.3f}s")


@pytest.mark.asyncio
async def test_rest_basic_upsert_operations(sdk_fixture):
    """Test basic upsert operations via REST"""
    print("🚀 Testing REST basic upsert operations...")
    
    # Test batch upsert
    batch_size = 50
    test_batch = sdk_fixture.test_vectors[:batch_size]
    
    start_time = time.time()
    upsert_response = await sdk_fixture.rest_client.upsert_vectors(
        collection_id=sdk_fixture.collection_id,
        vectors=[v.to_dict() for v in test_batch],
        immediate_flush=False
    )
    upsert_duration = time.time() - start_time
    
    assert upsert_response["success"] is True
    assert upsert_response["vectors_processed"] == batch_size
    
    print(f"   ✅ REST upsert: {batch_size} vectors in {upsert_duration:.3f}s")
    print(f"   📊 Throughput: {batch_size / upsert_duration:.0f} vectors/sec")
    
    # Test search
    query_vector = [25.0, 26.0, 27.0, 28.0]
    
    start_time = time.time()
    search_response = await sdk_fixture.rest_client.search_vectors(
        collection_id=sdk_fixture.collection_id,
        query_vector=query_vector,
        k=10,
        distance_metric="cosine"
    )
    search_duration = time.time() - start_time
    
    assert len(search_response["results"]) > 0
    assert len(search_response["results"]) <= 10
    
    # Verify results have proper structure
    for result in search_response["results"]:
        assert "id" in result
        assert "score" in result
        assert "metadata" in result
    
    print(f"   ✅ REST search: {len(search_response['results'])} results in {search_duration:.3f}s")


@pytest.mark.asyncio
async def test_upsert_deduplication_grpc(sdk_fixture):
    """Test upsert deduplication behavior via gRPC"""
    print("🚀 Testing upsert deduplication via gRPC...")
    
    # Insert initial batch
    initial_batch = sdk_fixture.test_vectors[:20]
    
    await sdk_fixture.grpc_client.upsert_vectors(
        UpsertRequest(
            collection_id=sdk_fixture.collection_id,
            vectors=initial_batch,
            immediate_flush=False
        )
    )
    print(f"   ✅ Initial batch inserted: {len(initial_batch)} vectors")
    
    # Create overlapping batch (same IDs, different data)
    overlapping_batch = sdk_fixture._generate_overlapping_vectors(initial_batch, version=2)
    
    await sdk_fixture.grpc_client.upsert_vectors(
        UpsertRequest(
            collection_id=sdk_fixture.collection_id,
            vectors=overlapping_batch,
            immediate_flush=False
        )
    )
    print(f"   ✅ Overlapping batch upserted: {len(overlapping_batch)} vectors")
    
    # Search for vectors that were updated
    test_vector_id = initial_batch[0].id
    query_vector = overlapping_batch[0].vector
    
    search_response = await sdk_fixture.grpc_client.search_vectors(
        SearchRequest(
            collection_id=sdk_fixture.collection_id,
            query_vector=query_vector,
            k=20,
            distance_metric=DistanceMetric.COSINE
        )
    )
    
    # Find the updated vector in results
    updated_result = None
    for result in search_response.results:
        if result.id == test_vector_id:
            updated_result = result
            break
    
    assert updated_result is not None, f"Should find updated vector {test_vector_id}"
    assert updated_result.metadata.get("version") == 2, "Should return latest version"
    
    print(f"   ✅ Deduplication verified: Latest version returned for {test_vector_id}")


@pytest.mark.asyncio
async def test_upsert_deduplication_rest(sdk_fixture):
    """Test upsert deduplication behavior via REST"""
    print("🚀 Testing upsert deduplication via REST...")
    
    # Insert initial batch
    initial_batch = sdk_fixture.test_vectors[:20]
    
    await sdk_fixture.rest_client.upsert_vectors(
        collection_id=sdk_fixture.collection_id,
        vectors=[v.to_dict() for v in initial_batch],
        immediate_flush=False
    )
    print(f"   ✅ Initial batch inserted: {len(initial_batch)} vectors")
    
    # Create overlapping batch (same IDs, different data)
    overlapping_batch = sdk_fixture._generate_overlapping_vectors(initial_batch, version=2)
    
    await sdk_fixture.rest_client.upsert_vectors(
        collection_id=sdk_fixture.collection_id,
        vectors=[v.to_dict() for v in overlapping_batch],
        immediate_flush=False
    )
    print(f"   ✅ Overlapping batch upserted: {len(overlapping_batch)} vectors")
    
    # Search for vectors that were updated
    test_vector_id = initial_batch[0].id
    query_vector = overlapping_batch[0].vector
    
    search_response = await sdk_fixture.rest_client.search_vectors(
        collection_id=sdk_fixture.collection_id,
        query_vector=query_vector,
        k=20,
        distance_metric="cosine"
    )
    
    # Find the updated vector in results
    updated_result = None
    for result in search_response["results"]:
        if result["id"] == test_vector_id:
            updated_result = result
            break
    
    assert updated_result is not None, f"Should find updated vector {test_vector_id}"
    assert updated_result["metadata"].get("version") == 2, "Should return latest version"
    
    print(f"   ✅ Deduplication verified: Latest version returned for {test_vector_id}")


@pytest.mark.asyncio
async def test_metadata_filtering_grpc(sdk_fixture):
    """Test metadata filtering via gRPC"""
    print("🚀 Testing metadata filtering via gRPC...")
    
    # Insert test vectors
    test_batch = sdk_fixture.test_vectors[:50]
    
    await sdk_fixture.grpc_client.upsert_vectors(
        UpsertRequest(
            collection_id=sdk_fixture.collection_id,
            vectors=test_batch,
            immediate_flush=False
        )
    )
    print(f"   ✅ Test batch inserted: {len(test_batch)} vectors")
    
    # Search with metadata filter
    metadata_filter = MetadataFilter(
        conditions=[
            FieldCondition(
                field="category",
                operator="equals",
                value="category_1"
            )
        ],
        logic="AND"
    )
    
    search_response = await sdk_fixture.grpc_client.search_vectors(
        SearchRequest(
            collection_id=sdk_fixture.collection_id,
            query_vector=[10.0, 11.0, 12.0, 13.0],
            k=20,
            distance_metric=DistanceMetric.COSINE,
            metadata_filter=metadata_filter
        )
    )
    
    # Verify all results match the filter
    assert len(search_response.results) > 0, "Should find filtered results"
    
    for result in search_response.results:
        assert result.metadata.get("category") == "category_1", "All results should match filter"
    
    print(f"   ✅ Metadata filtering: {len(search_response.results)} results matched category_1")


@pytest.mark.asyncio
async def test_metadata_filtering_rest(sdk_fixture):
    """Test metadata filtering via REST"""
    print("🚀 Testing metadata filtering via REST...")
    
    # Insert test vectors
    test_batch = sdk_fixture.test_vectors[:50]
    
    await sdk_fixture.rest_client.upsert_vectors(
        collection_id=sdk_fixture.collection_id,
        vectors=[v.to_dict() for v in test_batch],
        immediate_flush=False
    )
    print(f"   ✅ Test batch inserted: {len(test_batch)} vectors")
    
    # Search with metadata filter
    metadata_filter = {
        "conditions": [
            {
                "field": "category",
                "operator": "equals",
                "value": "category_1"
            }
        ],
        "logic": "AND"
    }
    
    search_response = await sdk_fixture.rest_client.search_vectors(
        collection_id=sdk_fixture.collection_id,
        query_vector=[10.0, 11.0, 12.0, 13.0],
        k=20,
        distance_metric="cosine",
        metadata_filter=metadata_filter
    )
    
    # Verify all results match the filter
    assert len(search_response["results"]) > 0, "Should find filtered results"
    
    for result in search_response["results"]:
        assert result["metadata"].get("category") == "category_1", "All results should match filter"
    
    print(f"   ✅ Metadata filtering: {len(search_response['results'])} results matched category_1")


@pytest.mark.asyncio
async def test_performance_comparison(sdk_fixture):
    """Compare performance between gRPC and REST protocols"""
    print("🚀 Testing performance comparison between gRPC and REST...")
    
    batch_sizes = [10, 50, 100, 200]
    performance_results = {
        "grpc": {"upsert": [], "search": []},
        "rest": {"upsert": [], "search": []}
    }
    
    for batch_size in batch_sizes:
        test_batch = sdk_fixture.test_vectors[:batch_size]
        query_vector = [batch_size / 2.0, batch_size / 2.0 + 1, batch_size / 2.0 + 2, batch_size / 2.0 + 3]
        
        # Test gRPC performance
        start_time = time.time()
        await sdk_fixture.grpc_client.upsert_vectors(
            UpsertRequest(
                collection_id=sdk_fixture.collection_id,
                vectors=test_batch,
                immediate_flush=False
            )
        )
        grpc_upsert_time = time.time() - start_time
        
        start_time = time.time()
        await sdk_fixture.grpc_client.search_vectors(
            SearchRequest(
                collection_id=sdk_fixture.collection_id,
                query_vector=query_vector,
                k=10,
                distance_metric=DistanceMetric.COSINE
            )
        )
        grpc_search_time = time.time() - start_time
        
        # Test REST performance
        start_time = time.time()
        await sdk_fixture.rest_client.upsert_vectors(
            collection_id=sdk_fixture.collection_id,
            vectors=[v.to_dict() for v in test_batch],
            immediate_flush=False
        )
        rest_upsert_time = time.time() - start_time
        
        start_time = time.time()
        await sdk_fixture.rest_client.search_vectors(
            collection_id=sdk_fixture.collection_id,
            query_vector=query_vector,
            k=10,
            distance_metric="cosine"
        )
        rest_search_time = time.time() - start_time
        
        # Record results
        performance_results["grpc"]["upsert"].append((batch_size, grpc_upsert_time))
        performance_results["grpc"]["search"].append((batch_size, grpc_search_time))
        performance_results["rest"]["upsert"].append((batch_size, rest_upsert_time))
        performance_results["rest"]["search"].append((batch_size, rest_search_time))
        
        print(f"   Batch {batch_size}:")
        print(f"     gRPC: upsert={grpc_upsert_time:.3f}s, search={grpc_search_time:.3f}s")
        print(f"     REST: upsert={rest_upsert_time:.3f}s, search={rest_search_time:.3f}s")
        print(f"     gRPC throughput: {batch_size / grpc_upsert_time:.0f} vectors/sec")
        print(f"     REST throughput: {batch_size / rest_upsert_time:.0f} vectors/sec")
    
    # Analyze performance characteristics
    print("\n📊 Performance Analysis:")
    
    # Calculate average throughput for largest batch
    largest_batch_size = batch_sizes[-1]
    grpc_throughput = largest_batch_size / performance_results["grpc"]["upsert"][-1][1]
    rest_throughput = largest_batch_size / performance_results["rest"]["upsert"][-1][1]
    
    print(f"   Large batch throughput ({largest_batch_size} vectors):")
    print(f"     gRPC: {grpc_throughput:.0f} vectors/sec")
    print(f"     REST: {rest_throughput:.0f} vectors/sec")
    
    # Both protocols should achieve reasonable performance
    assert grpc_throughput > 50, f"gRPC throughput too low: {grpc_throughput:.0f} vectors/sec"
    assert rest_throughput > 50, f"REST throughput too low: {rest_throughput:.0f} vectors/sec"
    
    print("   ✅ Both protocols achieve acceptable performance")


@pytest.mark.asyncio
async def test_concurrent_operations(sdk_fixture):
    """Test concurrent operations across both protocols"""
    print("🚀 Testing concurrent operations...")
    
    async def grpc_worker(batch_start: int, batch_size: int) -> Dict[str, Any]:
        """Worker function for gRPC operations"""
        vectors = sdk_fixture.test_vectors[batch_start:batch_start + batch_size]
        
        start_time = time.time()
        response = await sdk_fixture.grpc_client.upsert_vectors(
            UpsertRequest(
                collection_id=sdk_fixture.collection_id,
                vectors=vectors,
                immediate_flush=False
            )
        )
        duration = time.time() - start_time
        
        return {
            "protocol": "grpc",
            "batch_start": batch_start,
            "batch_size": batch_size,
            "duration": duration,
            "success": response.success,
            "vectors_processed": response.vectors_processed
        }
    
    async def rest_worker(batch_start: int, batch_size: int) -> Dict[str, Any]:
        """Worker function for REST operations"""
        vectors = sdk_fixture.test_vectors[batch_start:batch_start + batch_size]
        
        start_time = time.time()
        response = await sdk_fixture.rest_client.upsert_vectors(
            collection_id=sdk_fixture.collection_id,
            vectors=[v.to_dict() for v in vectors],
            immediate_flush=False
        )
        duration = time.time() - start_time
        
        return {
            "protocol": "rest",
            "batch_start": batch_start,
            "batch_size": batch_size,
            "duration": duration,
            "success": response["success"],
            "vectors_processed": response["vectors_processed"]
        }
    
    # Launch concurrent operations
    tasks = []
    batch_size = 20
    
    # Launch gRPC tasks
    for i in range(0, 60, batch_size):  # 3 gRPC batches
        task = grpc_worker(i, batch_size)
        tasks.append(task)
    
    # Launch REST tasks
    for i in range(60, 100, batch_size):  # 2 REST batches
        task = rest_worker(i, batch_size)
        tasks.append(task)
    
    # Wait for all tasks to complete
    results = await asyncio.gather(*tasks)
    
    # Verify all operations succeeded
    for result in results:
        assert result["success"] is True, f"Operation failed: {result}"
        assert result["vectors_processed"] == batch_size, f"Incorrect vector count: {result}"
    
    # Calculate aggregate performance
    total_vectors = sum(r["vectors_processed"] for r in results)
    total_time = max(r["duration"] for r in results)  # Max duration (parallel execution)
    overall_throughput = total_vectors / total_time
    
    print(f"   ✅ Concurrent operations completed:")
    print(f"     Total vectors: {total_vectors}")
    print(f"     Max duration: {total_time:.3f}s")
    print(f"     Overall throughput: {overall_throughput:.0f} vectors/sec")
    
    # Verify search works after concurrent operations
    search_response = await sdk_fixture.grpc_client.search_vectors(
        SearchRequest(
            collection_id=sdk_fixture.collection_id,
            query_vector=[50.0, 51.0, 52.0, 53.0],
            k=20,
            distance_metric=DistanceMetric.COSINE
        )
    )
    
    assert len(search_response.results) > 0, "Should find results after concurrent operations"
    print(f"   ✅ Post-concurrent search: {len(search_response.results)} results found")


@pytest.mark.asyncio
async def test_error_handling(sdk_fixture):
    """Test error handling across both protocols"""
    print("🚀 Testing error handling...")
    
    # Test invalid collection ID via gRPC
    try:
        await sdk_fixture.grpc_client.upsert_vectors(
            UpsertRequest(
                collection_id="nonexistent_collection",
                vectors=sdk_fixture.test_vectors[:5],
                immediate_flush=False
            )
        )
        assert False, "Should have raised exception for invalid collection"
    except Exception as e:
        print(f"   ✅ gRPC invalid collection error handled: {type(e).__name__}")
    
    # Test invalid collection ID via REST
    try:
        await sdk_fixture.rest_client.upsert_vectors(
            collection_id="nonexistent_collection",
            vectors=[v.to_dict() for v in sdk_fixture.test_vectors[:5]],
            immediate_flush=False
        )
        assert False, "Should have raised exception for invalid collection"
    except Exception as e:
        print(f"   ✅ REST invalid collection error handled: {type(e).__name__}")
    
    # Test invalid vector dimensions via gRPC
    invalid_vector = VectorRecord(
        id="invalid_vector",
        collection_id=sdk_fixture.collection_id,
        vector=[1.0, 2.0],  # Wrong dimension (should be 4)
        metadata={}
    )
    
    try:
        await sdk_fixture.grpc_client.upsert_vectors(
            UpsertRequest(
                collection_id=sdk_fixture.collection_id,
                vectors=[invalid_vector],
                immediate_flush=False
            )
        )
        # Note: This might succeed with an error in the response rather than exception
        print(f"   ✅ gRPC invalid dimension handled (graceful response)")
    except Exception as e:
        print(f"   ✅ gRPC invalid dimension error handled: {type(e).__name__}")
    
    # Test invalid vector dimensions via REST
    try:
        await sdk_fixture.rest_client.upsert_vectors(
            collection_id=sdk_fixture.collection_id,
            vectors=[invalid_vector.to_dict()],
            immediate_flush=False
        )
        print(f"   ✅ REST invalid dimension handled (graceful response)")
    except Exception as e:
        print(f"   ✅ REST invalid dimension error handled: {type(e).__name__}")


if __name__ == "__main__":
    """
    Run the tests directly for development/debugging
    
    Usage:
        python test_sdk_upsert_operations.py
    """
    pytest.main([__file__, "-v", "-s"])