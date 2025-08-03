#!/usr/bin/env python3
"""
Proto-First Architecture Integration Test

This test validates that:
1. Python SDK sends pure proto VectorRecord messages 
2. Rust server handles proto VectorBatchRequest correctly
3. End-to-end proto-first architecture works without Avro
"""

import pytest
import time
import grpc
from proximadb.protocols.grpc_async import ProximaDBClient
from proximadb import proximadb_pb2 as pb2


class TestProtoFirstArchitecture:
    """Test suite for proto-first architecture"""
    
    @pytest.fixture
    def grpc_client(self):
        """Create gRPC client"""
        client = ProximaDBClient(endpoint="localhost:5679")
        yield client
        # No cleanup needed as client doesn't maintain state
    
    @pytest.fixture
    def test_collection_name(self):
        """Generate unique collection name"""
        return f"test_proto_collection_{int(time.time() * 1000)}"
    
    @pytest.fixture
    def collection_config(self, test_collection_name):
        """Create collection configuration"""
        return pb2.CollectionConfig(
            name=test_collection_name,
            dimension=3,
            distance_metric=pb2.DistanceMetric.COSINE,
            storage_engine=pb2.StorageEngine.VIPER
        )
    
    @pytest.fixture
    def setup_collection(self, grpc_client, collection_config):
        """Create collection and return its ID"""
        collection_request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_CREATE,
            collection_config=collection_config
        )
        
        collection_response = grpc_client.stub.CollectionOperation(collection_request)
        assert collection_response.success, f"Collection creation failed: {collection_response.error_message}"
        
        collection_id = collection_response.collection.id
        yield collection_id
        
        # Cleanup
        delete_request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_DELETE,
            collection_id=collection_id
        )
        try:
            grpc_client.stub.CollectionOperation(delete_request)
        except:
            pass  # Ignore cleanup errors

    def test_proto_first_vector_insert(self, grpc_client, setup_collection):
        """Test pure proto message insertion"""
        collection_id = setup_collection
        
        # Create pure proto vector records (no Avro involved)
        vector_records = [
            pb2.VectorRecord(
                id="test_vec_1",
                vector=[0.1, 0.2, 0.3],
                metadata={"type": "test", "index": "1"}
            ),
            pb2.VectorRecord(
                id="test_vec_2", 
                vector=[0.4, 0.5, 0.6],
                metadata={"type": "test", "index": "2"}
            ),
            pb2.VectorRecord(
                id="test_vec_3",
                vector=[0.7, 0.8, 0.9],
                metadata={"type": "test", "index": "3"}
            )
        ]
        
        # Create batch request
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        # Insert vectors
        insert_response = grpc_client.stub.InsertVectorsBatch(batch_request)
        assert insert_response.success, f"Vector insertion failed: {insert_response.error_message}"
        assert insert_response.count == 3, f"Expected 3 vectors inserted, got {insert_response.count}"

    def test_proto_first_vector_search(self, grpc_client, setup_collection):
        """Test search with proto messages"""
        collection_id = setup_collection
        
        # First insert some vectors
        vector_records = [
            pb2.VectorRecord(
                id=f"search_vec_{i}",
                vector=[float(i) * 0.1, float(i) * 0.2, float(i) * 0.3],
                metadata={"batch": str(i // 10)}
            )
            for i in range(20)
        ]
        
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        insert_response = grpc_client.stub.InsertVectorsBatch(batch_request)
        assert insert_response.success, f"Vector insertion failed: {insert_response.error_message}"
        
        # Now search
        search_request = pb2.VectorSearchRequest(
            collection_id=collection_id,
            vector=[0.5, 1.0, 1.5],
            top_k=5,
            include_metadata=True
        )
        
        search_response = grpc_client.stub.SearchVectors(search_request)
        assert search_response.success, f"Search failed: {search_response.error_message}"
        assert len(search_response.results) > 0, "No search results returned"
        assert len(search_response.results) <= 5, f"Too many results: {len(search_response.results)}"
        
        # Verify results have IDs and scores
        for result in search_response.results:
            assert result.id, "Result missing ID"
            assert result.score >= 0, "Invalid score"

    def test_proto_metadata_filtering(self, grpc_client, setup_collection):
        """Test metadata filtering with proto messages"""
        collection_id = setup_collection
        
        # Insert vectors with different metadata
        vector_records = [
            pb2.VectorRecord(
                id="electronics_1",
                vector=[0.1, 0.2, 0.3],
                metadata={"category": "electronics", "price": "100"}
            ),
            pb2.VectorRecord(
                id="electronics_2", 
                vector=[0.2, 0.3, 0.4],
                metadata={"category": "electronics", "price": "200"}
            ),
            pb2.VectorRecord(
                id="books_1",
                vector=[0.3, 0.4, 0.5],
                metadata={"category": "books", "price": "50"}
            )
        ]
        
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        insert_response = grpc_client.stub.InsertVectorsBatch(batch_request)
        assert insert_response.success, f"Vector insertion failed: {insert_response.error_message}"
        
        # Search with metadata filter
        metadata_filter = pb2.MetadataFilter(
            field="category",
            operator=pb2.ComparisonOperator.EQUALS,
            value=pb2.FilterValue(string_value="electronics")
        )
        
        search_request = pb2.VectorSearchRequest(
            collection_id=collection_id,
            vector=[0.15, 0.25, 0.35],
            top_k=10,
            metadata_filter=metadata_filter,
            include_metadata=True
        )
        
        search_response = grpc_client.stub.SearchVectors(search_request)
        assert search_response.success, f"Search with filter failed: {search_response.error_message}"
        
        # Verify only electronics items returned
        for result in search_response.results:
            assert result.id.startswith("electronics"), \
                f"Got non-electronics result: {result.id}"

    def test_proto_batch_operations_performance(self, grpc_client, setup_collection):
        """Test batch operations performance"""
        collection_id = setup_collection
        batch_size = 100
        
        # Create batch of vectors
        start_time = time.time()
        vector_records = [
            pb2.VectorRecord(
                id=f"perf_vec_{i}",
                vector=[float(i % 10) / 10.0 for _ in range(3)],
                metadata={"batch": str(i // 10)}
            )
            for i in range(batch_size)
        ]
        
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        insert_response = grpc_client.stub.InsertVectorsBatch(batch_request)
        insert_time = time.time() - start_time
        
        assert insert_response.success, f"Batch insertion failed: {insert_response.error_message}"
        assert insert_response.count == batch_size, \
            f"Expected {batch_size} vectors inserted, got {insert_response.count}"
        
        # Performance assertion
        vectors_per_second = batch_size / insert_time
        assert vectors_per_second > 100, \
            f"Insertion too slow: {vectors_per_second:.2f} vectors/sec"

    @pytest.mark.parametrize("storage_engine", [
        pb2.StorageEngine.VIPER,
        pb2.StorageEngine.SST
    ])
    def test_proto_with_different_engines(self, grpc_client, test_collection_name, storage_engine):
        """Test proto operations with different storage engines"""
        # Create collection with specific engine
        collection_config = pb2.CollectionConfig(
            name=f"{test_collection_name}_{storage_engine.name}",
            dimension=3,
            distance_metric=pb2.DistanceMetric.EUCLIDEAN,
            storage_engine=storage_engine
        )
        
        collection_request = pb2.CollectionRequest(
            operation=pb2.CollectionOperation.COLLECTION_CREATE,
            collection_config=collection_config
        )
        
        collection_response = grpc_client.stub.CollectionOperation(collection_request)
        assert collection_response.success, \
            f"Collection creation failed for {storage_engine.name}: {collection_response.error_message}"
        
        collection_id = collection_response.collection.id
        
        try:
            # Insert test vector
            vector_record = pb2.VectorRecord(
                id=f"engine_test_{storage_engine.name}",
                vector=[1.0, 2.0, 3.0],
                metadata={"engine": storage_engine.name}
            )
            
            batch_request = pb2.VectorBatchRequest(
                collection_id=collection_id,
                vectors=[vector_record]
            )
            
            insert_response = grpc_client.stub.InsertVectorsBatch(batch_request)
            assert insert_response.success, \
                f"Vector insertion failed for {storage_engine.name}: {insert_response.error_message}"
            
        finally:
            # Cleanup
            delete_request = pb2.CollectionRequest(
                operation=pb2.CollectionOperation.COLLECTION_DELETE,
                collection_id=collection_id
            )
            grpc_client.stub.CollectionOperation(delete_request)