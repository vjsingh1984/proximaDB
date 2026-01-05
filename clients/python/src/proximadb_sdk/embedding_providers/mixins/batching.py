"""
Batching mixin

Provides intelligent batching strategies for embedding generation.
"""

import logging
from typing import Iterator, List, Optional

import numpy as np

logger = logging.getLogger(__name__)


class BatchingMixin:
    """
    Mixin for intelligent batch processing

    Provides utilities for:
    - Splitting large inputs into batches
    - Adaptive batch sizing based on text length
    - Memory-efficient batch iteration

    Usage:
        class MyProvider(BatchingMixin, BaseEmbeddingProvider):
            def embed(self, texts: List[str]) -> np.ndarray:
                batches = list(self.create_batches(texts))
                results = [self._embed_batch(batch) for batch in batches]
                return np.vstack(results)
    """

    def create_batches(
        self, texts: List[str], batch_size: Optional[int] = None
    ) -> Iterator[List[str]]:
        """
        Split texts into batches

        Args:
            texts: List of texts to batch
            batch_size: Batch size (uses config.batch_size if None)

        Yields:
            Batches of texts

        Example:
            >>> mixin = BatchingMixin()
            >>> texts = ["text1", "text2", "text3", "text4", "text5"]
            >>> batches = list(mixin.create_batches(texts, batch_size=2))
            >>> print(len(batches))
            3
        """
        if batch_size is None:
            batch_size = getattr(self.config, "batch_size", 32)

        for i in range(0, len(texts), batch_size):
            yield texts[i : i + batch_size]

    def adaptive_batch_size(self, texts: List[str]) -> int:
        """
        Calculate adaptive batch size based on text lengths

        Longer texts require smaller batches to avoid OOM errors.

        Args:
            texts: List of texts

        Returns:
            Recommended batch size

        Example:
            >>> mixin = BatchingMixin()
            >>> short_texts = ["hi"] * 100
            >>> long_texts = ["very " * 1000 + "long"] * 100
            >>> print(mixin.adaptive_batch_size(short_texts))
            32
            >>> print(mixin.adaptive_batch_size(long_texts))
            4
        """
        if not texts:
            return getattr(self.config, "batch_size", 32)

        # Calculate average text length
        avg_length = sum(len(t) for t in texts) / len(texts)

        # Adaptive sizing heuristic
        if avg_length < 100:
            return 64
        elif avg_length < 500:
            return 32
        elif avg_length < 2000:
            return 16
        elif avg_length < 5000:
            return 8
        else:
            return 4

    def estimate_memory_usage(self, num_texts: int, avg_text_length: int) -> float:
        """
        Estimate memory usage for embedding batch

        Args:
            num_texts: Number of texts
            avg_text_length: Average text length in characters

        Returns:
            Estimated memory in MB

        Example:
            >>> mixin = BatchingMixin()
            >>> memory_mb = mixin.estimate_memory_usage(100, 500)
            >>> print(f"Estimated: {memory_mb:.1f} MB")
        """
        # Rough heuristic:
        # - Each character ~2 bytes (UTF-8 average)
        # - Model hidden states ~4x text size
        # - Embedding output: num_texts * dimension * 4 bytes (float32)

        dimension = getattr(self.config.model, "dimension", 768)

        text_memory = num_texts * avg_text_length * 2
        hidden_memory = text_memory * 4
        output_memory = num_texts * dimension * 4

        total_bytes = text_memory + hidden_memory + output_memory
        return total_bytes / (1024 * 1024)  # Convert to MB

    def should_use_batching(self, num_texts: int) -> bool:
        """
        Determine if batching is needed

        Args:
            num_texts: Number of texts to embed

        Returns:
            True if batching should be used

        Example:
            >>> mixin = BatchingMixin()
            >>> print(mixin.should_use_batching(10))
            False
            >>> print(mixin.should_use_batching(100))
            True
        """
        batch_size = getattr(self.config, "batch_size", 32)
        return num_texts > batch_size
