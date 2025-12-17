"""
Base classes for embedding providers

Provides abstract base classes and protocols that all embedding providers must implement.
"""

from abc import ABC, abstractmethod
from typing import List, Optional, Dict, Any, Protocol, runtime_checkable
import numpy as np
import logging

from .config import ProviderConfig

logger = logging.getLogger(__name__)


@runtime_checkable
class EmbeddingProviderProtocol(Protocol):
    """
    Type protocol for embedding providers

    This defines the minimal interface that all embedding providers should implement.
    Use this for type hints when you want duck typing instead of inheritance.
    """

    def embed(self, texts: List[str]) -> np.ndarray:
        """Generate embeddings for a list of texts"""
        ...

    def get_dimension(self) -> int:
        """Get the embedding dimension"""
        ...

    def is_available(self) -> bool:
        """Check if the provider is available and ready to use"""
        ...


class BaseEmbeddingProvider(ABC):
    """
    Enhanced base class for embedding providers

    Provides:
    - Lazy initialization (models loaded only when first used)
    - Lifecycle management (init, cleanup)
    - Standardized configuration
    - Thread-safe initialization
    - Resource cleanup

    Subclasses must implement:
    - default_config(): Return default ProviderConfig
    - _load_model(): Load and return the model
    - embed(): Generate embeddings
    """

    def __init__(self, config: Optional[ProviderConfig] = None):
        """
        Initialize provider with optional configuration

        Args:
            config: Provider configuration. If None, uses default_config()
        """
        self.config = config if config is not None else self.default_config()
        self._initialized = False
        self._model = None
        self._init_lock = None  # Will be created on first use

    @abstractmethod
    def default_config(self) -> ProviderConfig:
        """
        Return default configuration for this provider

        Returns:
            ProviderConfig with sensible defaults

        Example:
            def default_config(self) -> ProviderConfig:
                return ProviderConfig(
                    model=MODEL_METADATA["default-model"],
                    batch_size=32,
                    normalize=True
                )
        """
        pass

    @abstractmethod
    def _load_model(self) -> Any:
        """
        Load the embedding model

        This method should:
        1. Load the model from cache or download
        2. Move to appropriate device
        3. Return the loaded model

        Returns:
            Loaded model object

        Note:
            This is called only once per provider instance (lazy loading).
            Use ModelCache for sharing models across instances.
        """
        pass

    @abstractmethod
    def embed(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for a list of texts

        Args:
            texts: List of text strings to embed

        Returns:
            NumPy array of shape (len(texts), dimension)

        Note:
            Implementations should call self.ensure_initialized() first
        """
        pass

    def ensure_initialized(self):
        """
        Ensure the provider is initialized (lazy loading)

        Thread-safe initialization. The model is loaded only on first use.
        Subsequent calls are no-ops.
        """
        if not self._initialized:
            # Thread-safe initialization
            import threading
            if self._init_lock is None:
                self._init_lock = threading.Lock()

            with self._init_lock:
                if not self._initialized:  # Double-check locking
                    logger.info(f"Initializing {self.__class__.__name__} "
                               f"with model: {self.config.model.name}")
                    self._model = self._load_model()
                    self._initialized = True
                    logger.info(f"Initialization complete: {self.config.model.name}")

    def get_dimension(self) -> int:
        """
        Get the embedding dimension

        Returns:
            Dimension of embedding vectors
        """
        return self.config.model.dimension

    def embed_text(self, text: str) -> np.ndarray:
        """
        Generate embedding for a single text (convenience method)

        Args:
            text: Text to embed

        Returns:
            Embedding vector as numpy array of shape (dimension,)
        """
        return self.embed([text])[0]

    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts (convenience method)

        Args:
            texts: List of texts to embed

        Returns:
            Embedding array of shape (len(texts), dimension)
        """
        return self.embed(texts)

    @property
    def dimension(self) -> int:
        """Alias for get_dimension() for backward compatibility"""
        return self.get_dimension()

    @property
    def model_name(self) -> str:
        """Get the model name"""
        if self.config and self.config.model:
            return self.config.model.name
        return "unknown"

    def is_available(self) -> bool:
        """
        Check if provider is available

        Returns:
            True if provider can be used, False otherwise
        """
        try:
            self.ensure_initialized()
            return self._model is not None
        except Exception as e:
            logger.warning(f"Provider not available: {e}")
            return False

    def get_model_info(self) -> Dict[str, Any]:
        """
        Get comprehensive model information

        Returns:
            Dictionary with model metadata
        """
        return {
            "name": self.config.model.name,
            "dimension": self.config.model.dimension,
            "max_length": self.config.model.max_length,
            "mteb_score": self.config.model.mteb_score,
            "languages": self.config.model.languages,
            "description": self.config.model.description,
            "provider_type": self.config.model.provider_type,
            "batch_size": self.config.batch_size,
            "normalize": self.config.normalize,
            "device": self.config.device,
        }

    def cleanup(self):
        """
        Cleanup resources

        Call this method to free memory when done using the provider.
        After cleanup, the provider can be reinitialized if needed.
        """
        if self._model is not None:
            logger.info(f"Cleaning up {self.__class__.__name__}")
            try:
                # Allow subclasses to customize cleanup
                self._cleanup_model()
            finally:
                del self._model
                self._model = None
                self._initialized = False

    def _cleanup_model(self):
        """
        Optional: Subclass-specific cleanup logic

        Override this method if you need custom cleanup logic.
        The default implementation does nothing.
        """
        pass

    def __enter__(self):
        """Context manager entry"""
        self.ensure_initialized()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit"""
        self.cleanup()

    def __repr__(self) -> str:
        """String representation"""
        status = "initialized" if self._initialized else "not initialized"
        return f"{self.__class__.__name__}(model={self.config.model.name}, {status})"
