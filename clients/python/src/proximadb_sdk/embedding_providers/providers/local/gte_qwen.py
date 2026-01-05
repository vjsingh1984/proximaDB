"""
gte-Qwen Provider

Alibaba's state-of-the-art multilingual embedding models.
Ranks #1 on MTEB for both English and Chinese.
"""

from ...core.base import BaseEmbeddingProvider
from ...core.config import ProviderConfig, ModelMetadata
from ...core.registry import ProviderRegistry
from ...mixins.sentence_transformer import SentenceTransformerMixin
from ...mixins.instruction import InstructionMixin


# Model catalog
GTE_QWEN_MODELS = {
    "Alibaba-NLP/gte-Qwen2-7B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen2-7B-instruct",
        dimension=3584,
        max_length=32768,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=71.0,
        languages="100+",
        description="Top MTEB performer, #1 English & Chinese, 7B parameters",
        use_case="Maximum accuracy, multilingual, enterprise applications",
    ),
    "Alibaba-NLP/gte-Qwen2-1.5B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen2-1.5B-instruct",
        dimension=1536,
        max_length=32768,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=68.0,
        languages="100+",
        description="Smaller variant, excellent quality, 1.5B parameters",
        use_case="Balanced accuracy and speed, multilingual",
    ),
    "Alibaba-NLP/gte-Qwen1.5-7B-instruct": ModelMetadata(
        name="Alibaba-NLP/gte-Qwen1.5-7B-instruct",
        dimension=3584,
        max_length=8192,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=70.0,
        languages="100+",
        description="Earlier 7B model, still excellent performance",
        use_case="Alternative to Qwen2-7B",
    ),
}


@ProviderRegistry.register(
    name="gte-qwen",
    models=GTE_QWEN_MODELS,
    aliases=["alibaba", "qwen", "gte"],
    description="Alibaba's state-of-the-art multilingual embeddings (#1 MTEB)",
)
class GTEQwenProvider(
    InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider
):
    """
    gte-Qwen embedding provider

    **Features:**
    - #1 MTEB score for multilingual embeddings
    - Automatic query instruction handling
    - Model caching for memory efficiency
    - 100+ language support

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers import get_provider

    # Simple usage
    provider = get_provider("gte-qwen")

    # Query embeddings (with automatic instruction)
    query_emb = provider.embed_query("What is machine learning?")

    # Passage embeddings (no instruction)
    passage_embs = provider.embed_passages([
        "ML is a subset of AI",
        "AI enables machines to learn"
    ])
    ```
    """

    def default_config(self) -> ProviderConfig:
        """Default configuration using 1.5B model for best balance"""
        return ProviderConfig(
            model=GTE_QWEN_MODELS["Alibaba-NLP/gte-Qwen2-1.5B-instruct"],
            batch_size=16,
            normalize=True,
            trust_remote_code=False,  # Use standard implementation
            extra={"use_query_instruction": True},
        )
