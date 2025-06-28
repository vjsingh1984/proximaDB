"""
Comprehensive End-to-End SDK Test Suite
Tests complete ProximaDB functionality with BERT embeddings, multi-disk persistence,
ID-based search, filtered metadata search, and similarity search using both gRPC and REST.
"""

import pytest
import asyncio
import logging
import time
import numpy as np
import json
import uuid
from typing import List, Dict, Optional, Tuple
from pathlib import Path
import sys
import os
from sentence_transformers import SentenceTransformer

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient, Protocol
from proximadb.exceptions import ProximaDBError
from proximadb.models import CollectionConfig

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(name)s: %(message)s',
    datefmt='%Y-%m-%dT%H:%M:%S'
)
logger = logging.getLogger('comprehensive_sdk_test')


class BERTEmbeddingService:
    """BERT Embedding Service for test data generation"""
    
    def __init__(self, model_name: str = "all-MiniLM-L6-v2", cache_dir: str = "./embedding_cache"):
        self.model_name = model_name
        self.cache_dir = cache_dir
        self.model = None
        self.dimension = None
        self._load_model()
    
    def _load_model(self):
        """Load the BERT model"""
        try:
            self.model = SentenceTransformer(self.model_name)
            # Test with a sample to get dimension
            test_embedding = self.model.encode("test")
            self.dimension = len(test_embedding)
            logger.info(f"✅ BERT model loaded: {self.model_name} ({self.dimension}D)")
        except Exception as e:
            logger.error(f"❌ Failed to load BERT model: {e}")
            raise
    
    def encode(self, texts: List[str]) -> List[List[float]]:
        """Encode texts to embeddings"""
        if isinstance(texts, str):
            texts = [texts]
        
        embeddings = self.model.encode(texts, convert_to_numpy=True)
        return [embedding.tolist() for embedding in embeddings]
    
    def encode_single(self, text: str) -> List[float]:
        """Encode single text to embedding"""
        embedding = self.model.encode(text, convert_to_numpy=True)
        return embedding.tolist()


def create_sample_corpus(size: int = 1000) -> List[str]:
    """Create sample text corpus for testing"""
    themes = [
        "artificial intelligence and machine learning",
        "cloud computing and distributed systems", 
        "vector databases and similarity search",
        "natural language processing and transformers",
        "computer vision and image recognition",
        "blockchain technology and cryptocurrencies",
        "quantum computing and quantum algorithms",
        "cybersecurity and privacy protection",
        "software engineering and development",
        "data science and analytics"
    ]
    
    variations = [
        "is revolutionizing the tech industry",
        "has significant applications in business",
        "requires advanced mathematical concepts",
        "is becoming increasingly important",
        "faces challenges in implementation",
        "shows promising future developments",
        "needs careful ethical considerations",
        "demands skilled professionals",
        "transforms traditional workflows",
        "enables innovative solutions"
    ]
    
    corpus = []
    for i in range(size):
        theme = themes[i % len(themes)]
        variation = variations[i % len(variations)]
        text = f"Document {i+1}: {theme.title()} {variation} in modern applications."
        corpus.append(text)
    
    return corpus


@pytest.fixture(scope="session")
def bert_service():
    """Create BERT service for the test session"""
    return BERTEmbeddingService()


@pytest.fixture(params=[Protocol.GRPC, Protocol.REST])
def client(request):
    """Create client with both gRPC and REST protocols"""
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
                logger.info(f"✅ Server ready with {protocol.value} protocol")
                break
        except Exception as e:
            if i == max_retries - 1:
                pytest.skip(f"Server not available for {protocol.value} protocol: {e}")
            time.sleep(1)
    
    yield client
    client.close()


@pytest.fixture
def sample_collection_config():
    """Sample collection configuration"""
    return CollectionConfig(
        dimension=384,
        distance_metric="cosine",
        index_config={
            "index_type": "hnsw",
            "ef_construction": 200,
            "max_connections": 16
        }
    )


