"""
SFR (Salesforce Research) Embedding Provider

Salesforce's top-tier embeddings with best MTEB accuracy.
"""

from ...core.base import BaseEmbeddingProvider
from ...core.config import ModelMetadata, ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.instruction import InstructionMixin
from ...mixins.sentence_transformer import SentenceTransformerMixin

SFR_MODELS = {
    "Salesforce/SFR-Embedding-2_R": ModelMetadata(
        name="Salesforce/SFR-Embedding-2_R",
        dimension=4096,
        max_length=4096,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=66.4,
        languages="en",
        description="Top MTEB performer, best accuracy, retrieval-optimized",
        use_case="Maximum accuracy, research, when quality is paramount",
    ),
    "Salesforce/SFR-Embedding-Mistral": ModelMetadata(
        name="Salesforce/SFR-Embedding-Mistral",
        dimension=4096,
        max_length=32768,
        provider_type="sentence-transformer",
        requires_instruction=True,
        instruction_template="Instruct: Given a query, retrieve relevant passages that answer the query\nQuery: {query}",
        mteb_score=65.0,
        languages="en",
        description="Based on Mistral, long context support",
        use_case="Long documents, maximum context length",
    ),
}


@ProviderRegistry.register(
    name="sfr",
    models=SFR_MODELS,
    aliases=["salesforce"],
    description="Salesforce's top-tier embeddings (highest MTEB score)",
)
class SFRProvider(InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider):
    """
    SFR embedding provider

    **Key features:**
    - Highest MTEB score (66.4)
    - 4096-dimensional embeddings
    - Long context support (up to 32K tokens)
    - Query instruction support

    **Usage:**

    ```python
    from proximadb_sdk.embedding_providers import get_provider

    provider = get_provider("sfr")

    # Query embeddings (with automatic instruction)
    query_emb = provider.embed_query("best machine learning course")

    # Passage embeddings (no instruction)
    passage_embs = provider.embed_passages(["Course 1", "Course 2"])
    ```

    **Note:** SFR models are large (4096 dims) and may require more memory.
    Consider using smaller batch sizes if you encounter OOM errors.
    """

    def default_config(self) -> ProviderConfig:
        """Default to SFR-Embedding-2_R"""
        return ProviderConfig(
            model=SFR_MODELS["Salesforce/SFR-Embedding-2_R"],
            batch_size=16,  # Smaller batch for large model
            normalize=True,
            extra={"use_query_instruction": True},
        )
