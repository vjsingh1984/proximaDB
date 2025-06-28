"""
gRPC-Specific End-to-End Tests
Tests gRPC-specific functionality, performance characteristics, and protocol features.
"""

import pytest
import asyncio
import logging
import time
import numpy as np
import json
import uuid
from typing import List, Dict, Optional
import sys
import os
from sentence_transformers import SentenceTransformer

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient, Protocol
from proximadb.exceptions import ProximaDBError
from proximadb.models import CollectionConfig

logger = logging.getLogger('grpc_specific_test')


class TestGRPCSpecificFeatures:
    """Test gRPC-specific features and performance characteristics"""
    
    @pytest.fixture
    def grpc_client(self):
        """Create gRPC client specifically"""
        client = ProximaDBClient("http://localhost:5679", protocol=Protocol.GRPC)
        
        # Wait for server to be ready
        max_retries = 30
        for i in range(max_retries):
            try:
                health = client.health()
                if health:
                    logger.info("✅ gRPC server ready")
                    break
            except Exception as e:
                if i == max_retries - 1:
                    pytest.skip(f"gRPC server not available: {e}")
                time.sleep(1)
        
        yield client
        client.close()
    
    @pytest.fixture
    def bert_model(self):
        """Load BERT model for testing"""
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    @pytest.fixture
    def test_collection(self, grpc_client):
        """Create test collection for gRPC tests"""
        collection_name = f"grpc_test_{uuid.uuid4().hex[:8]}"
        config = CollectionConfig(
            dimension=384,
            distance_metric="cosine",
            index_config={
                "index_type": "hnsw",
                "ef_construction": 200,
                "max_connections": 16
            }
        )
        
        collection = grpc_client.create_collection(
            name=collection_name,
            config=config
        )
        
        yield collection
        
        # Cleanup
        try:
            grpc_client.delete_collection(collection_name)
        except Exception as e:
            logger.warning(f"⚠️ Failed to cleanup gRPC test collection: {e}")
    
    def test_grpc_protocol_verification(self, grpc_client):
        """Verify that gRPC protocol is being used"""
        assert grpc_client.active_protocol == Protocol.GRPC
        
        perf_info = grpc_client.get_performance_info()
        assert perf_info["protocol"] == "gRPC"
        assert "Binary Protocol Buffers" in perf_info["serialization"]
        logger.info("✅ gRPC protocol verified")
    
    def test_grpc_uuid_based_operations(self, grpc_client, test_collection, bert_model):
        """Test UUID-based operations via gRPC"""
        collection_id = test_collection.name
        collection_uuid = getattr(test_collection, 'id', None)
        
        # Generate test data
        text = "gRPC UUID test document with BERT embeddings"
        embedding = bert_model.encode(text).tolist()
        vector_id = f"grpc_uuid_vector_{uuid.uuid4().hex[:8]}"
        metadata = {
            "text": text,
            "protocol": "grpc",
            "test_type": "uuid_operations",
            "timestamp": int(time.time())
        }
        
        try:
            # Insert vector using collection name
            insert_result = grpc_client.insert_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                vector=embedding,
                metadata=metadata
            )
            assert insert_result is not None
            logger.info(f"✅ Vector inserted via gRPC: {vector_id}")
            
            # Test search using collection name
            search_results = grpc_client.search(
                collection_id=collection_id,
                query=embedding,
                k=5,
                include_metadata=True
            )
            assert len(search_results) >= 1
            assert search_results[0].id == vector_id
            logger.info("✅ Search via collection name successful")
            
            # Test UUID-based access if available
            if collection_uuid:
                # Insert another vector using UUID
                uuid_vector_id = f"grpc_uuid_vector_2_{uuid.uuid4().hex[:8]}"
                uuid_insert_result = grpc_client.insert_vector(
                    collection_id=collection_uuid,
                    vector_id=uuid_vector_id,
                    vector=embedding,
                    metadata={**metadata, "access_method": "uuid"}
                )
                assert uuid_insert_result is not None
                logger.info(f"✅ Vector inserted via UUID: {uuid_vector_id}")
                
                # Search using UUID
                uuid_search_results = grpc_client.search(
                    collection_id=collection_uuid,
                    query=embedding,
                    k=5,
                    include_metadata=True
                )
                assert len(uuid_search_results) >= 2  # Should find both vectors
                logger.info("✅ Search via UUID successful")
        
        finally:
            # Cleanup vectors
            try:
                grpc_client.delete_vector(collection_id, vector_id)
                if collection_uuid:
                    grpc_client.delete_vector(collection_uuid, f"grpc_uuid_vector_2_{uuid.uuid4().hex[:8]}")
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup vectors: {e}")
    
    def test_grpc_performance_characteristics(self, grpc_client, test_collection, bert_model):
        """Test gRPC performance characteristics with batch operations"""
        collection_id = test_collection.name
        
        # Generate test data for performance testing
        texts = [
            f"Performance test document {i}: gRPC binary protocol efficiency"
            for i in range(100)
        ]
        embeddings = [bert_model.encode(text).tolist() for text in texts]
        vector_ids = [f"perf_vector_{i}" for i in range(100)]
        metadata_list = [
            {
                "text": text,
                "protocol": "grpc",
                "batch_index": i,
                "performance_test": True
            }
            for i, text in enumerate(texts)
        ]
        
        try:
            # Test batch insert performance
            start_time = time.time()
            batch_result = grpc_client.insert_vectors(
                collection_id=collection_id,
                vectors=embeddings,
                ids=vector_ids,
                metadata=metadata_list
            )
            insert_time = time.time() - start_time
            
            assert batch_result.inserted_count == 100
            insert_rate = 100 / insert_time
            logger.info(f"✅ gRPC batch insert: {100} vectors in {insert_time:.2f}s "
                       f"({insert_rate:.1f} vec/sec)")
            
            # Test search performance
            search_times = []
            for i in range(10):  # Multiple searches for average
                start_time = time.time()
                search_results = grpc_client.search(
                    collection_id=collection_id,
                    query=embeddings[0],
                    k=10,
                    include_metadata=True
                )
                search_time = time.time() - start_time
                search_times.append(search_time)
                assert len(search_results) == 10
            
            avg_search_time = sum(search_times) / len(search_times)
            logger.info(f"✅ gRPC search performance: {avg_search_time*1000:.1f}ms average")
            
            # Test filtered search performance
            filter_start = time.time()
            filtered_results = grpc_client.search(
                collection_id=collection_id,
                query=embeddings[0],
                k=10,
                filter={"performance_test": True},
                include_metadata=True
            )
            filter_time = time.time() - filter_start
            
            assert len(filtered_results) == 10
            logger.info(f"✅ gRPC filtered search: {filter_time*1000:.1f}ms")
        
        finally:
            # Cleanup
            try:
                grpc_client.delete_vectors(collection_id, vector_ids)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup performance test vectors: {e}")
    
    def test_grpc_large_payload_handling(self, grpc_client, test_collection, bert_model):
        """Test gRPC handling of large payloads (binary protocol efficiency)"""
        collection_id = test_collection.name
        
        # Create large metadata to test binary protocol efficiency
        large_text = "Large document content: " + "AI and machine learning " * 100
        embedding = bert_model.encode(large_text).tolist()
        
        large_metadata = {
            "text": large_text,
            "protocol": "grpc",
            "large_payload_test": True,
            "additional_data": {
                "field_1": "value_1" * 50,
                "field_2": "value_2" * 50,
                "field_3": list(range(100)),  # Large list
                "nested": {
                    "deep_field": "deep_value" * 30,
                    "deeper": {
                        "very_deep": "very_deep_value" * 20
                    }
                }
            },
            "tags": [f"tag_{i}" for i in range(50)],
            "timestamp": int(time.time())
        }
        
        vector_id = f"large_payload_vector_{uuid.uuid4().hex[:8]}"
        
        try:
            # Test large payload insert
            start_time = time.time()
            insert_result = grpc_client.insert_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                vector=embedding,
                metadata=large_metadata
            )
            insert_time = time.time() - start_time
            
            assert insert_result is not None
            logger.info(f"✅ Large payload insert via gRPC: {insert_time*1000:.1f}ms")
            
            # Test retrieval of large payload
            start_time = time.time()
            retrieved_vector = grpc_client.get_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                include_vector=True,
                include_metadata=True
            )
            retrieval_time = time.time() - start_time
            
            assert retrieved_vector is not None
            assert retrieved_vector["metadata"]["large_payload_test"] is True
            assert len(retrieved_vector["metadata"]["additional_data"]["field_1"]) > 100
            logger.info(f"✅ Large payload retrieval via gRPC: {retrieval_time*1000:.1f}ms")
            
            # Test search with large payload return
            start_time = time.time()
            search_results = grpc_client.search(
                collection_id=collection_id,
                query=embedding,
                k=5,
                include_vectors=True,
                include_metadata=True
            )
            search_time = time.time() - start_time
            
            assert len(search_results) >= 1
            assert search_results[0].metadata["large_payload_test"] is True
            logger.info(f"✅ Large payload search via gRPC: {search_time*1000:.1f}ms")
        
        finally:
            # Cleanup
            try:
                grpc_client.delete_vector(collection_id, vector_id)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup large payload vector: {e}")
    
    def test_grpc_error_handling(self, grpc_client):
        """Test gRPC-specific error handling"""
        # Test non-existent collection
        with pytest.raises(ProximaDBError):
            grpc_client.get_collection("non_existent_collection")
        
        # Test invalid vector operations
        with pytest.raises(ProximaDBError):
            grpc_client.insert_vector(
                collection_id="non_existent_collection",
                vector_id="test_vector",
                vector=[0.1, 0.2, 0.3],
                metadata={}
            )
        
        logger.info("✅ gRPC error handling verified")


