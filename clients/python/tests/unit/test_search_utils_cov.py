"""Offline unit tests for proximadb_sdk.search_utils.

Pure helper functions: REST optimization builder, gRPC SearchParams builder,
the protocol dispatcher, and the SqlValue conversion helper. No network, no
server, no heavy deps. The v1 proto modules ship with the SDK so they import
cleanly offline.
"""

import pytest

from proximadb_sdk import search_utils
from proximadb_sdk.search_utils import (
    _python_value_to_sql_value,
    build_search_hints,
    build_search_optimization_rest,
    build_search_params_grpc,
)
from proximadb_sdk.v1 import types_pb2, vector_types_pb2

# ---------------------------------------------------------------------------
# build_search_optimization_rest
# ---------------------------------------------------------------------------


def test_rest_empty_returns_empty_dict():
    assert build_search_optimization_rest() == {}


def test_rest_scalar_fields_set():
    out = build_search_optimization_rest(
        top_k=10,
        filters={"k": "v"},
        accuracy_threshold=0.9,
        include_expired=True,
        timeout_ms=500,
        enable_two_stage=True,
    )
    assert out["top_k"] == 10
    assert out["filters"] == {"k": "v"}
    assert out["accuracy_threshold"] == 0.9
    assert out["include_expired"] is True
    assert out["timeout_ms"] == 500
    assert out["enable_two_stage"] is True


def test_rest_falsy_but_not_none_values():
    # top_k=0 is not None -> included; filters={} is falsy -> skipped
    out = build_search_optimization_rest(top_k=0, filters={}, include_expired=False)
    assert out["top_k"] == 0
    assert "filters" not in out
    assert out["include_expired"] is False


@pytest.mark.parametrize(
    "hint,expected",
    [
        ("none", {"hint_type": "none"}),
        ("no", {"hint_type": "none"}),
        ("fp32", {"hint_type": "none"}),
        ("float32", {"hint_type": "none"}),
        ("binary", {"hint_type": "binary"}),
        ("bin", {"hint_type": "binary"}),
        ("scalar", {"hint_type": "scalar", "parameters": {"bits": 8}}),
        ("int8", {"hint_type": "scalar", "parameters": {"bits": 8}}),
        ("int16", {"hint_type": "scalar", "parameters": {"bits": 16}}),
    ],
)
def test_rest_quantization_string_variants(hint, expected):
    out = build_search_optimization_rest(quantization_hint=hint)
    assert out["quantization_hint"] == expected


def test_rest_quantization_case_insensitive():
    out = build_search_optimization_rest(quantization_hint="BINARY")
    assert out["quantization_hint"] == {"hint_type": "binary"}


def test_rest_quantization_pq_with_bits():
    out = build_search_optimization_rest(quantization_hint="pq16")
    assert out["quantization_hint"] == {
        "hint_type": "product",
        "parameters": {"num_subvectors": 8, "bits_per_code": 16},
    }


def test_rest_quantization_pq_bare_defaults_to_8():
    out = build_search_optimization_rest(quantization_hint="pq")
    assert out["quantization_hint"] == {
        "hint_type": "product",
        "parameters": {"num_subvectors": 8, "bits_per_code": 8},
    }


def test_rest_quantization_pq_invalid_bits_fallback():
    # "pqXY" -> int("XY") raises ValueError -> default 8
    out = build_search_optimization_rest(quantization_hint="pqXY")
    assert out["quantization_hint"] == {
        "hint_type": "product",
        "parameters": {"num_subvectors": 8, "bits_per_code": 8},
    }


def test_rest_quantization_unknown_string_ignored():
    # an unrecognized string sets nothing
    out = build_search_optimization_rest(quantization_hint="weird")
    assert "quantization_hint" not in out


def test_rest_quantization_dict_passthrough():
    custom = {"hint_type": "custom", "parameters": {"x": 1}}
    out = build_search_optimization_rest(quantization_hint=custom)
    assert out["quantization_hint"] == custom


def test_rest_clustering_and_metadata_hints():
    out = build_search_optimization_rest(
        enable_clustering_hint=True,
        enable_metadata_filtering_hint=False,
    )
    assert out["enable_clustering_hint"] is True
    assert out["enable_metadata_filtering_hint"] is False


