"""
SentenceTransformer mixin

Provides sentence-transformers integration with model caching.
"""

import json
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
        revision = self.config.model.revision
        truncate_dim = self.config.extra.get("truncate_dim")

        cache = ModelCache()
        # Device + backend + prompts participate in identity: two providers that
        # ask for different runtimes must NOT share a cached instance.
        cache_key = (
            f"st_{self.config.model.name}_{self.config.trust_remote_code}"
            f"_{device}_{backend}_{revision}_{truncate_dim}_"
            f"{json.dumps(prompts, sort_keys=True) if prompts else None}"
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
            if revision is not None:
                kwargs["revision"] = revision
            if truncate_dim is not None:
                kwargs["truncate_dim"] = truncate_dim
            try:
                model = SentenceTransformer(self.config.model.name, **kwargs)
            except TypeError as exc:
                if revision is not None or truncate_dim is not None:
                    raise TypeError(
                        "the installed sentence-transformers version cannot honor "
                        "the requested revision/truncate_dim; upgrade the dependency"
                    ) from exc
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

    @staticmethod
    def _prompt_template(prompt: str) -> str:
        """Convert a SentenceTransformers prefix into an input template."""
        return prompt if "{text}" in prompt else f"{prompt}{{text}}"

    def get_input_contract(self):
        """Resolve the exact tokenizer/rendering contract used for chunking.

        Model metadata is a declaration. This method loads the configured runtime
        and intersects that declaration with the actual tokenizer and model caps.
        Corpus builders should persist ``contract.to_manifest()`` beside texts.
        """
        from ...chunking_strategies.contracts import (
            InputRenderer,
            ResolvedInputContract,
        )
        from ...chunking_strategies.tokenizers import HuggingFaceTokenCounter

        self.ensure_initialized()
        tokenizer = getattr(self._model, "tokenizer", None)
        if tokenizer is None:
            raise ValueError(
                f"loaded model {self.config.model.name} exposes no tokenizer"
            )
        counter = HuggingFaceTokenCounter(tokenizer)

        declared_limit = self.config.model.max_length
        runtime_limit = getattr(self._model, "max_seq_length", None)
        limits = [declared_limit]
        if isinstance(runtime_limit, int) and runtime_limit > 0:
            limits.append(runtime_limit)
        if counter.advertised_limit is not None:
            limits.append(counter.advertised_limit)
        effective_limit = min(limits)

        metadata = self.config.model
        prompts = self.config.extra.get("prompts") or {}
        if "document" in prompts:
            document_template = self._prompt_template(prompts["document"])
        else:
            document_template = metadata.document_template

        if "query" in prompts:
            query_template = self._prompt_template(prompts["query"])
        else:
            query_template = metadata.query_template
            if query_template is None and metadata.requires_instruction:
                legacy = metadata.instruction_template
                if legacy is not None:
                    query_template = legacy.replace("{query}", "{text}")
        if query_template is None:
            query_template = "{text}"

        configured_dimension = self.get_dimension()
        dimension_getter = getattr(
            self._model, "get_sentence_embedding_dimension", None
        )
        runtime_dimension = dimension_getter() if callable(dimension_getter) else None
        if runtime_dimension is not None and runtime_dimension != configured_dimension:
            raise ValueError(
                f"configured output dimension {configured_dimension} does not match "
                f"runtime dimension {runtime_dimension} for {metadata.name}"
            )

        init_kwargs = getattr(tokenizer, "init_kwargs", {}) or {}
        resolved_revision = (
            metadata.revision
            or counter.resolved_revision
            or init_kwargs.get("_commit_hash")
            or init_kwargs.get("commit_hash")
            or "unresolved"
        )
        return ResolvedInputContract(
            model_id=metadata.name,
            model_revision=str(resolved_revision),
            counter=counter,
            effective_context_limit=effective_limit,
            renderer=InputRenderer(
                document_template=document_template,
                query_template=query_template,
            ),
            native_dimension=metadata.dimension,
            output_dimension=configured_dimension,
            supported_output_dimensions=metadata.supported_output_dimensions,
            minimum_output_dimension=metadata.minimum_output_dimension,
            document_encode_parameters=metadata.document_encode_parameters,
            query_encode_parameters=metadata.query_encode_parameters,
        )

    def _encode_with_prompt(self, texts: list[str], prompt_name: str) -> np.ndarray:
        """Encode using a registered native ST prompt, falling back to a plain
        encode if the prompt was not configured."""
        if not texts:
            return np.array([])
        self.ensure_initialized()
        configured = self.config.extra.get("prompts") or {}
        if prompt_name not in configured:
            metadata = self.config.model
            if prompt_name == "document":
                template = metadata.document_template
            else:
                template = metadata.query_template
                if template is None and metadata.requires_instruction:
                    legacy = metadata.instruction_template
                    template = (
                        legacy.replace("{query}", "{text}")
                        if legacy is not None
                        else None
                    )
            if template:
                texts = [template.format(text=text) for text in texts]
        role_parameters = (
            self.config.model.document_encode_parameters
            if prompt_name == "document"
            else self.config.model.query_encode_parameters
        )
        encode_kwargs = {
            "batch_size": self.config.batch_size,
            "normalize_embeddings": self.config.normalize,
            "show_progress_bar": False,
            "convert_to_numpy": True,
            **dict(role_parameters),
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
