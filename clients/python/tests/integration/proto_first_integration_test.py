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
                metadata=[
                    pb2.MetadataItem(key="type", string_value="test"),
                    pb2.MetadataItem(key="index", string_value="1")
                ]
            ),
            pb2.VectorRecord(
                id="test_vec_2", 
                vector=[0.4, 0.5, 0.6],
                metadata=[
                    pb2.MetadataItem(key="type", string_value="test"),
                    pb2.MetadataItem(key="index", string_value="2")
                ]
            ),
            pb2.VectorRecord(
                id="test_vec_3",
                vector=[0.7, 0.8, 0.9],
                metadata=[
                    pb2.MetadataItem(key="type", string_value="test"),
                    pb2.MetadataItem(key="index", string_value="3")
                ]
            )
        ]
        
        # Create batch request
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        # Insert vectors
        insert_response = grpc_client.stub.VectorBatch(batch_request)
        assert insert_response.success, f"Vector insertion failed: {insert_response.error_message}"
        assert insert_response.metrics.total_processed == 3, f"Expected 3 vectors inserted, got {insert_response.metrics.total_processed}"

    def test_proto_first_vector_search(self, grpc_client, setup_collection):
        """Test search with proto messages"""
        collection_id = setup_collection
        
        # First insert some vectors
        vector_records = [
            pb2.VectorRecord(
                id=f"search_vec_{i}",
                vector=[float(i) * 0.1, float(i) * 0.2, float(i) * 0.3],
                metadata=[pb2.MetadataItem(key="batch", string_value=str(i // 10))]
            )
            for i in range(20)
        ]
        
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        insert_response = grpc_client.stub.VectorBatch(batch_request)
        assert insert_response.success, f"Vector insertion failed: {insert_response.error_message}"
        
        # Now search
        search_query = pb2.SearchQuery(
            vector=[0.5, 1.0, 1.5]
        )
        
        search_request = pb2.VectorSearchRequest(
            collection_id=collection_id,
            queries=[search_query],
            top_k=5,
            include_fields=pb2.IncludeFields(metadata=True, score=True)
        )
        
        search_response = grpc_client.stub.VectorSearch(search_request)
        assert search_response.success, f"Search failed: {search_response.error_message}"
        assert search_response.compact_results is not None, "No compact results in response"
        assert len(search_response.compact_results.results) > 0, "No search results returned"
        assert len(search_response.compact_results.results) <= 5, f"Too many results: {len(search_response.compact_results.results)}"
        
        # Verify results have IDs and scores
        for result in search_response.compact_results.results:
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
                metadata=[
                    pb2.MetadataItem(key="category", string_value="electronics"),
                    pb2.MetadataItem(key="price", string_value="100")
                ]
            ),
            pb2.VectorRecord(
                id="electronics_2", 
                vector=[0.2, 0.3, 0.4],
                metadata=[
                    pb2.MetadataItem(key="category", string_value="electronics"),
                    pb2.MetadataItem(key="price", string_value="200")
                ]
            ),
            pb2.VectorRecord(
                id="books_1",
                vector=[0.3, 0.4, 0.5],
                metadata=[
                    pb2.MetadataItem(key="category", string_value="books"),
                    pb2.MetadataItem(key="price", string_value="50")
                ]
            )
        ]
        
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        insert_response = grpc_client.stub.VectorBatch(batch_request)
        assert insert_response.success, f"Vector insertion failed: {insert_response.error_message}"
        
        # Search with metadata filter
        filter_condition = pb2.FilterCondition(
            field_name="category",
            operation=pb2.FilterOperation.EQUALS,
            value=pb2.MetadataValue(string_value="electronics")
        )
        
        metadata_filter = pb2.MetadataFilter(
            conditions=[filter_condition],
            operator=pb2.FilterOperator.AND
        )
        
        search_query = pb2.SearchQuery(
            vector=[0.15, 0.25, 0.35],
            metadata_filter=metadata_filter
        )
        
        search_request = pb2.VectorSearchRequest(
            collection_id=collection_id,
            queries=[search_query],
            top_k=10,
            include_fields=pb2.IncludeFields(metadata=True, score=True)
        )
        
        search_response = grpc_client.stub.VectorSearch(search_request)
        assert search_response.success, f"Search with filter failed: {search_response.error_message}"
        
        # Verify only electronics items returned
        assert search_response.compact_results is not None, "No compact results in response"
        for result in search_response.compact_results.results:
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
                metadata=[pb2.MetadataItem(key="batch", string_value=str(i // 10))]
            )
            for i in range(batch_size)
        ]
        
        batch_request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=vector_records
        )
        
        insert_response = grpc_client.stub.VectorBatch(batch_request)
        insert_time = time.time() - start_time
        
        assert insert_response.success, f"Batch insertion failed: {insert_response.error_message}"
        assert insert_response.metrics.total_processed == batch_size, \
            f"Expected {batch_size} vectors inserted, got {insert_response.metrics.total_processed}"
        
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
        # Get engine name from enum
        engine_name = pb2.StorageEngine.Name(storage_engine)
        
        # Create collection with specific engine
        collection_config = pb2.CollectionConfig(
            name=f"{test_collection_name}_{engine_name}",
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
            f"Collection creation failed for {engine_name}: {collection_response.error_message}"
        
        collection_id = collection_response.collection.id
        
        try:
            # Insert test vector
            vector_record = pb2.VectorRecord(
                id=f"engine_test_{engine_name}",
                vector=[1.0, 2.0, 3.0],
                metadata=[pb2.MetadataItem(key="engine", string_value=engine_name)]
            )
            
            batch_request = pb2.VectorBatchRequest(
                collection_id=collection_id,
                vectors=[vector_record]
            )
            
            insert_response = grpc_client.stub.VectorBatch(batch_request)
            assert insert_response.success, \
                f"Vector insertion failed for {storage_engine.name}: {insert_response.error_message}"
            
        finally:
            # Cleanup
            delete_request = pb2.CollectionRequest(
                operation=pb2.CollectionOperation.COLLECTION_DELETE,
                collection_id=collection_id
            )
            grpc_client.stub.CollectionOperation(delete_request)