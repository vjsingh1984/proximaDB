"""Contract tests for exact, model-agnostic token-budget chunking."""

from __future__ import annotations

import re

import pytest

from proximadb_sdk.chunking import ChunkerPool
from proximadb_sdk.chunking_strategies import (
    ChunkingConfig,
    ChunkingStrategy,
    CompositeInputContract,
    InputRenderer,
    InputRole,
    OverflowPolicy,
    ResolvedInputContract,
    ShortChunkPolicy,
    TokenBudget,
    get_chunking_strategy,
)
from proximadb_sdk.chunking_strategies.tokenizers import HuggingFaceTokenCounter
from proximadb_sdk.embedding_providers import get_provider
from proximadb_sdk.embedding_providers.catalog import OPEN_MODEL_CATALOG
from proximadb_sdk.embedding_providers.providers.local.open_weights import (
    create_open_model_provider,
)


class WordCounter:
    """Fast-tokenizer-shaped test double: words plus two special tokens."""

    def __init__(self, name: str = "words", advertised_limit: int = 512):
        self.name = name
        self.fingerprint = f"fingerprint:{name}"
        self.advertised_limit = advertised_limit

    def count(self, text: str) -> int:
        return len(list(re.finditer(r"\S+", text))) + 2

    def content_offsets(self, text: str) -> tuple[tuple[int, int], ...]:
        return tuple(match.span() for match in re.finditer(r"\S+", text))


def make_contract(
    *, name: str = "model", document_template: str = "prefix: {text}", limit: int = 8
) -> ResolvedInputContract:
    return ResolvedInputContract(
        model_id=name,
        model_revision="revision",
        counter=WordCounter(name, advertised_limit=limit),
        effective_context_limit=limit,
        renderer=InputRenderer(document_template=document_template),
        native_dimension=768,
    )


def make_strategy(
    contract=None,
    *,
    target: int = 7,
    overlap: int = 1,
    overflow: OverflowPolicy = OverflowPolicy.SPLIT,
    short: ShortChunkPolicy = ShortChunkPolicy.KEEP,
):
    return get_chunking_strategy(
        ChunkingStrategy.FIXED_SIZE,
        chunk_size=10_000,
        min_chunk_size=1,
        token_budget=TokenBudget(
            target_tokens=target,
            overlap_tokens=overlap,
            min_content_tokens=3,
            overflow_policy=overflow,
            short_chunk_policy=short,
        ),
        input_contract=contract or make_contract(),
        input_role=InputRole.DOCUMENT,
    )


def test_exact_rendered_budget_splits_with_token_overlap_and_full_coverage():
    strategy = make_strategy()
    chunks = strategy.chunk("one two three four five six seven eight nine", "doc")

    assert [chunk.text.split() for chunk in chunks] == [
        ["one", "two", "three", "four"],
        ["four", "five", "six", "seven"],
        ["seven", "eight", "nine"],
    ]
    assert [chunk.metadata["token_counts"]["model"] for chunk in chunks] == [
        7,
        7,
        6,
    ]
    assert all(chunk.metadata["total_chunks"] == 3 for chunk in chunks)
    assert chunks[1].metadata["overlap_tokens"] == 1


def test_composite_contract_uses_most_restrictive_rendered_input():
    primary = make_contract(name="primary", document_template="p: {text}", limit=8)
    secondary = make_contract(
        name="secondary", document_template="long role prefix: {text}", limit=8
    )
    strategy = make_strategy(CompositeInputContract((primary, secondary)))

    chunks = strategy.chunk("one two three four five", "doc")

    assert [len(chunk.text.split()) for chunk in chunks] == [2, 2, 2, 2]
    assert all(max(chunk.metadata["token_counts"].values()) <= 7 for chunk in chunks)


def test_sentence_boundaries_are_independent_of_legacy_character_size():
    strategy = get_chunking_strategy(
        ChunkingStrategy.SENTENCE,
        chunk_size=10_000,
        min_chunk_size=1,
        token_budget=TokenBudget(target_tokens=7),
        input_contract=make_contract(limit=8),
    )
    chunks = strategy.chunk("One two three. Four five six. Seven eight.", "doc")
    assert [chunk.text for chunk in chunks] == [
        "One two three.",
        "Four five six.",
        "Seven eight.",
    ]


@pytest.mark.parametrize("policy", [OverflowPolicy.ERROR, OverflowPolicy.DROP])
def test_oversized_source_policy_is_explicit(policy):
    strategy = make_strategy(overflow=policy)
    if policy == OverflowPolicy.ERROR:
        with pytest.raises(ValueError, match="token counts"):
            strategy.chunk("one two three four five", "doc")
    else:
        assert strategy.chunk("one two three four five", "doc") == []


def test_short_tail_policy_is_explicit():
    strategy = make_strategy(short=ShortChunkPolicy.DROP)
    chunks = strategy.chunk("one two three four five", "doc")
    assert [chunk.text.split() for chunk in chunks] == [["one", "two", "three", "four"]]


def test_pool_identity_includes_model_contract():
    pool = ChunkerPool()
    budget = TokenBudget(target_tokens=7, overlap_tokens=1)
    first = ChunkingConfig(token_budget=budget, input_contract=make_contract(name="a"))
    second = ChunkingConfig(token_budget=budget, input_contract=make_contract(name="b"))
    assert pool._get_pool_key(first) != pool._get_pool_key(second)


class FakeBackend:
    def to_str(self) -> str:
        return '{"model":"fake"}'


