"""
Base classes for embedding providers
"""

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import List, Optional, Dict, Any, Union
import numpy as np


@dataclass
class EmbeddingConfig:
    """Configuration for embedding providers"""
    model_name: str
    dimension: int
    batch_size: int = 32
    normalize: bool = True
    cache_embeddings: bool = True
    timeout_seconds: float = 30.0
    device: Optional[str] = None  # 'cpu', 'cuda', 'mps'
    extra_params: Optional[Dict[str, Any]] = None


class EmbeddingProvider(ABC):
    """
    Abstract base class for embedding providers
    
    All providers must implement this interface for compatibility
    """
    
    def __init__(self, config: Optional[EmbeddingConfig] = None):
        self.config = config or self._get_default_config()
        self._available = None
        self._initialize()
    
    @abstractmethod
    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration for this provider"""
        pass
    
    @abstractmethod
    def _initialize(self) -> None:
        """Initialize the embedding model"""
        pass
    
    @abstractmethod
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts
        
        Args:
            texts: List of texts to embed
            
        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        pass
    
    def embed_text(self, text: str) -> np.ndarray:
        """
        Generate embedding for a single text
        
        Args:
            text: Text to embed
            
        Returns:
            Embedding vector
        """
        embeddings = self.embed_texts([text])
        return embeddings[0]
    
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
        """Check if provider is available"""
        pass
    
    def batch_embed_texts(
        self,
        texts: List[str],
        batch_size: Optional[int] = None
    ) -> np.ndarray:
        """
        Embed texts in batches for memory efficiency
        
        Args:
            texts: List of texts to embed
            batch_size: Batch size (uses config default if not specified)
            
        Returns:
            Array of embeddings
        """
        batch_size = batch_size or self.config.batch_size
        
        if len(texts) <= batch_size:
            return self.embed_texts(texts)
        
        # Process in batches
        embeddings = []
        for i in range(0, len(texts), batch_size):
            batch = texts[i:i + batch_size]
            batch_embeddings = self.embed_texts(batch)
            embeddings.append(batch_embeddings)
        
        return np.vstack(embeddings)
    
    def __repr__(self) -> str:
        return f"{self.__class__.__name__}(model={self.model_name}, dim={self.dimension})"