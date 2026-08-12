"""Curated open-weight text embedding model contracts.

Facts are intentionally declarative and source-linked. Runtime contracts still
intersect these declarations with the loaded tokenizer/model and persist the
resolved revision. Add variants here instead of creating provider subclasses.
"""

from __future__ import annotations

from dataclasses import dataclass

from .core.config import ModelMetadata

OPEN_MODEL_CATALOG_VERSION = "2026-08-11"


@dataclass(frozen=True)
class OpenModelSpec:
    """Loading and adoption metadata around a model input/output contract."""

    metadata: ModelMetadata
    family: str
    trust_remote_code: bool = False
    notes: str = ""


def _metadata(
    name: str,
    dimension: int,
    max_length: int,
    *,
    license_id: str,
    languages: str = "en",
    document_template: str = "{text}",
    query_template: str | None = None,
    discrete_dimensions: tuple[int, ...] = (),
    minimum_dimension: int | None = None,
    access: str = "open",
    source_url: str | None = None,
    document_parameters: tuple[tuple[str, str], ...] = (),
    query_parameters: tuple[tuple[str, str], ...] = (),
) -> ModelMetadata:
    return ModelMetadata(
        name=name,
        dimension=dimension,
        max_length=max_length,
        requires_instruction=query_template is not None,
        document_template=document_template,
        query_template=query_template,
        supported_output_dimensions=discrete_dimensions,
        minimum_output_dimension=minimum_dimension,
        languages=languages,
        license_id=license_id,
        access=access,
        source_url=source_url or f"https://huggingface.co/{name}",
        document_encode_parameters=document_parameters,
        query_encode_parameters=query_parameters,
    )


_BGE_QUERY = "Represent this sentence for searching relevant passages: {text}"
_QWEN_QUERY = (
    "Instruct: Given a web search query, retrieve relevant passages that answer "
    "the query\nQuery:{text}"
)


