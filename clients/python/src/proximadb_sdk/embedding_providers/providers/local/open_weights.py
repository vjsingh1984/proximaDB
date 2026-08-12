"""Generic provider for the curated open-weight embedding model catalog."""

from dataclasses import replace

from ...catalog import OPEN_MODEL_CATALOG, get_open_model_spec
from ...core.base import BaseEmbeddingProvider
from ...core.config import ProviderConfig
from ...core.registry import ProviderRegistry
from ...mixins.instruction import InstructionMixin
from ...mixins.sentence_transformer import SentenceTransformerMixin


@ProviderRegistry.register(
    name="open-weights",
    models={key: spec.metadata for key, spec in OPEN_MODEL_CATALOG.items()},
    aliases=["huggingface-embedding"],
    description="Curated open-weight SentenceTransformers-compatible models",
)
class OpenWeightsProvider(
    InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider
):
    """One runtime adapter for catalogued open-weight text embedders."""

    def default_config(self) -> ProviderConfig:
        spec = get_open_model_spec("sentence-transformers/all-MiniLM-L6-v2")
        return ProviderConfig(model=spec.metadata, batch_size=32, normalize=True)

    def embed(self, texts):
        """Embed unqualified text as documents, the safe ingestion default."""
        return self._encode_with_prompt(texts, "document")


def create_open_model_provider(
    model_id: str,
    *,
    dimension: int | None = None,
    revision: str | None = None,
    document_template: str | None = None,
    query_template: str | None = None,
    **config_kwargs,
) -> OpenWeightsProvider:
    """Create a catalogued provider with validated Matryoshka dimension."""
    spec = get_open_model_spec(model_id)
    metadata = replace(
        spec.metadata,
        revision=revision or spec.metadata.revision,
        document_template=document_template or spec.metadata.document_template,
        query_template=(
            query_template
            if query_template is not None
            else spec.metadata.query_template
        ),
    )
    extra = dict(config_kwargs.pop("extra", {}))
    if dimension is not None and dimension != metadata.dimension:
        if not metadata.supports_dimension(dimension):
            raise ValueError(f"{model_id} does not support {dimension} dimensions")
        extra["truncate_dim"] = dimension
    config = ProviderConfig(
        model=metadata,
        trust_remote_code=spec.trust_remote_code,
        extra=extra,
    ).merge(**config_kwargs)
    return OpenWeightsProvider(config)
