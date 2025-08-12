"""
Base classes for embedding providers in ProximaDB SDK

This module defines the abstract base classes for embedding providers
that can be plugged into the ProximaDB SDK.
"""

from abc import ABC, abstractmethod
from typing import List, Dict, Any, Optional, Union
import numpy as np
from dataclasses import dataclass


@dataclass
class EmbeddingConfig:
    """Configuration for embedding providers"""
    model_name: str
    dimension: int
    batch_size: int = 32
    normalize: bool = True
    device: Optional[str] = None
    cache_dir: Optional[str] = None
    max_length: int = 512


class EmbeddingProvider(ABC):
    """
    Abstract base class for embedding providers
    
    All embedding providers must implement this interface to be
    compatible with the ProximaDB SDK.
    """
    
    @abstractmethod
    def embed_text(self, text: str) -> np.ndarray:
        """
        Generate embedding for a single text
        
        Args:
            text: Input text to embed
            
        Returns:
            Embedding vector as numpy array
        """
        pass
    
    @abstractmethod
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts
        
        Args:
            texts: List of texts to embed
            
        Returns:
            Array of embedding vectors
        """
        pass
    
    @abstractmethod
    def embed_documents(
        self,
        documents: List[Dict[str, Any]],
        text_field: str = 'text'
    ) -> np.ndarray:
        """
        Generate embeddings for documents
        
        Args:
            documents: List of document dictionaries
            text_field: Field in document containing text
            
        Returns:
            Array of embedding vectors
        """
        pass
    
    @abstractmethod
    def get_dimension(self) -> int:
        """Get embedding dimension"""
        pass
    
    @abstractmethod
    def get_model_info(self) -> Dict[str, Any]:
        """Get model information"""
        pass
    
    def preprocess_text(self, text: str) -> str:
        """
        Preprocess text before embedding (optional)
        
        Args:
            text: Raw text
            
        Returns:
            Preprocessed text
        """
        return text
    
    def clear_cache(self):
        """Clear any internal caches (optional)"""
        pass