OPEN_MODEL_CATALOG: dict[str, OpenModelSpec] = {
    "sentence-transformers/all-MiniLM-L6-v2": OpenModelSpec(
        _metadata(
            "sentence-transformers/all-MiniLM-L6-v2",
            384,
            256,
            license_id="apache-2.0",
        ),
        "sentence-transformers",
        notes="The model card says inputs beyond 256 WordPieces are truncated.",
    ),
    "sentence-transformers/all-mpnet-base-v2": OpenModelSpec(
        _metadata(
            "sentence-transformers/all-mpnet-base-v2",
            768,
            384,
            license_id="apache-2.0",
        ),
        "sentence-transformers",
        notes="SentenceTransformers max_seq_length is 384.",
    ),
    **{
        model_id: OpenModelSpec(
            _metadata(
                model_id,
                dimension,
                512,
                license_id="mit",
                query_template=_BGE_QUERY,
            ),
            "bge-v1.5",
        )
        for model_id, dimension in (
            ("BAAI/bge-small-en-v1.5", 384),
            ("BAAI/bge-base-en-v1.5", 768),
            ("BAAI/bge-large-en-v1.5", 1024),
        )
    },
    "BAAI/bge-m3": OpenModelSpec(
        _metadata(
            "BAAI/bge-m3",
            1024,
            8192,
            license_id="mit",
            languages="multilingual",
        ),
        "bge-m3",
        notes="Dense BGE-M3 does not require the BGE v1.5 query instruction.",
    ),
    **{
        model_id: OpenModelSpec(
            _metadata(
                model_id,
                dimension,
                512,
                license_id="mit",
                document_template="passage: {text}",
                query_template="query: {text}",
            ),
            "e5-v2",
        )
        for model_id, dimension in (
            ("intfloat/e5-small-v2", 384),
            ("intfloat/e5-base-v2", 768),
            ("intfloat/e5-large-v2", 1024),
            ("intfloat/multilingual-e5-large", 1024),
        )
    },
    "nomic-ai/nomic-embed-text-v1.5": OpenModelSpec(
        _metadata(
            "nomic-ai/nomic-embed-text-v1.5",
            768,
            8192,
            license_id="apache-2.0",
            document_template="search_document: {text}",
            query_template="search_query: {text}",
            discrete_dimensions=(768, 512, 256, 128, 64),
        ),
        "nomic-v1.5",
        trust_remote_code=True,
    ),
    "nomic-ai/nomic-embed-text-v2-moe": OpenModelSpec(
        _metadata(
            "nomic-ai/nomic-embed-text-v2-moe",
            768,
            512,
            license_id="apache-2.0",
            languages="100+",
            document_template="search_document: {text}",
            query_template="search_query: {text}",
            minimum_dimension=256,
        ),
        "nomic-v2-moe",
        trust_remote_code=True,
    ),
    "Alibaba-NLP/gte-multilingual-base": OpenModelSpec(
        _metadata(
            "Alibaba-NLP/gte-multilingual-base",
            768,
            8192,
            license_id="apache-2.0",
            languages="75",
            minimum_dimension=128,
        ),
        "gte-multilingual",
        trust_remote_code=True,
    ),
    "Snowflake/snowflake-arctic-embed-l-v2.0": OpenModelSpec(
        _metadata(
            "Snowflake/snowflake-arctic-embed-l-v2.0",
            1024,
            8192,
            license_id="apache-2.0",
            languages="multilingual",
            query_template="query: {text}",
            discrete_dimensions=(1024, 256),
        ),
        "arctic-embed-v2",
    ),
    "jinaai/jina-embeddings-v2-base-en": OpenModelSpec(
        _metadata(
            "jinaai/jina-embeddings-v2-base-en",
            768,
            8192,
            license_id="apache-2.0",
        ),
        "jina-v2",
        trust_remote_code=True,
    ),
    "jinaai/jina-embeddings-v3": OpenModelSpec(
        _metadata(
            "jinaai/jina-embeddings-v3",
            1024,
            8192,
            license_id="cc-by-nc-4.0",
            languages="94",
            discrete_dimensions=(1024, 768, 512, 256, 128, 64, 32),
            document_parameters=(("task", "retrieval.passage"),),
            query_parameters=(("task", "retrieval.query"),),
        ),
        "jina-v3",
        trust_remote_code=True,
        notes="Non-commercial license; retrieval role is an adapter parameter.",
    ),
    "mixedbread-ai/mxbai-embed-large-v1": OpenModelSpec(
        _metadata(
            "mixedbread-ai/mxbai-embed-large-v1",
            1024,
            512,
            license_id="apache-2.0",
            query_template=_BGE_QUERY,
            discrete_dimensions=(1024, 512),
        ),
        "mixedbread",
        notes="Only dimensions explicitly demonstrated by the model card are listed.",
    ),
    **{
        model_id: OpenModelSpec(
            _metadata(
                model_id,
                dimension,
                32768,
                license_id="apache-2.0",
                languages="100+",
                query_template=_QWEN_QUERY,
                minimum_dimension=32,
            ),
            "qwen3-embedding",
        )
        for model_id, dimension in (
            ("Qwen/Qwen3-Embedding-0.6B", 1024),
            ("Qwen/Qwen3-Embedding-4B", 2560),
            ("Qwen/Qwen3-Embedding-8B", 4096),
        )
    },
    "google/embeddinggemma-300m": OpenModelSpec(
        _metadata(
            "google/embeddinggemma-300m",
            768,
            2048,
            license_id="gemma",
            languages="100+",
            document_template="title: none | text: {text}",
            query_template="task: search result | query: {text}",
            discrete_dimensions=(768, 512, 256, 128),
            access="gated",
        ),
        "embeddinggemma",
        notes="Weights require accepting the Gemma terms on Hugging Face.",
    ),
    "ibm-granite/granite-embedding-311m-multilingual-r2": OpenModelSpec(
        _metadata(
            "ibm-granite/granite-embedding-311m-multilingual-r2",
            768,
            32768,
            license_id="apache-2.0",
            languages="200+",
            discrete_dimensions=(768, 512, 384, 256, 128),
        ),
        "granite-embedding-r2",
    ),
}


def get_open_model_spec(model_id: str) -> OpenModelSpec:
    """Return a curated spec or fail with a discoverable model list."""
    try:
        return OPEN_MODEL_CATALOG[model_id]
    except KeyError as exc:
        available = ", ".join(sorted(OPEN_MODEL_CATALOG))
        raise ValueError(
            f"unknown open embedding model {model_id!r}: {available}"
        ) from exc


def list_open_models(
    *, family: str | None = None, commercially_usable: bool | None = None
) -> list[OpenModelSpec]:
    """List specs with optional family and permissive-license filtering."""
    specs = list(OPEN_MODEL_CATALOG.values())
    if family is not None:
        specs = [spec for spec in specs if spec.family == family]
    if commercially_usable is not None:
        noncommercial = {"cc-by-nc-4.0"}
        specs = [
            spec
            for spec in specs
            if (spec.metadata.license_id not in noncommercial) == commercially_usable
        ]
    return sorted(specs, key=lambda spec: spec.metadata.name)
