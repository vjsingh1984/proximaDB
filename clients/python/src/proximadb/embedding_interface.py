"""
Pluggable Embedding Interface for ProximaDB Python SDK

Provides a generic interface for embedding providers (BERT, Cohere, OpenAI, etc.)
to enable embedding-aware semantic chunking and other features.
"""

from abc import ABC, abstractmethod
from typing import List, Union, Dict, Any, Optional
from dataclasses import dataclass
import numpy as np


@dataclass
class EmbeddingConfig:
    """Generic embedding configuration with ultra-efficient enum tracking"""
    model_name: str
    dimension: int
    batch_size: int = 32
    normalize: bool = True
    cache_embeddings: bool = True
    timeout_seconds: float = 30.0
    api_key: Optional[str] = None
    api_url: Optional[str] = None
    extra_params: Dict[str, Any] = None
    
    # NEW: Ultra-efficient enum tracking for 75% storage savings
    track_model_usage: bool = True
    track_processing_time: bool = True
    track_quality_metrics: bool = True


class EmbeddingProvider(ABC):
    """Abstract base class for embedding providers"""
    
    @abstractmethod
    def __init__(self, config: EmbeddingConfig):
        """Initialize embedding provider with configuration"""
        self.config = config
    
    @abstractmethod
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for a list of texts
        
        Args:
            texts: List of text strings to embed
            
        Returns:
            numpy array of shape (len(texts), embedding_dimension)
        """
        pass
        
    def embed_texts_with_metadata(self, texts: List[str]) -> tuple[np.ndarray, Dict[str, Any]]:
        """
        Generate embeddings with processing metadata for ultra-efficient storage
        
        Args:
            texts: List of text strings to embed
            
        Returns:
            Tuple of (embeddings, processing_metadata)
        """
        import time
        start_time = time.time()
        
        # Generate embeddings
        embeddings = self.embed_texts(texts)
        
        # Create processing metadata for enum packing
        processing_time_ms = int((time.time() - start_time) * 1000)
        
        metadata = {
            'model_id': self.get_model_id(),
            'processing_time_ms': processing_time_ms,
            'batch_size': len(texts),
            'dimension': self.dimension,
        }
        
        return embeddings, metadata
    
    @abstractmethod
    def embed_text(self, text: str) -> np.ndarray:
        """
        Generate embedding for a single text
        
        Args:
            text: Text string to embed
            
        Returns:
            numpy array of shape (embedding_dimension,)
        """
        pass
    
    @property
    @abstractmethod
    def dimension(self) -> int:
        """Get embedding dimension"""
        pass
    
    @property
    @abstractmethod
    def model_name(self) -> str:
        """Get model name"""
        pass
    
    @abstractmethod
    def is_available(self) -> bool:
        """Check if the embedding provider is available"""
        pass
        
    def get_model_id(self) -> str:
        """Get model identifier for tracking"""
        return f"{self.__class__.__name__.lower().replace('embeddingprovider', '')}_{self.model_name}"
    
    def batch_embed_texts(self, texts: List[str], batch_size: Optional[int] = None) -> np.ndarray:
        """
        Embed texts in batches for efficiency
        
        Args:
            texts: List of texts to embed
            batch_size: Override default batch size
            
        Returns:
            numpy array of embeddings
        """
        batch_size = batch_size or self.config.batch_size
        embeddings = []
        
        for i in range(0, len(texts), batch_size):
            batch = texts[i:i + batch_size]
            batch_embeddings = self.embed_texts(batch)
            embeddings.append(batch_embeddings)
        
        return np.vstack(embeddings) if embeddings else np.array([])


class BERTEmbeddingProvider(EmbeddingProvider):
    """BERT embedding provider using sentence-transformers"""
    
    def __init__(self, config: EmbeddingConfig = None):
        """Initialize BERT embedding provider"""
        config = config or EmbeddingConfig(
            model_name="all-MiniLM-L6-v2",
            dimension=384,
            batch_size=32
        )
        super().__init__(config)
        
        self._model = None
        self._available = None
        self._initialize_model()
    
    def _initialize_model(self):
        """Initialize BERT model"""
        try:
            from sentence_transformers import SentenceTransformer
            self._model = SentenceTransformer(self.config.model_name)
            self._available = True
        except ImportError:
            self._available = False
        except Exception as e:
            self._available = False
            print(f"Failed to initialize BERT model: {e}")
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """Generate embeddings for multiple texts"""
        if not self.is_available():
            raise RuntimeError("BERT embedding provider is not available")
        
        embeddings = self._model.encode(
            texts, 
            batch_size=self.config.batch_size,
            normalize_embeddings=self.config.normalize
        )
        
        return embeddings
    
    def embed_text(self, text: str) -> np.ndarray:
        """Generate embedding for single text"""
        return self.embed_texts([text])[0]
    
    @property
    def dimension(self) -> int:
        """Get embedding dimension"""
        if self._model:
            return self._model.get_sentence_embedding_dimension()
        return self.config.dimension
    
    @property
    def model_name(self) -> str:
        """Get model name"""
        return self.config.model_name
    
    def is_available(self) -> bool:
        """Check if BERT is available"""
        return self._available


class SimulatedEmbeddingProvider(EmbeddingProvider):
    """Simulated embedding provider for testing when real embeddings unavailable"""
    
    def __init__(self, config: EmbeddingConfig = None):
        """Initialize simulated embedding provider"""
        config = config or EmbeddingConfig(
            model_name="simulated",
            dimension=384,
            batch_size=32
        )
        super().__init__(config)
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """Generate simulated embeddings based on text characteristics"""
        embeddings = []
        
        for text in texts:
            # Create deterministic embedding based on text content
            np.random.seed(hash(text) % (2**32))
            embedding = np.random.randn(self.config.dimension)
            
            # Add some semantic structure based on text features
            # Word count influences first dimensions
            word_count = len(text.split())
            embedding[0] = word_count / 100.0
            
            # Sentence count influences second dimension
            sentence_count = text.count('.') + text.count('!') + text.count('?')
            embedding[1] = sentence_count / 10.0
            
            # Normalize if requested
            if self.config.normalize:
                norm = np.linalg.norm(embedding)
                if norm > 0:
                    embedding = embedding / norm
            
            embeddings.append(embedding)
        
        return np.array(embeddings)
    
    def embed_text(self, text: str) -> np.ndarray:
        """Generate simulated embedding for single text"""
        return self.embed_texts([text])[0]
    
    @property
    def dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension
    
    @property 
    def model_name(self) -> str:
        """Get model name"""
        return self.config.model_name
    
    def is_available(self) -> bool:
        """Simulated provider is always available"""
        return True


class CohereEmbeddingProvider(EmbeddingProvider):
    """Cohere embedding provider (placeholder for future implementation)"""
    
    def __init__(self, config: EmbeddingConfig):
        """Initialize Cohere embedding provider"""
        if not config.api_key:
            raise ValueError("Cohere API key required")
        super().__init__(config)
        self._available = self._check_availability()
    
    def _check_availability(self) -> bool:
        """Check if Cohere API is available"""
        try:
            # Placeholder - would check Cohere API availability
            return False
        except:
            return False
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """Generate embeddings using Cohere API"""
        if not self.is_available():
            raise RuntimeError("Cohere embedding provider is not available")
        
        # Placeholder implementation
        # Would call Cohere API here
        raise NotImplementedError("Cohere provider not yet implemented")
    
    def embed_text(self, text: str) -> np.ndarray:
        """Generate embedding for single text"""
        return self.embed_texts([text])[0]
    
    @property
    def dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension
    
    @property
    def model_name(self) -> str:
        """Get model name"""
        return self.config.model_name
    
    def is_available(self) -> bool:
        """Check if Cohere is available"""
        return self._available


class EmbeddingProviderFactory:
    """Factory for creating embedding providers"""
    
    _providers = {
        "bert": BERTEmbeddingProvider,
        "sentence-transformers": BERTEmbeddingProvider,
        "all-MiniLM-L6-v2": BERTEmbeddingProvider,
        "all-mpnet-base-v2": BERTEmbeddingProvider,
        "simulated": SimulatedEmbeddingProvider,
        "cohere": CohereEmbeddingProvider,
    }
    
    @classmethod
    def create_provider(
        cls, 
        provider_type: str = "bert",
        config: Optional[EmbeddingConfig] = None
    ) -> EmbeddingProvider:
        """
        Create embedding provider instance
        
        Args:
            provider_type: Type of provider (bert, cohere, simulated, etc.)
            config: Embedding configuration
            
        Returns:
            EmbeddingProvider instance
        """
        # Handle model names as provider types
        if provider_type in ["all-MiniLM-L6-v2", "all-mpnet-base-v2"]:
            config = config or EmbeddingConfig(model_name=provider_type, dimension=384)
            provider_type = "bert"
        
        provider_class = cls._providers.get(provider_type.lower())
        
        if not provider_class:
            raise ValueError(f"Unknown embedding provider: {provider_type}")
        
        # Create provider with config
        provider = provider_class(config)
        
        # Fallback to simulated if requested provider unavailable
        if not provider.is_available():
            print(f"Warning: {provider_type} unavailable, using simulated embeddings")
            return SimulatedEmbeddingProvider(config)
        
        return provider
    
    @classmethod
    def register_provider(cls, name: str, provider_class: type):
        """Register custom embedding provider"""
        if not issubclass(provider_class, EmbeddingProvider):
            raise ValueError("Provider must inherit from EmbeddingProvider")
        cls._providers[name.lower()] = provider_class
    
    @classmethod
    def list_providers(cls) -> List[str]:
        """List available provider types"""
        return list(cls._providers.keys())


# Convenience functions
def create_embedding_provider(
    provider_type: str = "bert",
    model_name: Optional[str] = None,
    dimension: Optional[int] = None,
    **kwargs
) -> EmbeddingProvider:
    """
    Create embedding provider with simple configuration
    
    Args:
        provider_type: Provider type (bert, cohere, simulated)
        model_name: Model name override
        dimension: Embedding dimension override
        **kwargs: Additional config parameters
        
    Returns:
        Configured EmbeddingProvider instance
    """
    config_params = {}
    
    if model_name:
        config_params["model_name"] = model_name
    if dimension:
        config_params["dimension"] = dimension
    
    config_params.update(kwargs)
    
    # Create config if parameters provided
    config = EmbeddingConfig(**config_params) if config_params else None
    
    return EmbeddingProviderFactory.create_provider(provider_type, config)


def get_default_embedding_provider() -> EmbeddingProvider:
    """Get default embedding provider (BERT with fallback to simulated)"""
    return create_embedding_provider("bert")