"""
SentenceTransformer mixin

Provides sentence-transformers integration with model caching.
"""

import logging

import numpy as np

from ..core.cache import ModelCache
from ..core.device import resolve_device

logger = logging.getLogger(__name__)


class SentenceTransformerMixin:
    """
    Mixin for sentence-transformers based providers

    This mixin provides:
    - Automatic model loading with caching
    - Compute-device auto-detection (cuda -> mps -> cpu) when config.device is None
    - Optional ONNX / OpenVINO backend for faster CPU inference
    - sentence-transformers native prompt support (encode_query / encode_document)
    - Standard embedding generation, normalization, batch processing

    Usage:
        class MyProvider(SentenceTransformerMixin, BaseEmbeddingProvider):
            def default_config(self) -> ProviderConfig:
                return ProviderConfig(model=...)

    Note:
        This mixin assumes the provider has a `config` attribute of type ProviderConfig.

    Perf levers (via ProviderConfig):
    - ``device=None`` auto-detects cuda -> mps -> cpu.
    - ``backend="onnx"`` (or ``"openvino"``) selects a faster CPU inference
      runtime (sentence-transformers >= 3.x). Falls back to the default torch
      backend with a warning if the runtime is unavailable.
    - ``extra["prompts"]`` (a ``{name: template}`` dict, e.g.
      ``{"query": "query: ", "document": "passage: "}``) registers native ST
      prompts so ``encode_query`` / ``encode_document`` apply them in-model.
    """

    def _load_model(self):
        """
        Load sentence-transformer model with caching

        Uses ModelCache to share model instances across provider instances.
        Honours device auto-detect, an optional onnx/openvino backend, and
        native ST prompts.

        Returns:
            Loaded SentenceTransformer model
        """
        try:
            from sentence_transformers import SentenceTransformer
        except ImportError:
            raise ImportError(
                "sentence-transformers is required for this provider. "
                "Install with: pip install sentence-transformers"
            )

        device = resolve_device(self.config.device)
        backend = self.config.backend
        prompts = self.config.extra.get("prompts")

        cache = ModelCache()
        # Device + backend + prompts participate in identity: two providers that
        # ask for different runtimes must NOT share a cached instance.
        cache_key = (
            f"st_{self.config.model.name}_{self.config.trust_remote_code}"
            f"_{device}_{backend}_{sorted(prompts) if prompts else None}"
        )

        def loader():
            logger.info(
                "Loading sentence-transformer model %s (device=%s, backend=%s)",
                self.config.model.name,
                device,
                backend or "torch",
            )
            kwargs = {
                "device": device,
                "trust_remote_code": self.config.trust_remote_code,
                "cache_folder": self.config.cache_dir,
            }
            if backend is not None:
                kwargs["backend"] = backend
            if prompts:
                kwargs["prompts"] = prompts
            try:
                model = SentenceTransformer(self.config.model.name, **kwargs)
            except TypeError:
                # Older sentence-transformers without backend/prompts kwargs:
                # retry with the universally-supported subset.
                logger.warning(
                    "sentence-transformers does not support backend/prompts "
                    "kwargs; loading %s with the default torch backend.",
                    self.config.model.name,
                )
                model = SentenceTransformer(
                    self.config.model.name,
                    device=device,
                    trust_remote_code=self.config.trust_remote_code,
                    cache_folder=self.config.cache_dir,
                )
            logger.info("Model loaded: %s", self.config.model.name)
            return model

        return cache.get_or_load(cache_key, loader)

    def _encode_with_prompt(self, texts: list[str], prompt_name: str) -> np.ndarray:
        """Encode using a registered native ST prompt, falling back to a plain
        encode if the prompt was not configured."""
        if not texts:
            return np.array([])
        self.ensure_initialized()
        configured = self.config.extra.get("prompts") or {}
        encode_kwargs = {
            "batch_size": self.config.batch_size,
            "normalize_embeddings": self.config.normalize,
            "show_progress_bar": False,
            "convert_to_numpy": True,
        }
        if prompt_name in configured:
            encode_kwargs["prompt_name"] = prompt_name
        return self._model.encode(texts, **encode_kwargs)

    def encode_query(self, query: str) -> np.ndarray:
        """Embed a query using the native ST ``query`` prompt (if configured)."""
        return self._encode_with_prompt([query], "query")[0]

    def encode_document(self, documents: list[str]) -> np.ndarray:
        """Embed documents using the native ST ``document`` prompt (if configured)."""
        return self._encode_with_prompt(documents, "document")

    def embed(self, texts: list[str]) -> np.ndarray:
        """
        Generate embeddings using sentence-transformers

        Args:
            texts: List of text strings to embed

        Returns:
            NumPy array of shape (len(texts), dimension)

        Example:
            >>> provider = MyProvider()
            >>> embeddings = provider.embed(["Hello", "World"])
            >>> print(embeddings.shape)
            (2, 384)
        """
        if not texts:
            return np.array([])

        self.ensure_initialized()

        logger.debug(
            f"Embedding {len(texts)} texts (batch_size={self.config.batch_size})"
        )

        embeddings = self._model.encode(
            texts,
            batch_size=self.config.batch_size,
            normalize_embeddings=self.config.normalize,
            show_progress_bar=False,
            convert_to_numpy=True,
        )

        return embeddings

    def embed_batch(
        self, texts: list[str], batch_size: int | None = None
    ) -> np.ndarray:
        """
        Embed texts with custom batch size

        Args:
            texts: List of text strings
            batch_size: Custom batch size (overrides config)

        Returns:
            NumPy array of embeddings

        Example:
            >>> provider = MyProvider()
            >>> embeddings = provider.embed_batch(texts, batch_size=64)
        """
        if batch_size is not None:
            original_batch_size = self.config.batch_size
            self.config = self.config.merge(batch_size=batch_size)
            try:
                return self.embed(texts)
            finally:
                self.config = self.config.merge(batch_size=original_batch_size)
        else:
            return self.embed(texts)
