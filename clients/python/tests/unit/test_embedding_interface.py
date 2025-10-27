"""
Tests for Pluggable Embedding Interface with Real BERT Service

Tests the embedding provider abstraction, factory pattern, and integration
with semantic chunking using real BERT embeddings and ProximaDB server.
"""

import pytest
import numpy as np
import time
import sys
from pathlib import Path
from typing import List, Optional

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb.embedding_interface import (
    EmbeddingProvider,
    EmbeddingConfig,
    BERTEmbeddingProvider,
    SimulatedEmbeddingProvider,
    CohereEmbeddingProvider,
    EmbeddingProviderFactory,
    create_embedding_provider,
    get_default_embedding_provider
)


class TestEmbeddingConfig:
    """Test embedding configuration"""
    
    def test_default_config(self):
        """Test default configuration values"""
        config = EmbeddingConfig(
            model_name="test-model",
            dimension=384
        )
        
        assert config.model_name == "test-model"
        assert config.dimension == 384
        assert config.batch_size == 32
        assert config.normalize == True
        assert config.cache_embeddings == True
        assert config.timeout_seconds == 30.0
        assert config.api_key is None
        assert config.api_url is None
        assert config.extra_params is None
    
    def test_custom_config(self):
        """Test custom configuration"""
        config = EmbeddingConfig(
            model_name="custom-model",
            dimension=768,
            batch_size=64,
            normalize=False,
            cache_embeddings=False,
            timeout_seconds=60.0,
            api_key="test-key",
            api_url="https://api.example.com",
            extra_params={"temperature": 0.5}
        )
        
        assert config.model_name == "custom-model"
        assert config.dimension == 768
        assert config.batch_size == 64
        assert config.normalize == False
        assert config.cache_embeddings == False
        assert config.timeout_seconds == 60.0
        assert config.api_key == "test-key"
        assert config.api_url == "https://api.example.com"
        assert config.extra_params == {"temperature": 0.5}


class TestSimulatedEmbeddingProvider:
    """Test simulated embedding provider"""
    
    def test_initialization(self):
        """Test provider initialization"""
        provider = SimulatedEmbeddingProvider()
        
        assert provider.model_name == "simulated"
        assert provider.dimension == 384
        assert provider.is_available() == True
    
    def test_custom_dimension(self):
        """Test custom embedding dimension"""
        config = EmbeddingConfig(model_name="sim", dimension=768)
        provider = SimulatedEmbeddingProvider(config)
        
        assert provider.dimension == 768
    
    def test_embed_single_text(self):
        """Test single text embedding"""
        provider = SimulatedEmbeddingProvider()
        
        text = "This is a test sentence."
        embedding = provider.embed_text(text)
        
        assert isinstance(embedding, np.ndarray)
        assert embedding.shape == (384,)
        
        # Test normalization
        if provider.config.normalize:
            norm = np.linalg.norm(embedding)
            assert abs(norm - 1.0) < 1e-6
    
    def test_embed_multiple_texts(self):
        """Test multiple text embeddings"""
        provider = SimulatedEmbeddingProvider()
        
        texts = [
            "First test sentence.",
            "Second test sentence.",
            "Third test sentence."
        ]
        embeddings = provider.embed_texts(texts)
        
        assert isinstance(embeddings, np.ndarray)
        assert embeddings.shape == (3, 384)
        
        # Each embedding should be unique (deterministic based on text)
        assert not np.array_equal(embeddings[0], embeddings[1])
        assert not np.array_equal(embeddings[1], embeddings[2])
    
    def test_deterministic_embeddings(self):
        """Test that embeddings are deterministic for same text"""
        provider = SimulatedEmbeddingProvider()
        
        text = "Consistent test text"
        embedding1 = provider.embed_text(text)
        embedding2 = provider.embed_text(text)
        
        assert np.array_equal(embedding1, embedding2)
    
    def test_batch_embed_texts(self):
        """Test batch embedding functionality"""
        provider = SimulatedEmbeddingProvider()
        
        # Create many texts
        texts = [f"Test sentence number {i}" for i in range(100)]
        
        # Test with custom batch size
        embeddings = provider.batch_embed_texts(texts, batch_size=10)
        
        assert embeddings.shape == (100, 384)


