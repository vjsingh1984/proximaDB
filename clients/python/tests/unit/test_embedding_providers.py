"""
Unit tests for embedding providers

Tests the base classes, factory, and simulated provider without requiring
external model dependencies.
"""
import pytest
import numpy as np
from unittest.mock import Mock, patch, MagicMock

from proximadb_sdk.embedding_providers.core import (
    BaseEmbeddingProvider as EmbeddingProvider,
    ProviderConfig as EmbeddingConfig,
    ProviderRegistry
)
from proximadb_sdk.embedding_providers.providers.testing.simulated import SimulatedEmbeddingProvider

# For backward compatibility with old tests, alias get_provider as factory
class EmbeddingProviderFactory:
    @staticmethod
    def create(provider_name, **kwargs):
        from proximadb_sdk.embedding_providers import get_provider
        return get_provider(provider_name, **kwargs)


class TestEmbeddingConfig:
    """Test EmbeddingConfig dataclass"""

    def test_basic_config(self):
        """Test basic configuration creation"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata

        model = ModelMetadata(
            name="test-model",
            dimension=384
        )
        config = EmbeddingConfig(model=model)

        assert config.model.name == "test-model"
        assert config.model.dimension == 384
        assert config.batch_size == 32  # Default
        assert config.normalize is True  # Default
        assert config.device is None  # Default
        assert config.model.max_length == 512  # Default
        assert config.extra == {}  # Default

    def test_custom_config(self):
        """Test configuration with custom values"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata

        model = ModelMetadata(
            name="custom-model",
            dimension=768,
            max_length=1024
        )
        config = EmbeddingConfig(
            model=model,
            batch_size=64,
            normalize=False,
            device="cuda",
            cache_dir="/tmp/cache",
            extra={"seed": 42}
        )

        assert config.model.name == "custom-model"
        assert config.model.dimension == 768
        assert config.batch_size == 64
        assert config.normalize is False
        assert config.device == "cuda"
        assert config.cache_dir == "/tmp/cache"
        assert config.model.max_length == 1024
        assert config.extra == {"seed": 42}


class TestEmbeddingProviderBase:
    """Test EmbeddingProvider abstract base class"""

    def test_cannot_instantiate_directly(self):
        """Test that abstract base class cannot be instantiated"""
        with pytest.raises(TypeError):
            EmbeddingProvider()

    def test_concrete_implementation(self):
        """Test that concrete implementation works"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata, ProviderConfig

        class ConcreteProvider(EmbeddingProvider):
            def __init__(self, config: EmbeddingConfig = None):
                if config is None:
                    config = ProviderConfig(
                        model=ModelMetadata(name="test", dimension=3)
                    )
                super().__init__(config)

            def default_config(self) -> ProviderConfig:
                return ProviderConfig(model=ModelMetadata(name="test", dimension=3))

            def _load_model(self):
                return None

            def embed(self, texts: list) -> np.ndarray:
                return np.array([[0.1, 0.2, 0.3]] * len(texts))

        provider = ConcreteProvider()
        assert provider.embed(["test"]).shape == (1, 3)
        assert provider.config.model.dimension == 3

    def test_optional_methods(self):
        """Test lifecycle methods work correctly"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata, ProviderConfig

        class MinimalProvider(EmbeddingProvider):
            def __init__(self, config: EmbeddingConfig = None):
                if config is None:
                    config = ProviderConfig(
                        model=ModelMetadata(name="test", dimension=3)
                    )
                super().__init__(config)

            def default_config(self) -> ProviderConfig:
                return ProviderConfig(model=ModelMetadata(name="test", dimension=3))

            def _load_model(self):
                return "dummy_model"

            def embed(self, texts: list) -> np.ndarray:
                return np.array([[1, 2, 3]])

        provider = MinimalProvider()

        # Test lifecycle methods
        assert not provider._initialized
        provider.ensure_initialized()
        assert provider._initialized
        assert provider._model == "dummy_model"

        # Test cleanup
        provider.cleanup()  # Should not raise


