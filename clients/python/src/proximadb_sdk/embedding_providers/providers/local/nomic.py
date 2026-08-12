"""Nomic retrieval embedding providers with explicit input contracts."""

from ...catalog import OPEN_MODEL_CATALOG
from ...core.base import BaseEmbeddingProvider
from ...core.config import ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.instruction import InstructionMixin
from ...mixins.sentence_transformer import SentenceTransformerMixin

NOMIC_MODELS = {
    model_id: OPEN_MODEL_CATALOG[model_id].metadata
    for model_id in (
        "nomic-ai/nomic-embed-text-v1.5",
        "nomic-ai/nomic-embed-text-v2-moe",
    )
}


@ProviderRegistry.register(
    name="nomic",
    models=NOMIC_MODELS,
    aliases=["nomic-embed"],
    description="Nomic retrieval embeddings with role prefixes and Matryoshka output",
)
class NomicProvider(InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider):
    """Nomic provider; v1.5 defaults to 8192-token, 768-dimensional output."""

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=NOMIC_MODELS["nomic-ai/nomic-embed-text-v1.5"],
            batch_size=16,
            normalize=True,
            trust_remote_code=True,
        )