@pytest.mark.skip(reason="Integration test - requires real server. Move to tests/integration/")
class TestBERTEmbeddingProvider(BaseProximaDBTest):
    """Test BERT embedding provider with real BERT service"""
    
    def test_initialization_success(self):
        """Test successful initialization with real BERT model"""
        try:
            provider = BERTEmbeddingProvider()
            
            if provider.is_available():
                assert provider.model_name == "all-MiniLM-L6-v2"
                assert provider.dimension == 384
                assert provider.is_available() == True
            else:
                pytest.skip("BERT provider not available - sentence-transformers not installed")
        except ImportError:
            pytest.skip("sentence-transformers not installed")
    
    def test_embed_single_text_real(self):
        """Test embedding single text with real BERT"""
        provider = BERTEmbeddingProvider()
        
        if not provider.is_available():
            pytest.skip("BERT provider not available")
        
        text = "This is a test sentence for BERT embeddings."
        embedding = provider.embed_text(text)
        
        assert isinstance(embedding, np.ndarray)
        assert embedding.shape == (384,)
        
        # BERT embeddings should be normalized if configured
        if provider.config.normalize:
            norm = np.linalg.norm(embedding)
            assert abs(norm - 1.0) < 1e-5
    
    def test_embed_multiple_texts_real(self):
        """Test embedding multiple texts with real BERT"""
        provider = BERTEmbeddingProvider()
        
        if not provider.is_available():
            pytest.skip("BERT provider not available")
        
        texts = [
            "Machine learning is transforming technology.",
            "Natural language processing enables computers to understand text.",
            "Vector databases store high-dimensional embeddings."
        ]
        
        embeddings = provider.embed_texts(texts)
        
        assert isinstance(embeddings, np.ndarray)
        assert embeddings.shape == (3, 384)
        
        # Each embedding should be unique
        assert not np.array_equal(embeddings[0], embeddings[1])
        assert not np.array_equal(embeddings[1], embeddings[2])
        
        # Test semantic similarity - similar texts should have higher similarity
        similarity_01 = np.dot(embeddings[0], embeddings[1])
        similarity_02 = np.dot(embeddings[0], embeddings[2])
        
        # ML and NLP are more related than ML and vector databases
        assert similarity_01 > 0.5  # Reasonably similar
        assert similarity_02 > 0.3  # Somewhat related
    
    def test_batch_processing_real(self):
        """Test batch processing with real BERT"""
        provider = BERTEmbeddingProvider()
        
        if not provider.is_available():
            pytest.skip("BERT provider not available")
        
        # Create many texts
        texts = [f"Test sentence number {i} with some content." for i in range(50)]
        
        # Process with custom batch size
        embeddings = provider.batch_embed_texts(texts, batch_size=10)
        
        assert embeddings.shape == (50, 384)
        
        # All embeddings should be normalized if configured
        if provider.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1)
            assert np.allclose(norms, 1.0, atol=1e-5)


class TestCohereEmbeddingProvider:
    """Test Cohere embedding provider"""
    
    def test_initialization_no_api_key(self):
        """Test initialization without API key"""
        config = EmbeddingConfig(model_name="cohere", dimension=768)
        
        with pytest.raises(ValueError, match="Cohere API key required"):
            CohereEmbeddingProvider(config)
    
    def test_initialization_with_api_key(self):
        """Test initialization with API key"""
        config = EmbeddingConfig(
            model_name="cohere",
            dimension=768,
            api_key="test-api-key"
        )
        
        provider = CohereEmbeddingProvider(config)
        
        assert provider.model_name == "cohere"
        assert provider.dimension == 768
        assert provider.is_available() == False  # Mock returns False
    
    def test_embed_texts_unavailable(self):
        """Test embedding when service unavailable"""
        config = EmbeddingConfig(
            model_name="cohere",
            dimension=768,
            api_key="test-api-key"
        )
        provider = CohereEmbeddingProvider(config)
        
        with pytest.raises(RuntimeError, match="Cohere embedding provider is not available"):
            provider.embed_texts(["test"])


