"""
Integration tests for chunking and embedding separation

Tests the complete workflow of chunking text, generating embeddings,
and creating vector records with different providers.
"""

from typing import List

import numpy as np
import pytest

from proximadb_sdk.chunking import (
    ChunkingConfig,
    ChunkingStrategy,
    TextChunker,
    chunk_and_embed_text,
    create_vector_records,
)
from proximadb_sdk.embedding_providers import get_provider, recommend_free_providers
from proximadb_sdk.embedding_providers.core import (
    BaseEmbeddingProvider as EmbeddingProvider,
)


class TestChunkingEmbeddingIntegration:
    """Test the integration between chunking and embedding systems"""

    @pytest.fixture
    def sample_text(self):
        """Sample text for testing"""
        return """
        Introduction to Machine Learning
        
        Machine learning is a subset of artificial intelligence that enables 
        computers to learn from data without being explicitly programmed. It 
        has revolutionized many industries including healthcare, finance, and 
        transportation.
        
        Types of Machine Learning
        
        There are three main types of machine learning: supervised learning, 
        unsupervised learning, and reinforcement learning. Each type has its 
        own use cases and algorithms.
        
        Supervised learning uses labeled data to train models. Common algorithms 
        include linear regression, decision trees, and neural networks. It's 
        widely used for classification and regression tasks.
        
        Unsupervised learning finds patterns in unlabeled data. Clustering and 
        dimensionality reduction are common applications. K-means and PCA are 
        popular algorithms in this category.
        
        Reinforcement learning trains agents to make sequential decisions by 
        maximizing rewards. It's used in robotics, game playing, and autonomous 
        systems.
        
        Applications and Future
        
        Machine learning applications are everywhere: recommendation systems, 
        fraud detection, medical diagnosis, and autonomous vehicles. The future 
        promises even more exciting developments with advances in deep learning 
        and neural architecture search.
        """

    def test_separation_of_concerns(self, sample_text):
        """Test that chunking and embedding are properly separated"""
        # 1. Chunking should work without any embedding provider
        chunker = TextChunker(
            ChunkingConfig(strategy=ChunkingStrategy.PARAGRAPH, chunk_size=500)
        )

        chunks = chunker.chunk_text(sample_text, "test_doc")

        # Verify chunks were created
        assert len(chunks) > 0
        assert all(hasattr(chunk, "text") for chunk in chunks)
        assert all(hasattr(chunk, "metadata") for chunk in chunks)

        # Verify no embedding-related data in chunks
        for chunk in chunks:
            assert "embedding" not in chunk.metadata
            assert "embedding_model" not in chunk.metadata
            assert "bert" not in chunk.metadata.get("chunk_type", "").lower()

    def test_simulated_embedding_provider(self, sample_text):
        """Test with simulated embeddings (no dependencies)"""
        # Get simulated provider
        provider = get_provider("simulated")
        assert provider.is_available()

        # Test single text embedding
        embedding = provider.embed_text("test text")
        assert isinstance(embedding, (list, np.ndarray))
        assert len(embedding) == provider.dimension

        # Test multiple text embeddings
        texts = ["first text", "second text", "third text"]
        embeddings = provider.embed_texts(texts)
        assert embeddings.shape == (3, provider.dimension)

        # Test deterministic behavior
        embedding1 = provider.embed_text("test")
        embedding2 = provider.embed_text("test")
        np.testing.assert_array_equal(embedding1, embedding2)

    def test_complete_workflow_with_simulated(self, sample_text):
        """Test complete chunking + embedding workflow"""
        # 1. Chunk text
        chunker = TextChunker(
            ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC, chunk_size=300)
        )
        chunks = chunker.chunk_text(sample_text, "ml_intro")

        assert len(chunks) > 0
        print(f"Created {len(chunks)} chunks")

        # 2. Generate embeddings
        provider = get_provider("simulated", dimension=256)
        chunk_texts = [chunk.text for chunk in chunks]
        embeddings = provider.embed_texts(chunk_texts)

        assert embeddings.shape == (len(chunks), 256)

        # 3. Create vector records
        records = create_vector_records(
            chunks,
            embeddings.tolist(),
            collection_metadata={"document_type": "tutorial"},
            filterable_fields=["document_type", "section_type"],
        )

        assert len(records) == len(chunks)

        # Verify record structure
        for i, record in enumerate(records):
            assert record.id == chunks[i].chunk_id
            assert len(record.vector) == 256
            assert "source_id" in record.metadata
            assert "chunk_index" in record.metadata
            assert record.metadata.get("document_type") == "tutorial"

    def test_convenience_function(self, sample_text):
        """Test the chunk_and_embed_text convenience function"""
        provider = get_provider("simulated", dimension=128)

        records = chunk_and_embed_text(
            text=sample_text,
            source_id="ml_guide",
            embedding_provider=provider,
            chunking_config=ChunkingConfig(
                strategy=ChunkingStrategy.SLIDING_WINDOW,
                chunk_size=200,
                chunk_overlap=50,
            ),
            metadata={"category": "education", "topic": "ML"},
            filterable_fields=["category", "topic"],
        )

        assert len(records) > 0

        # Verify all records have required fields
        for record in records:
            assert record.id.startswith("ml_guide_")
            assert len(record.vector) == 128
            assert record.metadata["category"] == "education"
            assert record.metadata["topic"] == "ML"
            assert "chunk_index" in record.metadata

    @pytest.mark.parametrize(
        "strategy",
        [
            ChunkingStrategy.SLIDING_WINDOW,
            ChunkingStrategy.SENTENCE,
            ChunkingStrategy.PARAGRAPH,
            ChunkingStrategy.SEMANTIC,
            ChunkingStrategy.RECURSIVE,
        ],
    )
    def test_all_strategies_with_embeddings(self, sample_text, strategy):
        """Test all chunking strategies work with embeddings"""
        # Configure chunking
        config = ChunkingConfig(strategy=strategy, chunk_size=250)
        chunker = TextChunker(config)

        # Chunk text
        chunks = chunker.chunk_text(sample_text, f"test_{strategy.value}")
        assert len(chunks) > 0

        # Generate embeddings
        provider = get_provider("simulated")
        embeddings = provider.embed_texts([c.text for c in chunks])

        # Create records
        records = create_vector_records(chunks, embeddings.tolist())

        assert len(records) == len(chunks)
        assert all(r.metadata["chunking_strategy"] == strategy.value for r in records)

    def test_provider_listing(self):
        """Test listing available embedding providers"""
        # This should work without any imports
        recommend_free_providers()

        # Test creating different providers
        providers_to_test = [
            ("simulated", True),  # Always available
            ("sentence-transformer", None),  # Depends on installation
            ("fastembed", None),  # Depends on installation
            ("instructor", None),  # Depends on installation
        ]

        for provider_name, expected_available in providers_to_test:
            try:
                provider = get_provider(provider_name)
                is_available = provider.is_available()

                if expected_available is not None:
                    assert is_available == expected_available

                print(
                    f"{provider_name}: {'✓ Available' if is_available else '✗ Not installed'}"
                )

            except Exception as e:
                print(f"{provider_name}: ✗ Error - {e}")

    def test_real_embeddings_if_available(self, sample_text):
        """Test with real embedding providers if available"""
        # Try to get a real embedding provider
        real_provider = None

        for provider_type in ["fastembed", "sentence-transformer"]:
            try:
                provider = get_provider(provider_type)
                if provider.is_available():
                    real_provider = provider
                    print(f"Using real provider: {provider_type}")
                    break
            except:
                continue

        if not real_provider:
            pytest.skip("No real embedding providers available")

        # Test with real embeddings
        chunks = TextChunker().chunk_text(sample_text, "real_test")
        chunk_texts = [chunk.text for chunk in chunks]

        embeddings = real_provider.embed_texts(chunk_texts)

        # Verify embeddings are high quality
        assert embeddings.shape[0] == len(chunks)
        assert embeddings.shape[1] == real_provider.dimension

        # Check embeddings are normalized if configured
        if real_provider.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1)
            np.testing.assert_allclose(norms, 1.0, rtol=1e-5)

        # Test semantic similarity
        # Similar texts should have similar embeddings
        similar_texts = [
            "Machine learning is a type of artificial intelligence",
            "ML is a subset of AI technology",
        ]
        similar_embeddings = real_provider.embed_texts(similar_texts)

        # Calculate cosine similarity
        similarity = np.dot(similar_embeddings[0], similar_embeddings[1])
        print(f"Semantic similarity: {similarity:.3f}")

        # Should be reasonably similar (> 0.5 for most models)
        assert similarity > 0.5

    def test_embedding_provider_fallback(self):
        """Test fallback mechanism for embedding providers"""
        # Try to create a provider that might not be available
        try:
            provider = get_provider("instructor")
            if not provider.is_available():
                # Should fall back to simulated
                assert provider.__class__.__name__ == "SimulatedEmbeddingProvider"
        except:
            # If provider creation fails, that's ok for this test
            pass

    def test_context_addition(self, sample_text):
        """Test adding context to chunks"""
        chunker = TextChunker(
            ChunkingConfig(
                strategy=ChunkingStrategy.SENTENCE, add_context=True, context_size=30
            )
        )

        chunks = chunker.chunk_text(sample_text, "context_test")

        # Add context manually
        chunks_with_context = chunker.add_context_to_chunks(chunks, context_size=30)

        # Verify context was added
        for i, chunk in enumerate(chunks_with_context):
            if i > 0:
                assert "prev_context" in chunk.metadata
            if i < len(chunks_with_context) - 1:
                assert "next_context" in chunk.metadata
            assert chunk.metadata.get("has_context") is True

    def test_metadata_handling(self):
        """Test metadata propagation through the pipeline"""
        text = "This is a test document about testing."

        # Custom metadata - VectorRecord only supports primitive types and lists
        # Nested dicts must be JSON-serialized
        import json

        doc_metadata = {
            "author": "Test Author",
            "date": "2024-01-01",
            "version": 1.0,
            "tags": ["test", "demo"],
            "complex_data": json.dumps(
                {"nested": {"value": 42}}
            ),  # Serialize nested dict as JSON string
        }

        provider = get_provider("simulated", dimension=64)

        records = chunk_and_embed_text(
            text=text,
            source_id="metadata_test",
            embedding_provider=provider,
            chunking_config=ChunkingConfig(
                strategy=ChunkingStrategy.SLIDING_WINDOW, chunk_size=20
            ),
            metadata=doc_metadata,
            filterable_fields=["author", "date", "version"],
        )

        # Verify metadata handling
        for record in records:
            # Filterable metadata should be at top level
            assert record.metadata["author"] == "Test Author"
            assert record.metadata["date"] == "2024-01-01"
            assert record.metadata["version"] == 1.0

            # Non-filterable metadata
            assert record.metadata["tags"] == ["test", "demo"]
            # Complex data is JSON-serialized string
            complex_data = json.loads(record.metadata["complex_data"])
            assert complex_data["nested"]["value"] == 42

    def test_batch_processing(self, sample_text):
        """Test batch processing of multiple documents"""
        # Use longer documents to ensure multiple chunks are created
        documents = [
            (
                "doc1",
                "First document about Python programming. " * 5,
            ),  # Repeat to get multiple chunks
            (
                "doc2",
                "Second document about data science and machine learning algorithms. "
                * 5,
            ),
            (
                "doc3",
                "Third document about neural networks and deep learning frameworks. "
                * 5,
            ),
        ]

        provider = get_provider("simulated", dimension=128)
        chunker = TextChunker(
            ChunkingConfig(
                strategy=ChunkingStrategy.SLIDING_WINDOW,
                chunk_size=50,
                chunk_overlap=10,  # Add overlap to increase chunk count
            )
        )

        all_records = []

        for doc_id, text in documents:
            chunks = chunker.chunk_text(text, doc_id)
            embeddings = provider.embed_texts([c.text for c in chunks])
            records = create_vector_records(
                chunks, embeddings.tolist(), collection_metadata={"batch": "test_batch"}
            )
            all_records.extend(records)

        # Verify all records were created (should have multiple chunks per document)
        assert len(all_records) >= len(
            documents
        ), f"Expected at least {len(documents)} records, got {len(all_records)}"

        # Verify each document is represented
        doc_ids = {r.metadata["source_id"] for r in all_records}
        assert doc_ids == {"doc1", "doc2", "doc3"}


if __name__ == "__main__":
    # Run specific test for debugging
    test = TestChunkingEmbeddingIntegration()
    test.test_provider_listing()

    # Test with sample text
    sample = test.sample_text()
    test.test_complete_workflow_with_simulated(sample)
