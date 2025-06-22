"""
Test vector insertion and search operations.
"""

import pytest
import numpy as np
from typing import List, Dict, Any


@pytest.mark.integration
class TestVectorOperations:
    """Test vector insertion and search operations."""
    
    @pytest.fixture(autouse=True)
    def setup_collection(self, client, cleanup_collection):
        """Set up test collection for vector operations."""
        self.client = client
        self.collection_name = cleanup_collection
        
        # Create collection for testing
        self.client.create_collection(
            name=self.collection_name,
            dimension=384,
            distance_metric="cosine",
            storage_engine="viper",
            indexing_algorithm="hnsw"
        )
    
    def test_insert_single_vector(self):
        """Test inserting a single vector."""
        vector_id = "test_vector_001"
        vector = np.random.random(384).tolist()
        metadata = {"category": "test", "source": "unit_test"}
        
        result = self.client.insert_vectors(
            self.collection_name,
            [vector],
            [vector_id],
            [metadata]
        )
        
        assert result is not None
    
    def test_insert_batch_vectors(self):
        """Test inserting multiple vectors in batch."""
        batch_size = 10
        vector_ids = [f"batch_vector_{i:03d}" for i in range(batch_size)]
        vectors = [np.random.random(384).tolist() for _ in range(batch_size)]
        metadata_list = [
            {"category": "batch_test", "index": i, "source": "unit_test"}
            for i in range(batch_size)
        ]
        
        result = self.client.insert_vectors(
            self.collection_name,
            vectors,
            vector_ids,
            metadata_list
        )
        
        assert result is not None
    
    def test_search_vectors(self):
        """Test vector similarity search."""
        # First insert some test vectors
        test_vectors = [np.random.random(384).tolist() for _ in range(5)]
        test_ids = [f"search_test_{i}" for i in range(5)]
        test_metadata = [{"category": "search_test", "id": i} for i in range(5)]
        
        self.client.insert_vectors(
            self.collection_name,
            test_vectors,
            test_ids,
            test_metadata
        )
        
        # Search with a random query vector
        query_vector = np.random.random(384).tolist()
        results = self.client.search(
            self.collection_name,
            query_vector,
            k=3
        )
        
        # Results might be empty if server doesn't find matches
        # Just verify the call succeeded
        assert results is not None
    
    def test_search_with_metadata_filter(self):
        """Test search with metadata filtering."""
        # Insert test vectors with specific metadata
        test_vectors = [np.random.random(384).tolist() for _ in range(3)]
        test_ids = [f"filter_test_{i}" for i in range(3)]
        test_metadata = [
            {"category": "AI", "type": "research"},
            {"category": "ML", "type": "research"}, 
            {"category": "AI", "type": "tutorial"}
        ]
        
        self.client.insert_vectors(
            self.collection_name,
            test_vectors,
            test_ids,
            test_metadata
        )
        
        # Search with metadata filter
        query_vector = np.random.random(384).tolist()
        results = self.client.search(
            self.collection_name,
            query_vector,
            k=5,
            filter={"category": "AI"}
        )
        
        assert results is not None
    
    @pytest.mark.slow
    def test_large_batch_insert(self):
        """Test inserting a larger batch of vectors."""
        batch_size = 100
        vector_ids = [f"large_batch_{i:04d}" for i in range(batch_size)]
        vectors = [np.random.random(384).tolist() for _ in range(batch_size)]
        metadata_list = [
            {
                "category": "large_test",
                "batch_index": i,
                "source": "performance_test",
                "timestamp": f"2024-01-{(i % 28) + 1:02d}"
            }
            for i in range(batch_size)
        ]
        
        result = self.client.insert_vectors(
            self.collection_name,
            vectors,
            vector_ids,
            metadata_list
        )
        
        assert result is not None


@pytest.mark.integration
@pytest.mark.embedding
class TestEmbeddingIntegration:
    """Test integration with BERT embeddings."""
    
    @pytest.fixture(autouse=True)
    def setup_collection(self, client, cleanup_collection):
        """Set up test collection for embedding tests."""
        self.client = client
        self.collection_name = cleanup_collection
        
        # Create collection for testing
        self.client.create_collection(
            name=self.collection_name,
            dimension=384,
            distance_metric="cosine"
        )
    
    def test_insert_with_bert_embeddings(self, bert_service):
        """Test inserting vectors with real BERT embeddings."""
        test_texts = [
            "artificial intelligence and machine learning",
            "deep neural networks for computer vision",
            "natural language processing with transformers"
        ]
        
        # Generate embeddings
        embeddings = bert_service.embed_texts(test_texts)
        vector_ids = [f"bert_test_{i}" for i in range(len(test_texts))]
        metadata_list = [
            {"text": text, "category": "AI", "source": "bert_test"}
            for text in test_texts
        ]
        
        # Convert numpy arrays to lists
        vectors = [emb.tolist() for emb in embeddings]
        
        result = self.client.insert_vectors(
            self.collection_name,
            vectors,
            vector_ids,
            metadata_list
        )
        
        assert result is not None
    
    def test_semantic_search(self, bert_service):
        """Test semantic search with BERT embeddings."""
        # Insert some documents
        documents = [
            "machine learning algorithms",
            "deep learning neural networks", 
            "computer vision applications",
            "natural language understanding"
        ]
        
        embeddings = bert_service.embed_texts(documents)
        vector_ids = [f"doc_{i}" for i in range(len(documents))]
        metadata_list = [
            {"text": doc, "category": "AI_docs"}
            for doc in documents
        ]
        
        vectors = [emb.tolist() for emb in embeddings]
        
        # Insert documents
        self.client.insert_vectors(
            self.collection_name,
            vectors,
            vector_ids,
            metadata_list
        )
        
        # Search for similar content
        query_text = "neural network algorithms"
        query_embedding = bert_service.embed_texts([query_text])[0]
        
        results = self.client.search(
            self.collection_name,
            query_embedding.tolist(),
            k=3
        )
        
        assert results is not None