class TestComprehensiveSDK:
    """Comprehensive SDK test suite"""
    
    def test_server_health(self, client):
        """Test server health check"""
        health = client.health()
        assert health is not None
        logger.info(f"✅ Server health check passed for {client.active_protocol.value}")
    
    def test_protocol_performance_info(self, client):
        """Test protocol performance information"""
        perf_info = client.get_performance_info()
        assert "protocol" in perf_info
        assert "advantages" in perf_info
        logger.info(f"📊 Protocol info: {perf_info['protocol']}")
    
    def test_collection_crud_operations(self, client, sample_collection_config):
        """Test complete collection CRUD operations"""
        collection_name = f"test_collection_{uuid.uuid4().hex[:8]}"
        
        try:
            # Create collection
            collection = client.create_collection(
                name=collection_name,
                config=sample_collection_config
            )
            assert collection is not None
            assert collection.name == collection_name
            assert collection.dimension == 384
            logger.info(f"✅ Collection created: {collection_name}")
            
            # Get collection by name
            retrieved_collection = client.get_collection(collection_name)
            assert retrieved_collection is not None
            assert retrieved_collection.name == collection_name
            
            # Get collection by UUID
            if hasattr(collection, 'id') and collection.id:
                retrieved_by_uuid = client.get_collection(collection.id)
                assert retrieved_by_uuid is not None
                assert retrieved_by_uuid.name == collection_name  # Name should match
                logger.info(f"✅ Collection retrieved by UUID: {collection.id}")
            
            # List collections
            collections = client.list_collections()
            assert any(c.name == collection_name for c in collections)
            logger.info(f"✅ Collection found in list: {len(collections)} total")
            
            # Update collection
            updates = {"description": "Updated test collection"}
            updated_collection = client.update_collection(collection_name, updates)
            assert updated_collection is not None
            logger.info("✅ Collection updated successfully")
            
        finally:
            # Cleanup: Delete collection
            try:
                success = client.delete_collection(collection_name)
                assert success
                logger.info(f"✅ Collection deleted: {collection_name}")
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup collection: {e}")


class TestVectorOperations:
    """Test vector operations with BERT embeddings"""
    
    @pytest.fixture
    def test_collection(self, client, sample_collection_config):
        """Create a test collection for vector operations"""
        collection_name = f"vector_test_{uuid.uuid4().hex[:8]}"
        collection = client.create_collection(
            name=collection_name,
            config=sample_collection_config
        )
        yield collection
        
        # Cleanup
        try:
            client.delete_collection(collection_name)
        except Exception as e:
            logger.warning(f"⚠️ Failed to cleanup collection: {e}")
    
    def test_single_vector_operations(self, client, test_collection, bert_service):
        """Test single vector insert, get, update, delete"""
        collection_id = test_collection.name
        
        # Generate test embedding
        text = "Test document for single vector operations"
        embedding = bert_service.encode_single(text)
        vector_id = "test_vector_1"
        metadata = {
            "text": text,
            "category": "test",
            "timestamp": int(time.time()),
            "source": "unit_test"
        }
        
        try:
            # Insert vector
            result = client.insert_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                vector=embedding,
                metadata=metadata
            )
            assert result is not None
            logger.info(f"✅ Vector inserted: {vector_id}")
            
            # Get vector by ID
            retrieved_vector = client.get_vector(
                collection_id=collection_id,
                vector_id=vector_id,
                include_vector=True,
                include_metadata=True
            )
            assert retrieved_vector is not None
            assert retrieved_vector["id"] == vector_id
            assert "vector" in retrieved_vector
            assert "metadata" in retrieved_vector
            assert retrieved_vector["metadata"]["text"] == text
            logger.info("✅ Vector retrieved by ID successfully")
            
            # Search similar vectors
            search_results = client.search(
                collection_id=collection_id,
                query=embedding,
                k=5,
                include_vectors=True,
                include_metadata=True
            )
            assert len(search_results) >= 1
            assert search_results[0].id == vector_id
            assert search_results[0].score > 0.9  # Should be very similar to itself
            logger.info(f"✅ Similarity search found {len(search_results)} results")
            
        finally:
            # Delete vector
            try:
                delete_result = client.delete_vector(collection_id, vector_id)
                assert delete_result is not None
                logger.info(f"✅ Vector deleted: {vector_id}")
            except Exception as e:
                logger.warning(f"⚠️ Failed to delete vector: {e}")
    
    def test_batch_vector_operations(self, client, test_collection, bert_service):
        """Test batch vector operations"""
        collection_id = test_collection.name
        
        # Generate test data
        texts = [
            "Artificial intelligence is transforming technology",
            "Machine learning algorithms process vast datasets", 
            "Neural networks enable deep learning capabilities",
            "Vector databases store high-dimensional embeddings",
            "Similarity search finds nearest neighbors efficiently"
        ]
        
        embeddings = bert_service.encode(texts)
        vector_ids = [f"batch_vector_{i}" for i in range(len(texts))]
        metadata_list = [
            {
                "text": text,
                "category": "ai" if i < 3 else "database",
                "batch_id": "test_batch_1",
                "index": i
            }
            for i, text in enumerate(texts)
        ]
        
        try:
            # Batch insert
            batch_result = client.insert_vectors(
                collection_id=collection_id,
                vectors=embeddings,
                ids=vector_ids,
                metadata=metadata_list
            )
            assert batch_result is not None
            assert batch_result.inserted_count == len(vector_ids)
            logger.info(f"✅ Batch insert: {batch_result.inserted_count} vectors")
            
            # Test filtered search by category
            ai_results = client.search(
                collection_id=collection_id,
                query=embeddings[0],  # AI-related query
                k=10,
                filter={"category": "ai"},
                include_metadata=True
            )
            assert len(ai_results) == 3  # Should find 3 AI-related vectors
            for result in ai_results:
                assert result.metadata["category"] == "ai"
            logger.info(f"✅ Filtered search (AI category): {len(ai_results)} results")
            
            # Test filtered search by batch_id
            batch_results = client.search(
                collection_id=collection_id,
                query=embeddings[2],
                k=10,
                filter={"batch_id": "test_batch_1"},
                include_metadata=True
            )
            assert len(batch_results) == 5  # Should find all vectors in batch
            logger.info(f"✅ Filtered search (batch_id): {len(batch_results)} results")
            
            # Test multi-query search
            query_vectors = embeddings[:2]
            multi_results = client.multi_search(
                collection_id=collection_id,
                queries=query_vectors,
                k=3,
                include_metadata=True
            )
            assert len(multi_results) >= 2
            logger.info(f"✅ Multi-query search: {len(multi_results)} result sets")
            
        finally:
            # Cleanup: Delete all vectors
            try:
                delete_result = client.delete_vectors(collection_id, vector_ids)
                assert delete_result is not None
                logger.info(f"✅ Batch delete: {len(vector_ids)} vectors")
            except Exception as e:
                logger.warning(f"⚠️ Failed to delete vectors: {e}")


