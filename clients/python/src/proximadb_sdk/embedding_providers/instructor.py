"""
Instructor embedding provider

Uses the InstructorEmbedding library (Apache 2.0 license) for
instruction-following embedding models.
"""

import logging
from typing import Any, Dict, List, Optional, Union

import numpy as np

from .base import EmbeddingConfig, EmbeddingProvider

logger = logging.getLogger(__name__)


class InstructorProvider(EmbeddingProvider):
    """
    Embedding provider using Instructor models

    Instructor models are designed to follow instructions and generate
    embeddings based on the intended use case. They can produce different
    embeddings for the same text based on the instruction.

    Models:
    - hkunlp/instructor-base: 768 dims, good balance
    - hkunlp/instructor-large: 768 dims, higher quality
    - hkunlp/instructor-xl: 768 dims, best quality

    All models are Apache 2.0 licensed and free to use.
    """

    # Default instructions for different use cases
    DEFAULT_INSTRUCTIONS = {
        "retrieval": "Represent the document for retrieval:",
        "clustering": "Represent the document for clustering:",
        "classification": "Represent the document for classification:",
        "similarity": "Represent the document for similarity search:",
        "qa_doc": "Represent the document for question answering:",
        "qa_query": "Represent the question for retrieving supporting documents:",
    }

    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="hkunlp/instructor-base",
            dimension=768,
            batch_size=32,
            normalize=True,
            cache_embeddings=True,
            device=None,  # Auto-detect
            extra_params={"instruction": self.DEFAULT_INSTRUCTIONS["retrieval"]},
        )

    def _initialize(self) -> None:
        """Initialize the Instructor model"""
        try:
            from InstructorEmbedding import INSTRUCTOR

            # Initialize model
            self.model = INSTRUCTOR(self.config.model_name, device=self.config.device)

            # All Instructor models have 768 dimensions
            self.config.dimension = 768

            # Get instruction from config or use default
            self.instruction = self.config.extra_params.get(
                "instruction", self.DEFAULT_INSTRUCTIONS["retrieval"]
            )

            self._available = True
            logger.info(
                f"Initialized Instructor with model: {self.config.model_name} "
                f"(dimension: {self.config.dimension})"
            )

        except ImportError:
            self._available = False
            logger.warning(
                "InstructorEmbedding not installed. "
                "Install with: pip install InstructorEmbedding"
            )
        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize Instructor: {e}")

    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of texts to embed

        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError(
                "Instructor not available. "
                "Install with: pip install InstructorEmbedding"
            )

        if not texts:
            return np.array([])

        # Prepare texts with instructions
        instruction_pairs = [[self.instruction, text] for text in texts]

        # Generate embeddings
        embeddings = self.model.encode(
            instruction_pairs,
            batch_size=self.config.batch_size,
            show_progress_bar=False,
            normalize_embeddings=self.config.normalize,
            convert_to_numpy=True,
        )

        return embeddings

    def embed_texts_with_instructions(
        self, texts: List[str], instructions: Union[str, List[str]]
    ) -> np.ndarray:
        """
        Generate embeddings with custom instructions

        Args:
            texts: List of texts to embed
            instructions: Single instruction or list of instructions per text

        Returns:
            Array of embeddings
        """
        if not self._available:
            raise RuntimeError("Instructor not available")

        if isinstance(instructions, str):
            instructions = [instructions] * len(texts)

        instruction_pairs = [[inst, text] for inst, text in zip(instructions, texts)]

        embeddings = self.model.encode(
            instruction_pairs,
            batch_size=self.config.batch_size,
            show_progress_bar=False,
            normalize_embeddings=self.config.normalize,
            convert_to_numpy=True,
        )

        return embeddings

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
        if self._available is None:
            self._initialize()
        return self._available

    @classmethod
    def create_with_instruction(
        cls, instruction: str, model_name: str = "hkunlp/instructor-base", **kwargs
    ) -> "InstructorProvider":
        """
        Create provider with custom instruction

        Args:
            instruction: Custom instruction for embeddings
            model_name: Model to use
            **kwargs: Additional config parameters

        Returns:
            Configured InstructorProvider instance
        """
        config = EmbeddingConfig(
            model_name=model_name,
            dimension=768,
            extra_params={"instruction": instruction},
            **kwargs,
        )
        return cls(config)
