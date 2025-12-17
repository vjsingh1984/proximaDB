"""
Normalization mixin

Provides L2 normalization utilities for embedding vectors.
"""

import numpy as np
from typing import List
import logging

logger = logging.getLogger(__name__)


class NormalizationMixin:
    """
    Mixin for embedding normalization

    Provides utilities for L2-normalizing embeddings, which is required
    for cosine similarity calculations.

    Usage:
        class MyProvider(NormalizationMixin, BaseEmbeddingProvider):
            def embed(self, texts: List[str]) -> np.ndarray:
                embeddings = self._generate_raw_embeddings(texts)
                return self.normalize_embeddings(embeddings)
    """

    @staticmethod
    def normalize_embeddings(embeddings: np.ndarray) -> np.ndarray:
        """
        L2-normalize embeddings

        Args:
            embeddings: NumPy array of shape (n, dim) or (dim,)

        Returns:
            Normalized embeddings with L2 norm = 1.0

        Example:
            >>> embs = np.array([[3.0, 4.0], [1.0, 0.0]])
            >>> normalized = NormalizationMixin.normalize_embeddings(embs)
            >>> print(np.linalg.norm(normalized[0]))  # Should be 1.0
            1.0
        """
        if embeddings.size == 0:
            return embeddings

        # Handle both 1D and 2D arrays
        if embeddings.ndim == 1:
            norm = np.linalg.norm(embeddings)
            if norm == 0:
                logger.warning("Zero norm embedding detected")
                return embeddings
            return embeddings / norm
        else:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            # Avoid division by zero
            norms[norms == 0] = 1.0
            return embeddings / norms

    @staticmethod
    def check_normalized(embeddings: np.ndarray, atol: float = 1e-5) -> bool:
        """
        Check if embeddings are normalized

        Args:
            embeddings: NumPy array to check
            atol: Absolute tolerance for norm check

        Returns:
            True if all embeddings have L2 norm ≈ 1.0

        Example:
            >>> embs = np.array([[0.6, 0.8], [1.0, 0.0]])
            >>> is_normalized = NormalizationMixin.check_normalized(embs)
            >>> print(is_normalized)
            True
        """
        if embeddings.size == 0:
            return True

        if embeddings.ndim == 1:
            norm = np.linalg.norm(embeddings)
            return np.isclose(norm, 1.0, atol=atol)
        else:
            norms = np.linalg.norm(embeddings, axis=1)
            return np.allclose(norms, 1.0, atol=atol)

    @staticmethod
    def get_cosine_similarity(emb1: np.ndarray, emb2: np.ndarray) -> float:
        """
        Compute cosine similarity between two embeddings

        Args:
            emb1: First embedding (1D array)
            emb2: Second embedding (1D array)

        Returns:
            Cosine similarity in range [-1, 1]

        Note:
            If embeddings are already normalized, this is just a dot product.

        Example:
            >>> emb1 = np.array([1.0, 0.0])
            >>> emb2 = np.array([0.0, 1.0])
            >>> sim = NormalizationMixin.get_cosine_similarity(emb1, emb2)
            >>> print(sim)
            0.0
        """
        # Normalize inputs
        emb1_norm = NormalizationMixin.normalize_embeddings(emb1)
        emb2_norm = NormalizationMixin.normalize_embeddings(emb2)

        # Dot product of normalized vectors = cosine similarity
        return float(np.dot(emb1_norm, emb2_norm))