@pytest.mark.integration
@pytest.mark.performance
class TestGRPCPerformanceComparison:
    """Compare gRPC vs REST performance characteristics"""
    
    @pytest.fixture
    def both_clients(self):
        """Create both gRPC and REST clients for comparison"""
        grpc_client = ProximaDBClient("http://localhost:5679", protocol=Protocol.GRPC)
        rest_client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Wait for both servers to be ready
        for client, name in [(grpc_client, "gRPC"), (rest_client, "REST")]:
            max_retries = 30
            for i in range(max_retries):
                try:
                    health = client.health()
                    if health:
                        logger.info(f"✅ {name} server ready")
                        break
                except Exception as e:
                    if i == max_retries - 1:
                        pytest.skip(f"{name} server not available: {e}")
                    time.sleep(1)
        
        yield grpc_client, rest_client
        
        grpc_client.close()
        rest_client.close()
    
    def test_protocol_performance_comparison(self, both_clients):
        """Compare gRPC vs REST performance for various operations"""
        grpc_client, rest_client = both_clients
        
        # Create collections on both
        collection_name_grpc = f"perf_grpc_{uuid.uuid4().hex[:8]}"
        collection_name_rest = f"perf_rest_{uuid.uuid4().hex[:8]}"
        
        config = CollectionConfig(
            dimension=384,
            distance_metric="cosine"
        )
        
        try:
            # Collection creation comparison
            start_time = time.time()
            grpc_collection = grpc_client.create_collection(collection_name_grpc, config)
            grpc_create_time = time.time() - start_time
            
            start_time = time.time()
            rest_collection = rest_client.create_collection(collection_name_rest, config)
            rest_create_time = time.time() - start_time
            
            logger.info(f"📊 Collection creation - gRPC: {grpc_create_time*1000:.1f}ms, "
                       f"REST: {rest_create_time*1000:.1f}ms")
            
            # Generate test data
            bert_model = SentenceTransformer('all-MiniLM-L6-v2')
            text = "Performance comparison test document"
            embedding = bert_model.encode(text).tolist()
            metadata = {"text": text, "performance_test": True}
            
            # Vector insert comparison
            start_time = time.time()
            grpc_client.insert_vector(
                collection_id=collection_name_grpc,
                vector_id="perf_vector_grpc",
                vector=embedding,
                metadata=metadata
            )
            grpc_insert_time = time.time() - start_time
            
            start_time = time.time()
            rest_client.insert_vector(
                collection_id=collection_name_rest,
                vector_id="perf_vector_rest",
                vector=embedding,
                metadata=metadata
            )
            rest_insert_time = time.time() - start_time
            
            logger.info(f"📊 Vector insert - gRPC: {grpc_insert_time*1000:.1f}ms, "
                       f"REST: {rest_insert_time*1000:.1f}ms")
            
            # Search comparison
            start_time = time.time()
            grpc_results = grpc_client.search(
                collection_id=collection_name_grpc,
                query=embedding,
                k=5,
                include_metadata=True
            )
            grpc_search_time = time.time() - start_time
            
            start_time = time.time()
            rest_results = rest_client.search(
                collection_id=collection_name_rest,
                query=embedding,
                k=5,
                include_metadata=True
            )
            rest_search_time = time.time() - start_time
            
            logger.info(f"📊 Vector search - gRPC: {grpc_search_time*1000:.1f}ms, "
                       f"REST: {rest_search_time*1000:.1f}ms")
            
            assert len(grpc_results) >= 1
            assert len(rest_results) >= 1
            
            # Calculate performance ratio
            total_grpc_time = grpc_create_time + grpc_insert_time + grpc_search_time
            total_rest_time = rest_create_time + rest_insert_time + rest_search_time
            performance_ratio = total_rest_time / total_grpc_time if total_grpc_time > 0 else 1
            
            logger.info(f"📊 Overall performance ratio (REST/gRPC): {performance_ratio:.2f}x")
            
        finally:
            # Cleanup
            try:
                grpc_client.delete_collection(collection_name_grpc)
                rest_client.delete_collection(collection_name_rest)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup performance test collections: {e}")


# Mark all tests as integration tests
pytestmark = [pytest.mark.integration, pytest.mark.performance]


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])