class FakeFastTokenizer:
    is_fast = True
    name_or_path = "fake/tokenizer"
    model_max_length = 16
    backend_tokenizer = FakeBackend()
    init_kwargs = {"_commit_hash": "runtime-revision"}

    def __call__(self, text, *, add_special_tokens, return_offsets_mapping=False, **_):
        offsets = tuple(match.span() for match in re.finditer(r"\S+", text))
        if return_offsets_mapping:
            return {"input_ids": list(range(len(offsets))), "offset_mapping": offsets}
        special = 2 if add_special_tokens else 0
        return {"input_ids": list(range(len(offsets) + special))}


def test_huggingface_adapter_counts_specials_and_exposes_offsets():
    counter = HuggingFaceTokenCounter(FakeFastTokenizer())
    assert counter.count("one two") == 4
    assert counter.content_offsets("one two") == ((0, 3), (4, 7))
    assert counter.advertised_limit == 16
    assert counter.resolved_revision == "runtime-revision"
    assert len(counter.fingerprint) == 64


def test_nomic_provider_resolves_runtime_contract_and_matryoshka_dimension():
    provider = get_provider("nomic", dimension=256, device="cpu")

    class FakeModel:
        tokenizer = FakeFastTokenizer()
        max_seq_length = 12

        @staticmethod
        def get_sentence_embedding_dimension():
            return 256

    provider._model = FakeModel()
    provider._initialized = True

    contract = provider.get_input_contract()
    assert provider.get_dimension() == 256
    assert contract.native_dimension == 768
    assert contract.output_dimension == 256
    assert contract.effective_context_limit == 12
    assert contract.model_revision == "runtime-revision"
    assert contract.render("doc", InputRole.DOCUMENT) == "search_document: doc"
    assert contract.render("q", InputRole.QUERY) == "search_query: q"


def test_specialized_provider_rejects_unsupported_matryoshka_dimension():
    with pytest.raises(ValueError, match="does not support 300 dimensions"):
        get_provider("nomic", dimension=300)


@pytest.mark.parametrize("model_id", sorted(OPEN_MODEL_CATALOG))
def test_every_catalog_model_resolves_same_runtime_contract_shape(model_id):
    spec = OPEN_MODEL_CATALOG[model_id]
    provider = create_open_model_provider(model_id, device="cpu")

    class FakeModel:
        tokenizer = FakeFastTokenizer()
        max_seq_length = spec.metadata.max_length

        @staticmethod
        def get_sentence_embedding_dimension():
            return spec.metadata.dimension

    provider._model = FakeModel()
    provider._initialized = True
    contract = provider.get_input_contract()

    assert contract.model_id == model_id
    assert contract.effective_context_limit == min(spec.metadata.max_length, 16)
    payload = "UNIQUE_PAYLOAD"
    assert contract.render(payload, InputRole.DOCUMENT).count(payload) == 1
    assert contract.render(payload, InputRole.QUERY).count(payload) == 1
    assert contract.document_encode_parameters == (
        spec.metadata.document_encode_parameters
    )
    assert contract.query_encode_parameters == spec.metadata.query_encode_parameters
    assert len(contract.fingerprint) == 64
    assert contract.to_manifest()["contract_fingerprint"] == contract.fingerprint
    assert spec.metadata.source_url == f"https://huggingface.co/{model_id}"


def test_catalog_models_cover_discrete_and_range_matryoshka_policies():
    gemma = OPEN_MODEL_CATALOG["google/embeddinggemma-300m"].metadata
    qwen = OPEN_MODEL_CATALOG["Qwen/Qwen3-Embedding-0.6B"].metadata
    assert gemma.supports_dimension(256)
    assert not gemma.supports_dimension(300)
    assert qwen.supports_dimension(300)
    assert not qwen.supports_dimension(31)


def test_role_specific_adapter_parameters_reach_model_encode():
    provider = create_open_model_provider("jinaai/jina-embeddings-v3")
    calls = []

    class FakeModel:
        def encode(self, texts, **kwargs):
            calls.append((texts, kwargs))
            return [[0.0] * 1024 for _ in texts]

    provider._model = FakeModel()
    provider._initialized = True
    provider.embed_query("query")
    provider.embed_passages(["document"])

    assert calls[0][1]["task"] == "retrieval.query"
    assert calls[1][1]["task"] == "retrieval.passage"


def test_open_model_plain_embed_uses_document_rendering():
    provider = create_open_model_provider("google/embeddinggemma-300m")
    captured = {}

    class FakeModel:
        def encode(self, texts, **kwargs):
            captured["texts"] = texts
            return [[0.0] * 768 for _ in texts]

    provider._model = FakeModel()
    provider._initialized = True
    provider.embed(["Mars is red"])
    assert captured["texts"] == ["title: none | text: Mars is red"]


def test_registered_runtime_prompts_override_catalog_templates_in_contract():
    provider = create_open_model_provider(
        "BAAI/bge-base-en-v1.5",
        extra={"prompts": {"query": "custom query: ", "document": "custom doc: "}},
    )

    class FakeModel:
        tokenizer = FakeFastTokenizer()
        max_seq_length = 512

        @staticmethod
        def get_sentence_embedding_dimension():
            return 768

    provider._model = FakeModel()
    provider._initialized = True
    contract = provider.get_input_contract()
    assert contract.render("x", InputRole.QUERY) == "custom query: x"
    assert contract.render("x", InputRole.DOCUMENT) == "custom doc: x"
