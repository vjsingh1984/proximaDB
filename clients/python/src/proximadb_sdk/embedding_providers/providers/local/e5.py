"""
E5 (Text Embeddings by Weakly-Supervised Contrastive Pre-training) Provider

Microsoft's excellent general-purpose embeddings with query/passage prefix support.
"""

import numpy as np

from ...core.base import BaseEmbeddingProvider
from ...core.config import ModelMetadata, ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.instruction import InstructionMixin
from ...mixins.sentence_transformer import SentenceTransformerMixin

E5_MODELS = {
    "intfloat/e5-large-v2": ModelMetadata(
        name="intfloat/e5-large-v2",
        dimension=1024,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="query: {query}",
        mteb_score=65.0,
        languages="en",
        description="Best quality, top MTEB performer",
        use_case="Maximum accuracy for English text",
    ),
    "intfloat/e5-base-v2": ModelMetadata(
        name="intfloat/e5-base-v2",
        dimension=768,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="query: {query}",
        mteb_score=64.5,
        languages="en",
        description="Balanced quality and speed",
        use_case="Production systems",
    ),
    "intfloat/e5-small-v2": ModelMetadata(
        name="intfloat/e5-small-v2",
        dimension=384,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="query: {query}",
        mteb_score=62.8,
        languages="en",
        description="Fast and efficient",
        use_case="High-throughput applications",
    ),
    "intfloat/multilingual-e5-large": ModelMetadata(
        name="intfloat/multilingual-e5-large",
        dimension=1024,
        max_length=512,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="query: {query}",
        mteb_score=64.0,
        languages="100+",
        description="Multilingual support for 100+ languages",
        use_case="Multilingual applications",
    ),
}


@ProviderRegistry.register(
    name="e5",
    models=E5_MODELS,
    aliases=["microsoft-e5"],
    description="Microsoft's excellent general-purpose embeddings",
)
class E5Provider(InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider):
    """
    E5 embedding provider

    **Key features:**
    - Excellent general-purpose performance
    - Requires "query: " prefix for queries
    - Requires "passage: " prefix for documents
    - Multilingual variant available

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers import get_provider

    provider = get_provider("e5")

    # Query embeddings (automatic "query: " prefix)
    query_emb = provider.embed_query("machine learning")

    # Passage embeddings (automatic "passage: " prefix)
    passage_embs = provider.embed_passages(["ML is great", "AI is awesome"])
    ```

    **Note:** E5 uses different prefixes than BGE:
    - Queries: "query: {text}"
    - Passages: "passage: {text}" (handled automatically)
    """

    #: E5 models require this prefix on documents/passages (not just queries).
    PASSAGE_PREFIX = "passage: "

    def default_config(self) -> ProviderConfig:
        """Default to large model"""
        return ProviderConfig(
            model=E5_MODELS["intfloat/e5-large-v2"],
            batch_size=32,
            normalize=True,
            extra={"use_query_instruction": True},
        )

    def embed_passages(self, passages: list[str]) -> np.ndarray:
        """Embed passages with the mandatory E5 ``"passage: "`` prefix.

        Unlike the generic :class:`InstructionMixin` (which leaves passages
        unprefixed), E5 was trained with an asymmetric ``query:``/``passage:``
        scheme. Omitting the passage prefix silently degrades retrieval recall,
        so we prepend it here.
        """
        prefixed = [f"{self.PASSAGE_PREFIX}{p}" for p in passages]
        return self.embed(prefixed)