def test_rest_custom_hints_and_additional_params():
    out = build_search_optimization_rest(
        custom_hints={"a": 1},
        distance_metric="cosine",
        requires_ordering=True,
        candidate_multiplier=2.5,
    )
    assert out["custom_hints"] == {"a": 1}
    assert out["distance_metric"] == "cosine"
    assert out["requires_ordering"] is True
    assert out["candidate_multiplier"] == 2.5


def test_rest_streaming_creates_custom_hints():
    out = build_search_optimization_rest(
        streaming_buffer_size=64,
        streaming_concurrent_search=True,
        streaming_max_concurrent_tasks=4,
        streaming_batch_size=16,
    )
    ch = out["custom_hints"]
    assert ch["streaming_buffer_size"] == 64
    assert ch["streaming_concurrent_search"] is True
    assert ch["streaming_max_concurrent_tasks"] == 4
    assert ch["streaming_batch_size"] == 16


def test_rest_streaming_merges_into_existing_custom_hints():
    out = build_search_optimization_rest(
        custom_hints={"pre": "existing"},
        streaming_buffer_size=8,
    )
    assert out["custom_hints"]["pre"] == "existing"
    assert out["custom_hints"]["streaming_buffer_size"] == 8


def test_rest_no_streaming_no_custom_hints_key():
    out = build_search_optimization_rest(top_k=5)
    assert "custom_hints" not in out


def test_rest_streaming_without_buffer_size_branch():
    # `any([...])` is True via batch_size, but streaming_buffer_size is None
    # so the buffer_size assignment branch is skipped (108->112).
    out = build_search_optimization_rest(streaming_batch_size=4)
    ch = out["custom_hints"]
    assert "streaming_buffer_size" not in ch
    assert ch["streaming_batch_size"] == 4


def test_rest_streaming_concurrent_only():
    out = build_search_optimization_rest(streaming_concurrent_search=True)
    ch = out["custom_hints"]
    assert ch["streaming_concurrent_search"] is True
    assert "streaming_buffer_size" not in ch
    assert "streaming_batch_size" not in ch


def test_rest_quantization_string_then_clustering_branch():
    # quantization_hint is a non-matching str (enters str block, no sub-branch,
    # skips the dict elif at 79 -> falls through to 82) while clustering set.
    out = build_search_optimization_rest(
        quantization_hint="totally-unknown",
        enable_clustering_hint=True,
    )
    assert "quantization_hint" not in out
    assert out["enable_clustering_hint"] is True


# ---------------------------------------------------------------------------
# build_search_params_grpc
# ---------------------------------------------------------------------------


def test_grpc_returns_searchparams_proto():
    sp = build_search_params_grpc(top_k=7)
    assert isinstance(sp, vector_types_pb2.SearchParams)
    assert sp.top_k == 7


def test_grpc_scalar_fields():
    sp = build_search_params_grpc(
        top_k=3,
        accuracy_threshold=0.5,
        include_expired=True,
        timeout_ms=200,
        enable_two_stage=True,
        enable_clustering_hint=True,
        enable_metadata_filtering_hint=True,
    )
    assert sp.top_k == 3
    assert abs(sp.accuracy_threshold - 0.5) < 1e-6
    assert sp.include_expired is True
    assert sp.timeout_ms == 200
    assert sp.enable_two_stage is True
    assert sp.enable_clustering_hint is True
    assert sp.enable_metadata_filtering_hint is True


def test_grpc_custom_hints_and_extra_params_packed():
    sp = build_search_params_grpc(
        custom_hints={"label": "abc", "count": 5},
        distance_metric="euclidean",
        requires_ordering=True,
        candidate_multiplier=1.5,
        streaming_buffer_size=32,
        streaming_concurrent_search=False,
        streaming_max_concurrent_tasks=2,
        streaming_batch_size=8,
    )
    h = sp.custom_hints
    assert h["label"].string_value == "abc"
    assert h["count"].int64_value == 5
    assert h["distance_metric"].string_value == "euclidean"
    assert h["requires_ordering"].bool_value is True
    assert abs(h["candidate_multiplier"].number_value - 1.5) < 1e-6
    assert h["streaming_buffer_size"].int64_value == 32
    assert h["streaming_concurrent_search"].bool_value is False
    assert h["streaming_max_concurrent_tasks"].int64_value == 2
    assert h["streaming_batch_size"].int64_value == 8


