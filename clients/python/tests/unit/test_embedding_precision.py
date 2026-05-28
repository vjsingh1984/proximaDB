"""Python SDK: EmbeddingPrecision enum + CollectionConfig field.

TDD coverage for the per-collection canonical_embedding_precision option
the server already accepts on every protocol surface
(REST / gRPC / SQL DDL / Arrow Flight). The Python SDK is the most-used
client per CLAUDE.md, so this binding is what makes fp16 storage
discoverable to operators.

Mirrors the Rust SDK shape from `clients/rust/src/collection.rs` so a
team using both clients sees consistent semantics: same enum names,
same string forms (proto SCREAMING label / shorthand aliases / canonical
lowercase), same default (Fp32 — backward compatible with pre-rollout
CollectionConfig usage).
"""

import json

import pytest

from proximadb_sdk import CollectionConfig, EmbeddingPrecision

# ── EmbeddingPrecision enum ────────────────────────────────────────────────


def test_embedding_precision_has_all_proto_variants():
    """The SDK enum must cover every variant the server proto exposes
    (proto/proximadb/v1/collection_types.proto:EmbeddingPrecision)."""
    expected = {"FP32", "FP16", "BF16", "INT8", "UINT8"}
    actual = {p.name for p in EmbeddingPrecision}
    assert expected.issubset(actual), f"missing variants: {expected - actual}"


def test_embedding_precision_value_matches_server_label():
    """The enum value (str-backed) must match what the server's
    apply_proto_enum_workarounds accepts — lowercase, no prefix."""
    assert EmbeddingPrecision.FP32.value == "fp32"
    assert EmbeddingPrecision.FP16.value == "fp16"
    assert EmbeddingPrecision.BF16.value == "bf16"
    assert EmbeddingPrecision.INT8.value == "int8"
    assert EmbeddingPrecision.UINT8.value == "uint8"


def test_embedding_precision_str_constructor_accepts_canonical_form():
    """Operators write `EmbeddingPrecision('fp16')` from config files."""
    assert EmbeddingPrecision("fp16") == EmbeddingPrecision.FP16


def test_embedding_precision_serializes_as_lowercase_string_in_json():
    """The on-wire form must match what the server's REST workaround
    takes — lowercase string. This is the same shape the Rust SDK
    serializes (clients/rust tests: `embedding_precision_serializes_as_lowercase_string`).
    """
    # str-backed enums round-trip through json.dumps as their value
    encoded = json.dumps(EmbeddingPrecision.FP16.value)
    assert json.loads(encoded) == "fp16"


# ── CollectionConfig.canonical_embedding_precision ────────────────────────


def _valid_kwargs(**overrides):
    """Minimum kwargs to construct a valid CollectionConfig. Apply
    overrides for the field under test. CollectionConfig requires
    name >= 8 chars and dimension in [1, 65536]."""
    base = dict(name="precision_test", dimension=8)
    base.update(overrides)
    return base


def test_collection_config_default_precision_is_none_for_back_compat():
    """Existing callers that never touched the field must continue to
    produce the same serialized payload (no new key)."""
    cfg = CollectionConfig(**_valid_kwargs())
    assert cfg.canonical_embedding_precision is None


def test_collection_config_accepts_enum_value():
    cfg = CollectionConfig(
        **_valid_kwargs(canonical_embedding_precision=EmbeddingPrecision.FP16)
    )
    assert cfg.canonical_embedding_precision == EmbeddingPrecision.FP16


def test_collection_config_accepts_canonical_string():
    cfg = CollectionConfig(**_valid_kwargs(canonical_embedding_precision="fp16"))
    assert cfg.canonical_embedding_precision == EmbeddingPrecision.FP16


def test_collection_config_accepts_proto_screaming_label():
    """The server's proto-generated form
    (EMBEDDING_PRECISION_FP16) is what tonic / proto round-trips emit."""
    cfg = CollectionConfig(
        **_valid_kwargs(canonical_embedding_precision="EMBEDDING_PRECISION_FP16")
    )
    assert cfg.canonical_embedding_precision == EmbeddingPrecision.FP16


@pytest.mark.parametrize(
    "alias,expected",
    [
        ("f16", EmbeddingPrecision.FP16),
        ("half", EmbeddingPrecision.FP16),
        ("float16", EmbeddingPrecision.FP16),
        ("FP16", EmbeddingPrecision.FP16),  # case-insensitive
        ("bfloat16", EmbeddingPrecision.BF16),
        ("i8", EmbeddingPrecision.INT8),
        ("int8_scalar", EmbeddingPrecision.INT8),
        ("u8", EmbeddingPrecision.UINT8),
        ("uint8_scalar", EmbeddingPrecision.UINT8),
    ],
)
def test_collection_config_accepts_common_aliases(alias, expected):
    """Same alias surface the server's apply_proto_enum_workarounds
    (crates/platform/proximadb-api/src/rest/v1/catalog.rs) takes —
    SDK users see consistent semantics across REST / gRPC / pgwire."""
    cfg = CollectionConfig(**_valid_kwargs(canonical_embedding_precision=alias))
    assert cfg.canonical_embedding_precision == expected


def test_collection_config_rejects_unknown_precision_label():
    """Don't silently coerce typos. The server's WITH clause silently
    falls back to fp32 with a warn log, but the SDK should fail fast
    because the operator is right there to fix the typo."""
    with pytest.raises((ValueError, Exception)) as exc_info:
        CollectionConfig(
            **_valid_kwargs(canonical_embedding_precision="not_a_precision")
        )
    msg = str(exc_info.value).lower()
    assert (
        "precision" in msg or "not_a_precision" in msg
    ), f"error should mention the bad input or the field name; got: {exc_info.value}"


def test_collection_config_serializes_precision_to_wire_payload():
    """`model_dump()` must include canonical_embedding_precision when
    set so REST/gRPC clients can JSON-serialize the config directly.
    Field key must match what the server's
    catalog_schema_from_collection at services/collection/manager.rs
    reads (`canonical_embedding_precision`)."""
    cfg = CollectionConfig(
        **_valid_kwargs(canonical_embedding_precision=EmbeddingPrecision.FP16)
    )
    payload = cfg.model_dump(exclude_none=True)
    assert "canonical_embedding_precision" in payload
    # Either the enum or its string value — both deserialize on the
    # server side. The enum.value form is the canonical SDK output.
    value = payload["canonical_embedding_precision"]
    if hasattr(value, "value"):
        value = value.value
    assert value == "fp16"


def test_collection_config_omits_precision_when_default():
    """Back-compat: existing serialized configs without the field
    stay byte-identical."""
    cfg = CollectionConfig(**_valid_kwargs())
    payload = cfg.model_dump(exclude_none=True)
    assert "canonical_embedding_precision" not in payload
