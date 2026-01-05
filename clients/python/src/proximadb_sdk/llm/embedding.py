# Copyright 2025 ProximaDB
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.

"""Embedding Service using Victor's embedding infrastructure."""

import asyncio
from typing import Any, Dict, List, Optional

from proximadb_sdk.llm.config import EmbeddingConfig, EmbeddingProvider


class EmbeddingService:
    """Embedding service that leverages Victor's embedding models.

    Provides a unified interface for embedding generation using:
    - Sentence-transformers (local, air-gapped compatible)
    - OpenAI API
    - Cohere API
    - Ollama (local, high-performance)

    Usage:
        service = EmbeddingService(EmbeddingConfig(
            provider=EmbeddingProvider.SENTENCE_TRANSFORMERS,
            model_name="all-MiniLM-L12-v2",
        ))
        await service.initialize()
        embedding = await service.embed_text("Hello, world!")
    """

    def __init__(self, config: EmbeddingConfig):
        """Initialize embedding service.

        Args:
            config: Embedding configuration
        """
        self.config = config
        self._model = None
        self._initialized = False

    async def initialize(self) -> None:
        """Initialize the embedding model.

        Loads the appropriate embedding model based on configuration.
        Uses Victor's embedding infrastructure when available.
        """
        if self._initialized:
            return

        try:
            # Try to use Victor's embedding infrastructure
            if self.config.provider == EmbeddingProvider.SENTENCE_TRANSFORMERS:
                await self._init_sentence_transformers()
            elif self.config.provider == EmbeddingProvider.OPENAI:
                await self._init_openai()
            elif self.config.provider == EmbeddingProvider.COHERE:
                await self._init_cohere()
            elif self.config.provider == EmbeddingProvider.OLLAMA:
                await self._init_ollama()
            else:
                raise ValueError(f"Unknown provider: {self.config.provider}")

            self._initialized = True

        except ImportError as e:
            # Provide helpful error messages
            if "sentence_transformers" in str(e):
                raise ImportError(
                    "sentence-transformers not installed. "
                    "Install with: pip install proximadb-python[llm]"
                ) from e
            elif "openai" in str(e):
                raise ImportError(
                    "openai not installed. " "Install with: pip install openai"
                ) from e
            raise

    async def _init_sentence_transformers(self) -> None:
        """Initialize sentence-transformers model."""
        try:
            # Try to use Victor's shared EmbeddingService
            from victor.embeddings.service import (
                EmbeddingService as VictorEmbeddingService,
            )

            self._model = VictorEmbeddingService.get_instance(
                model_name=self.config.model_name
            )
            self._model._ensure_model_loaded()
            self._use_victor = True

        except ImportError:
            # Fall back to direct sentence-transformers
            from sentence_transformers import SentenceTransformer

            self._model = SentenceTransformer(self.config.model_name)
            self._use_victor = False

    async def _init_openai(self) -> None:
        """Initialize OpenAI embedding client."""
        try:
            from victor.vector_stores.models import (
                EmbeddingModelConfig,
                OpenAIEmbeddingModel,
            )

            config = EmbeddingModelConfig(
                model_type="openai",
                model_name=self.config.model_name,
                api_key=self.config.api_key,
                dimension=self.config.get_dimension(),
            )
            self._model = OpenAIEmbeddingModel(config)
            await self._model.initialize()
            self._use_victor = True

        except ImportError:
            # Fall back to direct openai
            from openai import AsyncOpenAI

            if not self.config.api_key:
                import os

                self.config.api_key = os.environ.get("OPENAI_API_KEY")

            self._model = AsyncOpenAI(api_key=self.config.api_key)
            self._use_victor = False

    async def _init_cohere(self) -> None:
        """Initialize Cohere embedding client."""
        try:
            from victor.vector_stores.models import (
                CohereEmbeddingModel,
                EmbeddingModelConfig,
            )

            config = EmbeddingModelConfig(
                model_type="cohere",
                model_name=self.config.model_name,
                api_key=self.config.api_key,
                dimension=self.config.get_dimension(),
            )
            self._model = CohereEmbeddingModel(config)
            await self._model.initialize()
            self._use_victor = True

        except ImportError:
            import cohere

            if not self.config.api_key:
                import os

                self.config.api_key = os.environ.get("COHERE_API_KEY")

            self._model = cohere.AsyncClient(api_key=self.config.api_key)
            self._use_victor = False

    async def _init_ollama(self) -> None:
        """Initialize Ollama embedding client."""
        try:
            from victor.vector_stores.models import (
                EmbeddingModelConfig,
                OllamaEmbeddingModel,
            )

            config = EmbeddingModelConfig(
                model_type="ollama",
                model_name=self.config.model_name,
                api_key=self.config.base_url,  # Reused for base_url
                dimension=self.config.get_dimension(),
            )
            self._model = OllamaEmbeddingModel(config)
            await self._model.initialize()
            self._use_victor = True

        except ImportError:
            import httpx

            self._model = httpx.AsyncClient(
                base_url=self.config.base_url,
                timeout=120.0,
            )
            self._use_victor = False

    async def embed_text(self, text: str) -> List[float]:
        """Generate embedding for a single text.

        Args:
            text: Text to embed

        Returns:
            Embedding vector as list of floats
        """
        if not self._initialized:
            await self.initialize()

        if self._use_victor:
            embedding = await self._model.embed_text(text)
            return embedding if isinstance(embedding, list) else embedding.tolist()

        # Direct implementations (fallback)
        if self.config.provider == EmbeddingProvider.SENTENCE_TRANSFORMERS:
            embedding = self._model.encode(text, convert_to_tensor=False)
            return embedding.tolist()

        elif self.config.provider == EmbeddingProvider.OPENAI:
            response = await self._model.embeddings.create(
                model=self.config.model_name,
                input=text,
            )
            return response.data[0].embedding

        elif self.config.provider == EmbeddingProvider.COHERE:
            response = await self._model.embed(
                texts=[text],
                model=self.config.model_name,
            )
            return response.embeddings[0]

        elif self.config.provider == EmbeddingProvider.OLLAMA:
            response = await self._model.post(
                "/api/embeddings",
                json={"model": self.config.model_name, "prompt": text},
            )
            return response.json()["embedding"]

        raise ValueError(f"Unknown provider: {self.config.provider}")

    async def embed_batch(self, texts: List[str]) -> List[List[float]]:
        """Generate embeddings for multiple texts.

        Args:
            texts: List of texts to embed

        Returns:
            List of embedding vectors
        """
        if not self._initialized:
            await self.initialize()

        if not texts:
            return []

        if self._use_victor:
            embeddings = await self._model.embed_batch(texts)
            return [e if isinstance(e, list) else e.tolist() for e in embeddings]

        # Direct implementations (fallback)
        if self.config.provider == EmbeddingProvider.SENTENCE_TRANSFORMERS:
            embeddings = self._model.encode(texts, convert_to_tensor=False)
            return [e.tolist() for e in embeddings]

        elif self.config.provider == EmbeddingProvider.OPENAI:
            response = await self._model.embeddings.create(
                model=self.config.model_name,
                input=texts,
            )
            return [item.embedding for item in response.data]

        elif self.config.provider == EmbeddingProvider.COHERE:
            response = await self._model.embed(
                texts=texts,
                model=self.config.model_name,
            )
            return response.embeddings

        elif self.config.provider == EmbeddingProvider.OLLAMA:
            # Ollama doesn't have batch API, use concurrent requests
            tasks = [self.embed_text(text) for text in texts]
            return await asyncio.gather(*tasks)

        raise ValueError(f"Unknown provider: {self.config.provider}")

    @property
    def dimension(self) -> int:
        """Get embedding dimension."""
        return self.config.get_dimension()

    @property
    def provider_name(self) -> str:
        """Get provider name."""
        return f"{self.config.provider.value}/{self.config.model_name}"

    async def close(self) -> None:
        """Clean up resources."""
        if self._model and hasattr(self._model, "close"):
            await self._model.close()
        self._model = None
        self._initialized = False