def test_grpc_no_hints_means_empty_map():
    sp = build_search_params_grpc(top_k=1)
    assert len(sp.custom_hints) == 0


def test_grpc_import_error_path(monkeypatch):
    # Force the proto import inside the function to fail and assert the
    # ImportError is re-raised with the install hint.
    import builtins

    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "proximadb_sdk.v1" or name.startswith("proximadb_sdk.v1"):
            raise ImportError("no proto")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    with pytest.raises(ImportError, match="Proto modules not available"):
        build_search_params_grpc(top_k=1)


# ---------------------------------------------------------------------------
# _python_value_to_sql_value
# ---------------------------------------------------------------------------


def test_sqlvalue_none():
    sv = _python_value_to_sql_value(None, types_pb2)
    assert sv.WhichOneof("value") == "null_value"


def test_sqlvalue_bool():
    sv = _python_value_to_sql_value(True, types_pb2)
    assert sv.WhichOneof("value") == "bool_value"
    assert sv.bool_value is True


def test_sqlvalue_int():
    sv = _python_value_to_sql_value(42, types_pb2)
    assert sv.int64_value == 42


def test_sqlvalue_float():
    sv = _python_value_to_sql_value(3.14, types_pb2)
    assert abs(sv.number_value - 3.14) < 1e-6


def test_sqlvalue_str():
    sv = _python_value_to_sql_value("hello", types_pb2)
    assert sv.string_value == "hello"


@pytest.mark.parametrize("raw", [b"abc", bytearray(b"xy"), memoryview(b"z")])
def test_sqlvalue_bytes_like(raw):
    sv = _python_value_to_sql_value(raw, types_pb2)
    assert sv.bytes_value == bytes(raw)


def test_sqlvalue_list_nested():
    sv = _python_value_to_sql_value([1, "two", 3.0], types_pb2)
    vals = sv.array_value.values
    assert len(vals) == 3
    assert vals[0].int64_value == 1
    assert vals[1].string_value == "two"
    assert abs(vals[2].number_value - 3.0) < 1e-6


def test_sqlvalue_tuple():
    sv = _python_value_to_sql_value((1, 2), types_pb2)
    assert len(sv.array_value.values) == 2


def test_sqlvalue_dict_nested():
    sv = _python_value_to_sql_value({"a": 1, "b": "x"}, types_pb2)
    fields = sv.object_value.fields
    assert fields["a"].int64_value == 1
    assert fields["b"].string_value == "x"


def test_sqlvalue_dict_nonstring_key_coerced():
    sv = _python_value_to_sql_value({5: "v"}, types_pb2)
    assert sv.object_value.fields["5"].string_value == "v"


def test_sqlvalue_unknown_type_falls_back_to_str():
    class Weird:
        def __str__(self):
            return "weird-repr"

    sv = _python_value_to_sql_value(Weird(), types_pb2)
    assert sv.string_value == "weird-repr"


# ---------------------------------------------------------------------------
# build_search_hints dispatcher
# ---------------------------------------------------------------------------


def test_hints_rest_dispatch():
    out = build_search_hints("rest", top_k=4, distance_metric="cosine")
    assert isinstance(out, dict)
    assert out["top_k"] == 4
    assert out["distance_metric"] == "cosine"


def test_hints_rest_dispatch_case_insensitive():
    out = build_search_hints("REST", top_k=1)
    assert out["top_k"] == 1


def test_hints_grpc_dispatch():
    out = build_search_hints("grpc", top_k=2)
    assert isinstance(out, vector_types_pb2.SearchParams)
    assert out.top_k == 2


def test_hints_unknown_protocol_raises():
    with pytest.raises(ValueError, match="Unknown protocol"):
        build_search_hints("thrift", top_k=1)


def test_module_logger_present():
    assert search_utils.logger is not None