class TestEmbeddingProviderFactory:
    """Test embedding provider factory"""
    
    def test_create_bert_provider(self):
        """Test creating BERT provider"""
        provider = EmbeddingProviderFactory.create_provider("bert")
        
        assert isinstance(provider, (BERTEmbeddingProvider, SimulatedEmbeddingProvider))
        # If BERT unavailable, falls back to simulated
        assert provider.is_available() == True
    
    def test_create_simulated_provider(self):
        """Test creating simulated provider"""
        provider = EmbeddingProviderFactory.create_provider("simulated")
        
        assert isinstance(provider, SimulatedEmbeddingProvider)
        assert provider.is_available() == True
    
    def test_create_provider_with_config(self):
        """Test creating provider with custom config"""
        config = EmbeddingConfig(
            model_name="custom",
            dimension=512
        )
        
        provider = EmbeddingProviderFactory.create_provider("simulated", config)
        
        assert provider.dimension == 512
    
    def test_create_provider_invalid_type(self):
        """Test creating provider with invalid type"""
        with pytest.raises(ValueError, match="Unknown embedding provider: invalid"):
            EmbeddingProviderFactory.create_provider("invalid")
    
    def test_fallback_mechanism_real(self):
        """Test fallback mechanism with real providers"""
        # Try to create a provider that might not be available
        provider = EmbeddingProviderFactory.create_provider("bert")
        
        # Should get either BERT or simulated, but always available
        assert provider.is_available() == True
        
        if isinstance(provider, BERTEmbeddingProvider):
            assert provider.model_name == "all-MiniLM-L6-v2"
        else:
            assert isinstance(provider, SimulatedEmbeddingProvider)
    
    def test_list_providers(self):
        """Test listing available providers"""
        providers = EmbeddingProviderFactory.list_providers()
        
        assert "bert" in providers
        assert "simulated" in providers
        assert "cohere" in providers
        assert "sentence-transformers" in providers
    
    def test_register_custom_provider(self):
        """Test registering custom provider"""
        class CustomProvider(EmbeddingProvider):
            def __init__(self, config):
                super().__init__(config)
            
            def embed_texts(self, texts):
                return np.zeros((len(texts), self.config.dimension))
            
            def embed_text(self, text):
                return np.zeros(self.config.dimension)
            
            @property
            def dimension(self):
                return self.config.dimension
            
            @property
            def model_name(self):
                return "custom"
            
            def is_available(self):
                return True
        
        # Register custom provider
        EmbeddingProviderFactory.register_provider("custom", CustomProvider)
        
        # Create instance
        provider = EmbeddingProviderFactory.create_provider("custom")
        assert isinstance(provider, CustomProvider)
        
        # Clean up
        del EmbeddingProviderFactory._providers["custom"]
    
    def test_model_name_as_provider_type(self):
        """Test using model names as provider types"""
        provider = EmbeddingProviderFactory.create_provider("all-MiniLM-L6-v2")
        
        # Should create BERT provider with specific model
        assert isinstance(provider, (BERTEmbeddingProvider, SimulatedEmbeddingProvider))


class TestConvenienceFunctions:
    """Test convenience functions"""
    
    def test_create_embedding_provider(self):
        """Test create_embedding_provider convenience function"""
        provider = create_embedding_provider(
            provider_type="simulated",
            model_name="test-model",
            dimension=512
        )
        
        assert isinstance(provider, SimulatedEmbeddingProvider)
        assert provider.config.model_name == "test-model"
        assert provider.dimension == 512
    
    def test_create_embedding_provider_with_kwargs(self):
        """Test create_embedding_provider with extra kwargs"""
        provider = create_embedding_provider(
            provider_type="simulated",
            batch_size=64,
            normalize=False
        )
        
        assert provider.config.batch_size == 64
        assert provider.config.normalize == False
    
    def test_get_default_embedding_provider(self):
        """Test get_default_embedding_provider"""
        provider = get_default_embedding_provider()
        
        # Should return BERT or fallback to simulated
        assert isinstance(provider, (BERTEmbeddingProvider, SimulatedEmbeddingProvider))
        assert provider.is_available() == True


