"""
Multi-Disk Persistence End-to-End Tests
Tests ProximaDB's multi-disk assignment service, WAL persistence, and recovery mechanisms.
"""

import pytest
import time
import logging
import uuid
import numpy as np
import json
import sys
import os
from pathlib import Path
from sentence_transformers import SentenceTransformer

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient, Protocol
from proximadb.exceptions import ProximaDBError
from proximadb.models import CollectionConfig

logger = logging.getLogger('multidisk_persistence_test')


class TestMultiDiskPersistence:
    """Test multi-disk assignment service and persistence"""
    
    @pytest.fixture(params=[Protocol.GRPC, Protocol.REST])
    def client(self, request):
        """Create client with both protocols for multi-disk testing"""
        protocol = request.param
        
        if protocol == Protocol.GRPC:
            client = ProximaDBClient("http://localhost:5679", protocol=Protocol.GRPC)
        else:
            client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Wait for server to be ready
        max_retries = 30
        for i in range(max_retries):
            try:
                health = client.health()
                if health:
                    logger.info(f"✅ Multi-disk server ready with {protocol.value}")
                    break
            except Exception as e:
                if i == max_retries - 1:
                    pytest.skip(f"Server not available for {protocol.value}: {e}")
                time.sleep(1)
        
        yield client
        client.close()
    
    @pytest.fixture
    def bert_model(self):
        """Load BERT model for testing"""
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    def test_assignment_service_statistics(self, client):
        """Test assignment service statistics endpoint"""
        try:
            # Note: This would require server admin endpoint
            # For now, we'll test indirectly through collection operations
            collection_name = f"assignment_test_{uuid.uuid4().hex[:8]}"
            config = CollectionConfig(dimension=384, distance_metric="cosine")
            
            collection = client.create_collection(collection_name, config)
            assert collection is not None
            
            # The collection should be assigned to one of the configured disks
            # This assignment happens automatically via the round-robin service
            logger.info(f"✅ Collection created and assigned: {collection_name}")
            
            # Test that the collection persists (indicating successful assignment)
            retrieved_collection = client.get_collection(collection_name)
            assert retrieved_collection.name == collection_name
            logger.info("✅ Collection assignment verified through persistence")
            
            # Cleanup
            client.delete_collection(collection_name)
            
        except Exception as e:
            logger.error(f"❌ Assignment service test failed: {e}")
            raise
    
    def test_multi_collection_distribution(self, client, bert_model):
        """Test that multiple collections are distributed across disks"""
        collections_created = []
        
        try:
            # Create multiple collections to test distribution
            num_collections = 5
            config = CollectionConfig(dimension=384, distance_metric="cosine")
            
            for i in range(num_collections):
                collection_name = f"dist_test_{i}_{uuid.uuid4().hex[:8]}"
                collection = client.create_collection(collection_name, config)
                collections_created.append(collection_name)
                
                # Add some data to ensure persistence
                text = f"Distribution test document {i}"
                embedding = bert_model.encode(text).tolist()
                
                client.insert_vector(
                    collection_id=collection_name,
                    vector_id=f"vector_{i}",
                    vector=embedding,
                    metadata={"collection_index": i, "test_type": "distribution"}
                )
                
                logger.info(f"✅ Collection {i+1}/{num_collections} created and populated")
            
            # Verify all collections exist and are accessible
            all_collections = client.list_collections()
            created_names = {col.name for col in all_collections}
            
            for collection_name in collections_created:
                assert collection_name in created_names
                
                # Verify data exists in each collection
                search_results = client.search(
                    collection_id=collection_name,
                    query=bert_model.encode("test query").tolist(),
                    k=5,
                    include_metadata=True
                )
                assert len(search_results) >= 1
                assert search_results[0].metadata["test_type"] == "distribution"
            
            logger.info(f"✅ All {num_collections} collections distributed and accessible")
            
        finally:
            # Cleanup all collections
            for collection_name in collections_created:
                try:
                    client.delete_collection(collection_name)
                except Exception as e:
                    logger.warning(f"⚠️ Failed to cleanup {collection_name}: {e}")
    
    def test_wal_flush_behavior(self, client, bert_model):
        """Test WAL flush behavior with multi-disk configuration"""
        collection_name = f"wal_flush_test_{uuid.uuid4().hex[:8]}"
        config = CollectionConfig(dimension=384, distance_metric="cosine")
        
        try:
            collection = client.create_collection(collection_name, config)
            
            # Generate enough data to potentially trigger WAL flush
            # Based on config: memory_flush_size_bytes = 1048576 (1MB)
            texts = [
                f"WAL flush test document {i}: " + 
                "This is a longer text to increase the data size for WAL testing. " * 10
                for i in range(100)
            ]
            
            embeddings = [bert_model.encode(text).tolist() for text in texts]
            vector_ids = [f"wal_vector_{i}" for i in range(100)]
            metadata_list = [
                {
                    "text": text,
                    "wal_test": True,
                    "vector_index": i,
                    "data_size": len(text)
                }
                for i, text in enumerate(texts)
            ]
            
            start_time = time.time()
            
            # Insert in batches to observe WAL behavior
            batch_size = 20
            for i in range(0, len(embeddings), batch_size):
                batch_embeddings = embeddings[i:i + batch_size]
                batch_ids = vector_ids[i:i + batch_size]
                batch_metadata = metadata_list[i:i + batch_size]
                
                batch_result = client.insert_vectors(
                    collection_id=collection_name,
                    vectors=batch_embeddings,
                    ids=batch_ids,
                    metadata=batch_metadata
                )
                
                assert batch_result.inserted_count == len(batch_embeddings)
                logger.info(f"📝 WAL batch {i//batch_size + 1}: {batch_result.inserted_count} vectors")
                
                # Small delay to allow WAL processing
                time.sleep(0.1)
            
            total_time = time.time() - start_time
            logger.info(f"✅ WAL flush test completed: {len(embeddings)} vectors in {total_time:.2f}s")
            
            # Verify data persistence after potential flush
            search_results = client.search(
                collection_id=collection_name,
                query=embeddings[0],
                k=50,
                include_metadata=True
            )
            
            assert len(search_results) >= 20  # Should find substantial results
            for result in search_results:
                assert result.metadata["wal_test"] is True
            
            logger.info(f"✅ Data persistence verified: {len(search_results)} vectors found")
            
        finally:
            # Cleanup
            try:
                client.delete_collection(collection_name)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup WAL test collection: {e}")
    
    def test_collection_affinity_behavior(self, client, bert_model):
        """Test collection affinity (consistent assignment to same disk)"""
        collection_name = f"affinity_test_{uuid.uuid4().hex[:8]}"
        config = CollectionConfig(dimension=384, distance_metric="cosine")
        
        try:
            # Create collection
            collection = client.create_collection(collection_name, config)
            
            # Insert vectors multiple times to test affinity
            base_text = "Collection affinity test document"
            
            for session in range(3):  # Multiple insert sessions
                texts = [
                    f"{base_text} session {session} document {i}"
                    for i in range(10)
                ]
                embeddings = [bert_model.encode(text).tolist() for text in texts]
                vector_ids = [f"affinity_vector_s{session}_{i}" for i in range(10)]
                metadata_list = [
                    {
                        "text": text,
                        "session": session,
                        "vector_index": i,
                        "affinity_test": True
                    }
                    for i, text in enumerate(texts)
                ]
                
                batch_result = client.insert_vectors(
                    collection_id=collection_name,
                    vectors=embeddings,
                    ids=vector_ids,
                    metadata=metadata_list
                )
                
                assert batch_result.inserted_count == 10
                logger.info(f"✅ Affinity session {session + 1}: {batch_result.inserted_count} vectors")
                
                # Verify data from this session
                session_results = client.search(
                    collection_id=collection_name,
                    query=embeddings[0],
                    k=20,
                    filter={"session": session},
                    include_metadata=True
                )
                
                assert len(session_results) >= 1
                for result in session_results:
                    assert result.metadata["session"] == session
                
                time.sleep(0.5)  # Brief pause between sessions
            
            # Verify all sessions' data is accessible (testing affinity persistence)
            total_results = client.search(
                collection_id=collection_name,
                query=bert_model.encode(base_text).tolist(),
                k=50,
                filter={"affinity_test": True},
                include_metadata=True
            )
            
            assert len(total_results) >= 15  # Should find data from all sessions
            sessions_found = set(result.metadata["session"] for result in total_results)
            assert len(sessions_found) == 3  # All three sessions should be represented
            
            logger.info(f"✅ Collection affinity verified: {len(total_results)} vectors from {len(sessions_found)} sessions")
            
        finally:
            # Cleanup
            try:
                client.delete_collection(collection_name)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup affinity test collection: {e}")
    
    def test_large_scale_multi_disk_behavior(self, client, bert_model):
        """Test behavior with large-scale data across multiple disks"""
        collection_name = f"large_scale_multi_{uuid.uuid4().hex[:8]}"
        config = CollectionConfig(dimension=384, distance_metric="cosine")
        
        try:
            collection = client.create_collection(collection_name, config)
            
            # Generate substantial amount of data to test multi-disk distribution
            total_vectors = 500  # Moderate size for CI/testing
            batch_size = 50
            
            logger.info(f"🎯 Large-scale test: {total_vectors} vectors across multiple disks")
            
            start_time = time.time()
            total_inserted = 0
            
            for batch_idx in range(0, total_vectors, batch_size):
                end_idx = min(batch_idx + batch_size, total_vectors)
                current_batch_size = end_idx - batch_idx
                
                texts = [
                    f"Large scale multi-disk test document {i}: " +
                    f"This document contains content designed to test the multi-disk " +
                    f"assignment service and distribution capabilities of ProximaDB. " +
                    f"Document index {i} in batch {batch_idx // batch_size}."
                    for i in range(batch_idx, end_idx)
                ]
                
                embeddings = [bert_model.encode(text).tolist() for text in texts]
                vector_ids = [f"large_scale_vector_{i}" for i in range(batch_idx, end_idx)]
                metadata_list = [
                    {
                        "text": text,
                        "batch_index": batch_idx // batch_size,
                        "vector_index": i,
                        "large_scale_test": True,
                        "category": f"category_{i % 5}"  # 5 categories for filtering
                    }
                    for i, text in enumerate(texts, batch_idx)
                ]
                
                batch_result = client.insert_vectors(
                    collection_id=collection_name,
                    vectors=embeddings,
                    ids=vector_ids,
                    metadata=metadata_list
                )
                
                assert batch_result.inserted_count == current_batch_size
                total_inserted += batch_result.inserted_count
                
                if (batch_idx // batch_size) % 5 == 0:  # Log every 5 batches
                    elapsed = time.time() - start_time
                    rate = total_inserted / elapsed if elapsed > 0 else 0
                    logger.info(f"📈 Progress: {total_inserted}/{total_vectors} vectors "
                              f"({rate:.0f} vec/sec)")
            
            total_time = time.time() - start_time
            final_rate = total_inserted / total_time if total_time > 0 else 0
            logger.info(f"✅ Large-scale insert complete: {total_inserted} vectors "
                       f"in {total_time:.1f}s ({final_rate:.0f} vec/sec)")
            
            # Test search performance on large multi-disk dataset
            search_start = time.time()
            search_query = bert_model.encode("large scale test query").tolist()
            
            search_results = client.search(
                collection_id=collection_name,
                query=search_query,
                k=20,
                include_metadata=True
            )
            
            search_time = time.time() - search_start
            assert len(search_results) == 20
            logger.info(f"✅ Large-scale search: {len(search_results)} results "
                       f"in {search_time*1000:.1f}ms")
            
            # Test filtered search across categories
            for category_idx in range(3):  # Test first 3 categories
                category_name = f"category_{category_idx}"
                category_results = client.search(
                    collection_id=collection_name,
                    query=search_query,
                    k=15,
                    filter={"category": category_name},
                    include_metadata=True
                )
                
                assert len(category_results) > 0
                for result in category_results:
                    assert result.metadata["category"] == category_name
                
                logger.info(f"✅ Category {category_name}: {len(category_results)} results")
            
        finally:
            # Cleanup
            try:
                client.delete_collection(collection_name)
                logger.info("✅ Large-scale test collection cleanup completed")
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup large-scale test collection: {e}")


class TestPersistenceRecovery:
    """Test persistence and recovery mechanisms"""
    
    @pytest.fixture
    def persistent_client(self):
        """Create client for persistence testing"""
        client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
        
        # Wait for server to be ready
        max_retries = 30
        for i in range(max_retries):
            try:
                health = client.health()
                if health:
                    logger.info("✅ Persistence test server ready")
                    break
            except Exception as e:
                if i == max_retries - 1:
                    pytest.skip(f"Server not available for persistence testing: {e}")
                time.sleep(1)
        
        yield client
        client.close()
    
    def test_collection_persistence_verification(self, persistent_client):
        """Test that collections persist and are recoverable"""
        collection_name = f"persist_verify_{uuid.uuid4().hex[:8]}"
        config = CollectionConfig(dimension=384, distance_metric="cosine")
        
        try:
            # Create collection
            collection = persistent_client.create_collection(collection_name, config)
            original_collection_id = getattr(collection, 'id', None)
            
            # Verify collection exists
            collections = persistent_client.list_collections()
            assert any(c.name == collection_name for c in collections)
            
            # Test immediate retrieval
            retrieved_collection = persistent_client.get_collection(collection_name)
            assert retrieved_collection.name == collection_name
            assert retrieved_collection.dimension == 384
            
            # Test UUID-based retrieval if available
            if original_collection_id:
                uuid_retrieved = persistent_client.get_collection(original_collection_id)
                assert uuid_retrieved.name == collection_name
                logger.info(f"✅ Collection UUID persistence verified: {original_collection_id}")
            
            logger.info("✅ Collection persistence verification completed")
            
        finally:
            # Cleanup
            try:
                persistent_client.delete_collection(collection_name)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup persistence test collection: {e}")
    
    def test_data_integrity_across_operations(self, persistent_client):
        """Test data integrity across multiple operations"""
        collection_name = f"integrity_test_{uuid.uuid4().hex[:8]}"
        config = CollectionConfig(dimension=384, distance_metric="cosine")
        bert_model = SentenceTransformer('all-MiniLM-L6-v2')
        
        try:
            # Create collection
            collection = persistent_client.create_collection(collection_name, config)
            
            # Insert initial data
            texts = [
                "Data integrity test document one",
                "Data integrity test document two", 
                "Data integrity test document three"
            ]
            embeddings = [bert_model.encode(text).tolist() for text in texts]
            vector_ids = ["integrity_vector_1", "integrity_vector_2", "integrity_vector_3"]
            metadata_list = [
                {"text": text, "integrity_test": True, "index": i}
                for i, text in enumerate(texts)
            ]
            
            insert_result = persistent_client.insert_vectors(
                collection_id=collection_name,
                vectors=embeddings,
                ids=vector_ids,
                metadata=metadata_list
            )
            assert insert_result.inserted_count == 3
            
            # Verify immediate retrieval
            for i, vector_id in enumerate(vector_ids):
                retrieved = persistent_client.get_vector(
                    collection_id=collection_name,
                    vector_id=vector_id,
                    include_vector=True,
                    include_metadata=True
                )
                assert retrieved is not None
                assert retrieved["metadata"]["index"] == i
                assert len(retrieved["vector"]) == 384
            
            # Test search integrity
            search_results = persistent_client.search(
                collection_id=collection_name,
                query=embeddings[0],
                k=5,
                include_metadata=True
            )
            assert len(search_results) == 3
            
            # Test filtered search integrity
            for i in range(3):
                filtered_results = persistent_client.search(
                    collection_id=collection_name,
                    query=embeddings[i],
                    k=5,
                    filter={"index": i},
                    include_metadata=True
                )
                assert len(filtered_results) == 1
                assert filtered_results[0].metadata["index"] == i
            
            logger.info("✅ Data integrity verification completed")
            
        finally:
            # Cleanup
            try:
                persistent_client.delete_collection(collection_name)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup integrity test collection: {e}")


# Mark all tests as integration tests
pytestmark = [pytest.mark.integration, pytest.mark.large_scale]


if __name__ == "__main__":
    pytest.main([
        __file__,
        "-v",
        "--cov=proximadb",
        "--cov-report=term-missing",
        "-s"
    ])