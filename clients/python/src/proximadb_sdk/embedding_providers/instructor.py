"""
Instructor embedding provider

Uses the InstructorEmbedding library (Apache 2.0 license) for
instruction-following embedding models. The same text yields different
embeddings depending on the instruction prompt, which lets a single model
serve retrieval, clustering, classification and similarity use cases.

Ported onto :class:`core.BaseEmbeddingProvider` (TD-126 System-B collapse): the
model is loaded lazily through the standard ``_load_model`` lifecycle + shared
:class:`ModelCache`, and the provider self-registers under ``instructor``.

Install the optional dependency with: ``pip install InstructorEmbedding``.
"""

import logging

import numpy as np

from .core.base import BaseEmbeddingProvider
from .core.cache import ModelCache
from .core.config import ModelMetadata, ProviderConfig
from .core.device import resolve_device
from .core.registry import ProviderRegistry

logger = logging.getLogger(__name__)

# Default instructions for different use cases. ``retrieval`` is the default;
# callers can override via ``extra={"instruction": ...}`` on the config or use
# :meth:`InstructorProvider.embed_texts_with_instructions` for per-text prompts.
DEFAULT_INSTRUCTIONS = {
    "retrieval": "Represent the document for retrieval:",
    "clustering": "Represent the document for clustering:",
    "classification": "Represent the document for classification:",
    "similarity": "Represent the document for similarity search:",
    "qa_doc": "Represent the document for question answering:",
    "qa_query": "Represent the question for retrieving supporting documents:",
}

# All hkunlp/instructor-* checkpoints emit 768-dim embeddings.
INSTRUCTOR_MODELS = {
    "hkunlp/instructor-base": ModelMetadata(
        name="hkunlp/instructor-base",
        dimension=768,
        max_length=512,
        provider_type="instructor",
        requires_instruction=True,
        languages="en",
        description="Instruction-following embeddings, balanced quality/speed",
        use_case="Retrieval/clustering/classification with task instructions",
    ),
    "hkunlp/instructor-large": ModelMetadata(
        name="hkunlp/instructor-large",
        dimension=768,
        max_length=512,
        provider_type="instructor",
        requires_instruction=True,
        languages="en",
        description="Higher-quality instruction-following embeddings",
        use_case="Accuracy-sensitive instruction-conditioned embeddings",
    ),
    "hkunlp/instructor-xl": ModelMetadata(
        name="hkunlp/instructor-xl",
        dimension=768,
        max_length=512,
        provider_type="instructor",
        requires_instruction=True,
        languages="en",
        description="Best-quality (largest) instruction-following model",
        use_case="Maximum accuracy when compute is not a constraint",
    ),
}


@ProviderRegistry.register(
    name="instructor",
    models=INSTRUCTOR_MODELS,
    aliases=["hkunlp", "instructor-embedding"],
    description="Instruction-following embeddings (InstructorEmbedding, Apache 2.0)",
)
class InstructorProvider(BaseEmbeddingProvider):
    """
    Embedding provider using Instructor models.

    Instructor models follow an instruction prefix, producing task-tailored
    embeddings. The active instruction is taken from
    ``config.extra["instruction"]`` (defaults to the retrieval instruction).

    Usage:

    ```python
    from proximadb_sdk.embedding_providers import get_provider

    provider = get_provider("instructor")
    emb = provider.embed(["financial report summary"])

    # Per-call instructions:
    emb = provider.embed_texts_with_instructions(
        ["query text"], "Represent the question for retrieval:"
    )
    ```
    """

    def default_config(self) -> ProviderConfig:
        """Default to the base model with the retrieval instruction."""
        return ProviderConfig(
            model=INSTRUCTOR_MODELS["hkunlp/instructor-base"],
            batch_size=32,
            normalize=True,
            extra={"instruction": DEFAULT_INSTRUCTIONS["retrieval"]},
        )

    @property
    def instruction(self) -> str:
        """Active default instruction for this provider."""
        return self.config.extra.get("instruction", DEFAULT_INSTRUCTIONS["retrieval"])

    def _load_model(self):
        """Load the INSTRUCTOR model (lazily, via the shared ModelCache)."""
        try:
            from InstructorEmbedding import INSTRUCTOR
        except ImportError as exc:
            raise ImportError(
                "InstructorEmbedding is required for InstructorProvider. "
                "Install with: pip install InstructorEmbedding"
            ) from exc

        device = resolve_device(self.config.device)
        cache = ModelCache()
        cache_key = f"instructor_{self.config.model.name}_{device}"

        def loader():
            logger.info(
                "Loading Instructor model %s (device=%s)",
                self.config.model.name,
                device,
            )
            return INSTRUCTOR(self.config.model.name, device=device)

        return cache.get_or_load(cache_key, loader)

    def embed(self, texts: list[str]) -> np.ndarray:
        """Embed texts using the provider's active instruction."""
        if not texts:
            return np.array([])
        return self.embed_texts_with_instructions(texts, self.instruction)

    def embed_texts_with_instructions(
        self, texts: list[str], instructions: str | list[str]
    ) -> np.ndarray:
        """
        Generate embeddings with explicit instruction(s).

        Args:
            texts: Texts to embed.
            instructions: A single instruction applied to all texts, or one
                instruction per text.

        Returns:
            NumPy array of shape ``(len(texts), dimension)``.
        """
        if not texts:
            return np.array([])

        self.ensure_initialized()

        if isinstance(instructions, str):
            instructions = [instructions] * len(texts)
        if len(instructions) != len(texts):
            raise ValueError(
                "instructions must be a single string or match the number of texts "
                f"(got {len(instructions)} instructions for {len(texts)} texts)"
            )

        instruction_pairs = [[inst, text] for inst, text in zip(instructions, texts)]

        return self._model.encode(
            instruction_pairs,
            batch_size=self.config.batch_size,
            show_progress_bar=False,
            normalize_embeddings=self.config.normalize,
            convert_to_numpy=True,
        )

    @classmethod
    def create_with_instruction(
        cls, instruction: str, model_name: str = "hkunlp/instructor-base", **kwargs
    ) -> "InstructorProvider":
        """
        Construct a provider pinned to a custom default instruction.

        Args:
            instruction: Default instruction prefix for :meth:`embed`.
            model_name: Instructor checkpoint (must be a known model name).
            **kwargs: Extra :class:`ProviderConfig` overrides (batch_size, device, ...).

        Returns:
            Configured :class:`InstructorProvider`.
        """
        model = INSTRUCTOR_MODELS.get(model_name)
        if model is None:
            model = ModelMetadata(
                name=model_name,
                dimension=768,
                provider_type="instructor",
                requires_instruction=True,
            )
        extra = {"instruction": instruction}
        extra.update(kwargs.pop("extra", {}))
        config = ProviderConfig(model=model, extra=extra, **kwargs)
        return cls(config)