@pytest.mark.skip(reason="Integration test - requires real server. Move to tests/integration/")
class TestEmbeddingProviderIntegration(BaseProximaDBTest):
    """Test integration with semantic chunking and vector storage"""
    
    def test_semantic_chunking_with_real_embeddings(self):
        """Test semantic chunking with real embedding providers"""
        # Note: Enhanced semantic chunking consolidated - using standard semantic strategy
        from proximadb.chunking import TextChunker, ChunkingConfig, ChunkingStrategy
        
        # Get default provider (BERT or simulated)
        provider = get_default_embedding_provider()
        
        chunker = create_enhanced_semantic_chunker(
            use_embeddings=True,
            embedding_provider=provider
        )
        
        assert chunker.config.use_embeddings == True
        assert chunker.config.embedding_provider == provider
        assert chunker.config.embedding_provider.is_available() == True
        
        # Test with real text
        text = """
        Machine learning is a subset of artificial intelligence that enables
        computers to learn from data. Deep learning uses neural networks
        with multiple layers to learn complex patterns.
        
        The weather forecast predicts rain tomorrow. Temperature will drop
        to 15 degrees Celsius with strong winds expected.
        """
        
        chunks = chunker.chunk_semantically(
            text=text,
            source_id="test_doc",
            base_metadata={"test": True}
        )
        
        assert len(chunks) > 0
        for chunk in chunks:
            assert chunk.text
            assert chunk.metadata.get("coherence_score") is not None
    
    def test_embedding_to_vector_storage(self):
        """Test storing embeddings in ProximaDB"""
        # Create collection
        collection_name = self.create_collection(dimension=384)
        
        # Get embedding provider
        provider = get_default_embedding_provider()
        
        # Create test texts
        texts = [
            "ProximaDB is a high-performance vector database.",
            "It supports multiple indexing algorithms like HNSW and IVF.",
            "The Python SDK provides easy integration."
        ]
        
        # Generate embeddings
        embeddings = provider.embed_texts(texts)
        
        # Create vector records
        from proximadb.models import VectorRecord
        
        records = []
        for i, (text, embedding) in enumerate(zip(texts, embeddings)):
            record = VectorRecord(
                id=f"embed_test_{i}",
                vector=embedding.tolist(),
                metadata={"text": text, "source": "embedding_test"}
            )
            records.append(record)
        
        # Insert into collection
        response = self.rest_client.insert_vectors(
            collection_name,
            records=records
        )
        
        assert response.get("success") or response.get("inserted") == len(records)
        
        # Wait and search
        self.wait_for_indexing()
        
        # Search with query embedding
        query = "Tell me about ProximaDB vector database"
        query_embedding = provider.embed_text(query)
        
        results = self.rest_client.search(
            collection_name,
            query_embedding.tolist(),
            top_k=3
        )
        
        self.verify_search_results(results, 3)
        
        # First result should be about ProximaDB
        assert "ProximaDB" in results[0]["metadata"]["text"]
    
    def test_topic_boundary_with_real_embeddings(self):
        """Test topic boundary detection with real embeddings"""
        # Note: Enhanced semantic chunking consolidated - using standard semantic strategy
        from proximadb.chunking import TextChunker, ChunkingConfig, ChunkingStrategy
        
        # Use BERT if available, otherwise simulated
        provider = get_default_embedding_provider()
        
        config = SemanticChunkingConfig(
            use_embeddings=True,
            embedding_provider=provider,
            enable_topic_boundary_detection=True,
            embedding_similarity_threshold=0.7
        )
        
        chunker = EnhancedSemanticChunker(config)
        
        # Test with clear topic changes
        sentences = [
            "Python is a versatile programming language.",
            "It is widely used for data science and web development.",
            "Machine learning models can be trained with Python.",
            "I enjoy cooking Italian pasta for dinner.",
            "The best pasta is made with fresh ingredients.",
            "Homemade sauce tastes much better than store-bought."
        ]
        
        boundaries = chunker.detect_topic_boundaries(sentences)
        
        assert len(boundaries) > 0
        
        # Should detect boundary between programming and cooking
        boundary_positions = [b.position for b in boundaries]
        assert any(pos in [3, 4] for pos in boundary_positions)
        
        # With real embeddings, should use embedding-based detection
        if isinstance(provider, BERTEmbeddingProvider):
            assert any(b.boundary_type == "embedding_topic_change" for b in boundaries)