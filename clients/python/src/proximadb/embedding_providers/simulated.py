"""
Simulated embedding provider for testing

Generates deterministic embeddings without requiring any external dependencies.
"""

import numpy as np
from typing import List, Optional, Dict, Any
import hashlib
import logging

from .base import EmbeddingProvider, EmbeddingConfig

logger = logging.getLogger(__name__)


class SimulatedEmbeddingProvider(EmbeddingProvider):
    """
    Simulated embedding provider for testing and development
    
    This provider generates deterministic embeddings based on text content
    without requiring any external models or APIs. Useful for:
    - Unit testing
    - Development without model dependencies
    - Performance testing of downstream components
    - Demos and examples
    """
    
    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="simulated-embeddings",
            dimension=384,
            batch_size=1000,
            normalize=True,
            cache_embeddings=False,
            device="cpu",
            extra_params={
                "seed": 42,
                "method": "hash_based"  # or "random", "sequential"
            }
        )
    
    def _initialize(self) -> None:
        """Initialize the simulated provider"""
        self.seed = self.config.extra_params.get("seed", 42)
        self.method = self.config.extra_params.get("method", "hash_based")
        self._available = True
        
        # Initialize random generator with seed
        self.rng = np.random.RandomState(self.seed)
        
        logger.info(f"Initialized SimulatedEmbeddingProvider with method: {self.method}, "
                   f"dimension: {self.config.dimension}")
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate simulated embeddings for multiple texts
        
        Args:
            texts: List of texts to embed
            
        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not texts:
            return np.array([])
        
        embeddings = []
        
        for text in texts:
            if self.method == "hash_based":
                embedding = self._hash_based_embedding(text)
            elif self.method == "random":
                embedding = self._random_embedding(text)
            elif self.method == "sequential":
                embedding = self._sequential_embedding(text)
            else:
                embedding = self._hash_based_embedding(text)
            
            embeddings.append(embedding)
        
        embeddings = np.array(embeddings)
        
        # Normalize if requested
        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1  # Avoid division by zero
            embeddings = embeddings / norms
        
        return embeddings
    
    def _hash_based_embedding(self, text: str) -> np.ndarray:
        """
        Generate deterministic embedding based on text hash
        
        This method creates embeddings that are:
        - Deterministic (same text -> same embedding)
        - Different for different texts
        - Distributed reasonably in embedding space
        """
        # Create multiple hashes for different parts of the embedding
        hashes = []
        for i in range(self.config.dimension // 8 + 1):
            h = hashlib.sha256(f"{text}_{i}_{self.seed}".encode()).digest()
            hashes.append(h)
        
        # Convert hashes to float values
        all_bytes = b''.join(hashes)
        values = []
        
        for i in range(self.config.dimension):
            # Get byte value and convert to float in [-1, 1]
            byte_val = all_bytes[i % len(all_bytes)]
            float_val = (byte_val / 127.5) - 1.0
            values.append(float_val)
        
        return np.array(values)
    
    def _random_embedding(self, text: str) -> np.ndarray:
        """
        Generate pseudo-random embedding seeded by text
        
        Same text will produce same embedding with same seed
        """
        # Use text hash as seed for this embedding
        text_hash = int(hashlib.md5(f"{text}_{self.seed}".encode()).hexdigest(), 16)
        local_rng = np.random.RandomState(text_hash % (2**32))
        
        # Generate random values
        embedding = local_rng.randn(self.config.dimension)
        
        return embedding
    
    def _sequential_embedding(self, text: str) -> np.ndarray:
        """
        Generate embedding based on text statistics
        
        Creates embeddings that reflect some text properties:
        - Length
        - Character distribution
        - Word count
        """
        # Calculate various text statistics
        stats = []
        
        # Basic stats
        stats.append(len(text) / 1000.0)  # Normalized length
        stats.append(len(text.split()) / 100.0)  # Word count
        stats.append(text.count('.') / 10.0)  # Sentences
        
        # Character frequency
        for char in 'aeiou':
            stats.append(text.lower().count(char) / len(text) if text else 0)
        
        # Hash for remaining dimensions
        text_hash = hashlib.sha256(f"{text}_{self.seed}".encode()).digest()
        hash_floats = [b / 255.0 for b in text_hash]
        
        # Combine stats and hash
        embedding = stats + hash_floats
        
        # Pad or truncate to correct dimension
        if len(embedding) < self.config.dimension:
            # Pad with cycled values
            while len(embedding) < self.config.dimension:
                embedding.extend(embedding[:self.config.dimension - len(embedding)])
        else:
            embedding = embedding[:self.config.dimension]
        
        return np.array(embedding)
    
    @property
    def dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension
    
    @property
    def model_name(self) -> str:
        """Get model name"""
        return self.config.model_name
    
    def is_available(self) -> bool:
        """Check if provider is available"""
        return True  # Always available
    
    def get_similarity(self, embedding1: np.ndarray, embedding2: np.ndarray) -> float:
        """
        Calculate cosine similarity between embeddings
        
        Utility method for testing
        """
        if self.config.normalize:
            # Already normalized, just dot product
            return np.dot(embedding1, embedding2)
        else:
            # Calculate cosine similarity
            norm1 = np.linalg.norm(embedding1)
            norm2 = np.linalg.norm(embedding2)
            if norm1 == 0 or norm2 == 0:
                return 0.0
            return np.dot(embedding1, embedding2) / (norm1 * norm2)