class TestSimulatedEmbeddingProvider:
    """Test SimulatedEmbeddingProvider"""

    def test_initialization_default_config(self):
        """Test provider initializes with default config"""
        provider = SimulatedEmbeddingProvider()

        assert provider.config.model.name == "simulated-embeddings"
        assert provider.config.model.dimension == 384
        assert provider.config.extra.get("seed") == 42
        assert provider.config.extra.get("method") == "hash"

    def test_initialization_custom_config(self):
        """Test provider initializes with custom config"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata

        model = ModelMetadata(name="test-simulated", dimension=128)
        config = EmbeddingConfig(
            model=model,
            extra={'seed': 123, 'method': 'hash'}
        )
        provider = SimulatedEmbeddingProvider(config)

        assert provider.config.model.name == "test-simulated"
        assert provider.config.model.dimension == 128
        assert provider.config.extra.get("seed") == 123
        assert provider.config.extra.get("method") == "hash"

    def test_embed_texts_hash_based(self):
        """Test hash-based embedding generation"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata

        model = ModelMetadata(name="simulated", dimension=128)
        config = EmbeddingConfig(
            model=model,
            normalize=True,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider = SimulatedEmbeddingProvider(config)

        texts = ["hello world", "test document", "another text"]
        embeddings = provider.embed(texts)

        # Check shape
        assert embeddings.shape == (3, 128)

        # Check normalization
        for emb in embeddings:
            norm = np.linalg.norm(emb)
            assert abs(norm - 1.0) < 1e-6  # Should be normalized

        # Check determinism
        embeddings2 = provider.embed(texts)
        np.testing.assert_array_equal(embeddings, embeddings2)

    def test_embed_texts_different_seeds(self):
        """Test different seeds produce different results"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata
        texts = ["test text"]

        # Seed 42
        model1 = ModelMetadata(name="sim", dimension=64)
        config1 = EmbeddingConfig(
            model=model1,
            normalize=False,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider1 = SimulatedEmbeddingProvider(config1)
        emb1 = provider1.embed(texts)

        # Seed 123
        model2 = ModelMetadata(name="sim", dimension=64)
        config2 = EmbeddingConfig(
            model=model2,
            normalize=False,
            extra={'seed': 123, 'method': 'hash'}
        )
        provider2 = SimulatedEmbeddingProvider(config2)
        emb2 = provider2.embed(texts)

        # Different seeds should produce different embeddings
        assert not np.array_equal(emb1, emb2)

    def test_empty_input(self):
        """Test handling of empty input"""
        provider = SimulatedEmbeddingProvider()
        embeddings = provider.embed([])
        assert embeddings.shape == (0,)

    def test_get_dimension(self):
        """Test dimension property"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata
        model = ModelMetadata(name="sim", dimension=256)
        config = EmbeddingConfig(model=model)
        provider = SimulatedEmbeddingProvider(config)
        assert provider.config.model.dimension == 256

    def test_get_model_info(self):
        """Test configuration access"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata
        model = ModelMetadata(name="test-simulated", dimension=64)
        config = EmbeddingConfig(
            model=model,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider = SimulatedEmbeddingProvider(config)

        assert provider.config.model.name == "test-simulated"
        assert provider.config.model.dimension == 64
        assert provider.config.extra.get("method") == "hash"
        assert provider.config.extra.get("seed") == 42

    def test_similarity_calculation(self):
        """Test similarity between embeddings"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata
        model = ModelMetadata(name="sim", dimension=64)
        config = EmbeddingConfig(
            model=model,
            normalize=True,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider = SimulatedEmbeddingProvider(config)

        emb1 = provider.embed(["test"])[0]
        emb2 = provider.embed(["test"])[0]
        emb3 = provider.embed(["different"])[0]

        # Same text should have similarity 1.0 (normalized, cosine similarity)
        sim_same = np.dot(emb1, emb2)
        assert abs(sim_same - 1.0) < 1e-6

        # Different texts should have similarity < 1.0
        sim_diff = np.dot(emb1, emb3)
        assert sim_diff < 1.0

    def test_normalization_flag(self):
        """Test normalization can be disabled"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata

        # With normalization
        model1 = ModelMetadata(name="sim", dimension=64)
        config1 = EmbeddingConfig(
            model=model1,
            normalize=True,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider1 = SimulatedEmbeddingProvider(config1)
        emb1 = provider1.embed(["test"])[0]
        assert abs(np.linalg.norm(emb1) - 1.0) < 1e-6

        # Without normalization
        model2 = ModelMetadata(name="sim", dimension=64)
        config2 = EmbeddingConfig(
            model=model2,
            normalize=False,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider2 = SimulatedEmbeddingProvider(config2)
        emb2 = provider2.embed(["test"])[0]
        assert abs(np.linalg.norm(emb2) - 1.0) > 0.1  # Should NOT be normalized

    def test_seed_determinism(self):
        """Test that same seed produces same embeddings"""
        from proximadb_sdk.embedding_providers.core import ModelMetadata

        # Provider with seed 42
        model1 = ModelMetadata(name="sim", dimension=64)
        config1 = EmbeddingConfig(
            model=model1,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider1 = SimulatedEmbeddingProvider(config1)
        emb1 = provider1.embed(["test"])[0]

        # Another provider with seed 42
        model2 = ModelMetadata(name="sim", dimension=64)
        config2 = EmbeddingConfig(
            model=model2,
            extra={'seed': 42, 'method': 'hash'}
        )
        provider2 = SimulatedEmbeddingProvider(config2)
        emb2 = provider2.embed(["test"])[0]

        # Should be identical
        np.testing.assert_array_equal(emb1, emb2)

        # Different seed should produce different embeddings
        model3 = ModelMetadata(name="sim", dimension=64)
        config3 = EmbeddingConfig(
            model=model3,
            extra={'seed': 123, 'method': 'hash'}
        )
        provider3 = SimulatedEmbeddingProvider(config3)
        emb3 = provider3.embed(["test"])[0]

        assert not np.array_equal(emb1, emb3)

    def test_embed_single_text(self):
        """Test embedding single text"""
        provider = SimulatedEmbeddingProvider()
        embedding = provider.embed(["test text"])[0]

        assert embedding.shape == (384,)  # Default dimension

    def test_embed_multiple_texts(self):
        """Test embedding multiple texts"""
        provider = SimulatedEmbeddingProvider()
        texts = ["first document", "second document"]

        embeddings = provider.embed(texts)
        assert embeddings.shape == (2, 384)


class TestEmbeddingProviderFactory:
    """Test EmbeddingProviderFactory"""

    def test_create_simulated_provider(self):
        """Test creating simulated provider through factory"""
        provider = EmbeddingProviderFactory.create(
            "simulated",
            dimension=128
        )

        assert isinstance(provider, SimulatedEmbeddingProvider)
        assert provider.config.model.dimension == 128

    def test_provider_aliases(self):
        """Test provider aliases work correctly"""
        aliases = ["test", "mock", "simulated"]

        for alias in aliases:
            provider = EmbeddingProviderFactory.create(alias, dimension=64)
            assert isinstance(provider, SimulatedEmbeddingProvider)

    def test_unknown_provider(self):
        """Test error on unknown provider"""
        with pytest.raises(ValueError) as exc_info:
            EmbeddingProviderFactory.create("unknown-provider")

        assert "Unknown" in str(exc_info.value) or "not found" in str(exc_info.value).lower()

    def test_dimension_override(self):
        """Test dimension can be overridden"""
        provider = EmbeddingProviderFactory.create("simulated", dimension=256)
        assert isinstance(provider, SimulatedEmbeddingProvider)
        assert provider.config.model.dimension == 256


class TestEmbeddingIntegration:
    """Integration tests for embedding providers"""

    def test_end_to_end_workflow(self):
        """Test complete workflow: create provider, embed, calculate similarity"""
        # Create provider
        provider = EmbeddingProviderFactory.create(
            "simulated",
            dimension=128
        )

        # Embed some texts
        texts = [
            "machine learning is fascinating",
            "deep learning with neural networks",
            "python programming language",
            "machine learning algorithms"
        ]

        embeddings = provider.embed(texts)

        # Check shape
        assert embeddings.shape == (4, 128)

        # Check all embeddings are normalized
        for emb in embeddings:
            norm = np.linalg.norm(emb)
            assert abs(norm - 1.0) < 1e-6

    def test_batch_processing(self):
        """Test batch processing of many texts"""
        provider = EmbeddingProviderFactory.create(
            "simulated",
            dimension=64
        )

        # Create large batch
        texts = [f"Document number {i}" for i in range(1000)]

        embeddings = provider.embed(texts)

        assert embeddings.shape == (1000, 64)

        # All should be normalized
        norms = np.linalg.norm(embeddings, axis=1)
        assert np.all(np.abs(norms - 1.0) < 1e-6)

    def test_dimension_flexibility(self):
        """Test different embedding dimensions"""
        dimensions = [64, 128, 256, 384, 512, 768, 1536]

        for dim in dimensions:
            provider = EmbeddingProviderFactory.create("simulated", dimension=dim)

            embeddings = provider.embed(["test"])
            assert embeddings.shape == (1, dim)
            assert provider.config.model.dimension == dim