class TestLargeScaleOperations:
    """Test large-scale operations with 10MB+ data to trigger WAL flush"""
    
    @pytest.fixture
    def large_test_collection(self, client, sample_collection_config):
        """Create a test collection for large-scale operations"""
        collection_name = f"large_test_{uuid.uuid4().hex[:8]}"
        collection = client.create_collection(
            name=collection_name,
            config=sample_collection_config
        )
        yield collection
        
        # Cleanup
        try:
            client.delete_collection(collection_name)
            logger.info(f"✅ Large test collection cleanup: {collection_name}")
        except Exception as e:
            logger.warning(f"⚠️ Failed to cleanup large collection: {e}")
    
    def test_10mb_bert_embeddings(self, client, large_test_collection, bert_service):
        """Test inserting 10MB+ of BERT embeddings to trigger WAL flush"""
        collection_id = large_test_collection.name
        
        # Calculate vectors needed for 10MB
        # 384D float32 = 384 * 4 bytes = 1,536 bytes per vector
        # 10MB / 1,536 bytes ≈ 6,827 vectors
        target_size_mb = 10
        bytes_per_vector = 384 * 4  # float32
        target_vectors = int((target_size_mb * 1024 * 1024) / bytes_per_vector)
        batch_size = 100
        
        logger.info(f"🎯 Target: {target_vectors} vectors ({target_size_mb}MB)")
        
        # Generate large corpus
        corpus = create_sample_corpus(target_vectors)
        total_inserted = 0
        start_time = time.time()
        
        try:
            # Insert in batches
            for i in range(0, target_vectors, batch_size):
                batch_texts = corpus[i:i + batch_size]
                batch_embeddings = bert_service.encode(batch_texts)
                batch_ids = [f"large_vector_{j}" for j in range(i, i + len(batch_texts))]
                batch_metadata = [
                    {
                        "text": text,
                        "batch_index": i // batch_size,
                        "vector_index": j,
                        "category": f"category_{j % 10}",
                        "size_test": True
                    }
                    for j, text in enumerate(batch_texts, i)
                ]
                
                batch_result = client.insert_vectors(
                    collection_id=collection_id,
                    vectors=batch_embeddings,
                    ids=batch_ids,
                    metadata=batch_metadata
                )
                
                total_inserted += batch_result.inserted_count
                
                if (i // batch_size) % 10 == 0:  # Log every 10 batches
                    elapsed = time.time() - start_time
                    rate = total_inserted / elapsed if elapsed > 0 else 0
                    estimated_mb = (total_inserted * bytes_per_vector) / (1024 * 1024)
                    logger.info(f"📈 Progress: {total_inserted}/{target_vectors} vectors "
                              f"({estimated_mb:.1f}MB, {rate:.0f} vec/sec)")
            
            elapsed_time = time.time() - start_time
            actual_mb = (total_inserted * bytes_per_vector) / (1024 * 1024)
            logger.info(f"✅ Large-scale insert complete: {total_inserted} vectors "
                       f"({actual_mb:.1f}MB) in {elapsed_time:.1f}s")
            
            # Test search performance on large dataset
            search_start = time.time()
            query_embedding = bert_service.encode_single("artificial intelligence search query")
            
            search_results = client.search(
                collection_id=collection_id,
                query=query_embedding,
                k=20,
                include_metadata=True
            )
            
            search_elapsed = time.time() - search_start
            assert len(search_results) == 20
            logger.info(f"✅ Search on large dataset: {len(search_results)} results "
                       f"in {search_elapsed*1000:.1f}ms")
            
            # Test filtered search on large dataset
            filter_start = time.time()
            filtered_results = client.search(
                collection_id=collection_id,
                query=query_embedding,
                k=10,
                filter={"category": "category_5"},
                include_metadata=True
            )
            
            filter_elapsed = time.time() - filter_start
            assert len(filtered_results) > 0
            for result in filtered_results:
                assert result.metadata["category"] == "category_5"
            logger.info(f"✅ Filtered search on large dataset: {len(filtered_results)} results "
                       f"in {filter_elapsed*1000:.1f}ms")
            
        except Exception as e:
            logger.error(f"❌ Large-scale test failed: {e}")
            raise


class TestPersistenceAndRecovery:
    """Test persistence and recovery mechanisms"""
    
    def test_collection_persistence(self, client, sample_collection_config):
        """Test that collections persist across operations"""
        collection_name = f"persist_test_{uuid.uuid4().hex[:8]}"
        
        try:
            # Create collection
            collection = client.create_collection(
                name=collection_name,
                config=sample_collection_config
            )
            
            # Verify persistence by listing collections
            collections = client.list_collections()
            assert any(c.name == collection_name for c in collections)
            
            # Get collection UUID for later verification
            collection_uuid = None
            if hasattr(collection, 'id'):
                collection_uuid = collection.id
                logger.info(f"✅ Collection UUID: {collection_uuid}")
            
            # Verify UUID-based access if available
            if collection_uuid:
                uuid_collection = client.get_collection(collection_uuid)
                assert uuid_collection is not None
                assert uuid_collection.name == collection_name
                logger.info("✅ Collection accessible by UUID")
            
        finally:
            # Cleanup
            try:
                client.delete_collection(collection_name)
            except Exception as e:
                logger.warning(f"⚠️ Failed to cleanup persistence test: {e}")


def pytest_configure(config):
    """Configure pytest with custom markers"""
    config.addinivalue_line(
        "markers", "integration: mark test as integration test requiring running server"
    )
    config.addinivalue_line(
        "markers", "performance: mark test as performance test that may take longer"
    )
    config.addinivalue_line(
        "markers", "large_scale: mark test as large-scale test with significant data"
    )


# Mark all tests as integration tests
pytestmark = pytest.mark.integration


if __name__ == "__main__":
    # Run tests with coverage when executed directly
    pytest.main([
        __file__,
        "-v",
        "--cov=proximadb",
        "--cov-report=term-missing",
        "--cov-report=html:htmlcov",
        "-s"
    ])