"""Unit tests for the Haystack DocumentStore integration.

Two concerns are covered:

  * the Haystack 2.x filter-tree -> ProximaDB conversions (#204's
    ``_convert_haystack_filters`` for the search path, plus the new
    ``_haystack_filters_to_scan_filter`` / client-side ``_haystack_filter_matches``
    that back the metadata scan), and
  * ``filter_documents`` now performing a REAL vector-free metadata scan
    (``client.scan`` with cursor pagination) instead of the old dummy zero-vector
    ANN window — returning ALL matching documents, not an ANN-ordered/capped set.

Haystack is an optional dep, so its modules are stubbed into ``sys.modules``
before importing the integration (mirroring the offline framework-stub pattern
used by the other integration cov tests).
"""

from __future__ import annotations

import sys
import types
from unittest.mock import MagicMock

from proximadb_sdk.models import SearchResult


def _install_haystack_stubs() -> None:
    """Install minimal stubs for the haystack symbols the integration imports."""
    hs = types.ModuleType("haystack")
    hs.__spec__ = types.SimpleNamespace(name="haystack")

    def component(cls):  # @component decorator passthrough
        return cls

    def _output_types(**_kw):
        def deco(fn):
            return fn

        return deco

    component.output_types = _output_types  # type: ignore[attr-defined]
    hs.component = component
    hs.default_from_dict = lambda cls, data: cls(**data.get("init_parameters", {}))
    hs.default_to_dict = lambda obj, **kw: {"init_parameters": kw}

    dc = types.ModuleType("haystack.dataclasses")

    class Document:
        def __init__(self, id=None, content=None, meta=None, embedding=None):
            self.id = id
            self.content = content
            self.meta = meta or {}
            self.embedding = embedding

        def with_id(self, doc_id):
            self.id = doc_id
            return self

    dc.Document = Document

    ds = types.ModuleType("haystack.document_stores")
    ds_types = types.ModuleType("haystack.document_stores.types")

    class DuplicatePolicy:
        FAIL = "fail"
        OVERWRITE = "overwrite"
        SKIP = "skip"

    ds_types.DuplicatePolicy = DuplicatePolicy

    sys.modules["haystack"] = hs
    sys.modules["haystack.dataclasses"] = dc
    sys.modules["haystack.document_stores"] = ds
    sys.modules["haystack.document_stores.types"] = ds_types


_install_haystack_stubs()

# Evict any cached copy so the import binds against the stubs above.
sys.modules.pop("proximadb_sdk.integrations.haystack", None)
from proximadb_sdk.integrations import haystack as hs_mod  # noqa: E402

# ---------------------------------------------------------------------------
# #204 search-path conversion (kept intact)
# ---------------------------------------------------------------------------


def test_convert_haystack_filters_equality_and_ops():
    assert hs_mod._convert_haystack_filters(None) is None
    # bare equality -> {field: value}
    assert hs_mod._convert_haystack_filters(
        {"field": "meta.author", "operator": "==", "value": "x"}
    ) == {"author": "x"}
    # comparison -> $-prefixed op
    assert hs_mod._convert_haystack_filters(
        {"field": "age", "operator": ">", "value": 30}
    ) == {"age": {"$gt": 30}}
    # logical AND tree
    assert hs_mod._convert_haystack_filters(
        {
            "operator": "AND",
            "conditions": [
                {"field": "a", "operator": "==", "value": 1},
                {"field": "b", "operator": ">=", "value": 2},
            ],
        }
    ) == {"and": [{"a": 1}, {"b": {"$gte": 2}}]}


# ---------------------------------------------------------------------------
# scan-filter conversion (pushdown subset) + client-side matcher (full tree)
# ---------------------------------------------------------------------------


def test_scan_filter_flattens_and_of_comparisons():
    flt = hs_mod._haystack_filters_to_scan_filter(
        {
            "operator": "AND",
            "conditions": [
                {"field": "meta.author", "operator": "==", "value": "ann"},
                {"field": "year", "operator": ">", "value": 2020},
                {"field": "tag", "operator": "in", "value": ["a", "b"]},
            ],
        }
    )
    assert flt == [
        {"field": "author", "op": "eq", "value": "ann"},
        {"field": "year", "op": "gt", "value": 2020},
        {"field": "tag", "op": "in", "value": ["a", "b"]},
    ]


def test_scan_filter_bare_comparison():
    assert hs_mod._haystack_filters_to_scan_filter(
        {"field": "k", "operator": "==", "value": "v"}
    ) == [{"field": "k", "op": "eq", "value": "v"}]


def test_scan_filter_skips_or_not_and_unsupported_ops_for_pushdown():
    # OR cannot be pushed down to the flat scan filter -> nothing pushed.
    assert (
        hs_mod._haystack_filters_to_scan_filter(
            {
                "operator": "OR",
                "conditions": [
                    {"field": "a", "operator": "==", "value": 1},
                    {"field": "b", "operator": "==", "value": 2},
                ],
            }
        )
        is None
    )
    # "not in" has no scan op -> skipped from pushdown.
    assert (
        hs_mod._haystack_filters_to_scan_filter(
            {"field": "a", "operator": "not in", "value": [1, 2]}
        )
        is None
    )


def test_client_side_matcher_full_tree():
    m = hs_mod._haystack_filter_matches
    # AND
    f = {
        "operator": "AND",
        "conditions": [
            {"field": "a", "operator": "==", "value": 1},
            {"field": "b", "operator": ">", "value": 5},
        ],
    }
    assert m({"a": 1, "b": 6}, f) is True
    assert m({"a": 1, "b": 5}, f) is False
    # OR
    f_or = {
        "operator": "OR",
        "conditions": [
            {"field": "a", "operator": "==", "value": 1},
            {"field": "a", "operator": "==", "value": 2},
        ],
    }
    assert m({"a": 2}, f_or) is True
    assert m({"a": 3}, f_or) is False
    # NOT
    f_not = {
        "operator": "NOT",
        "conditions": [{"field": "a", "operator": "==", "value": 1}],
    }
    assert m({"a": 1}, f_not) is False
    assert m({"a": 2}, f_not) is True
    # not in (the op the scan endpoint cannot push down)
    f_nin = {"field": "a", "operator": "not in", "value": [1, 2]}
    assert m({"a": 3}, f_nin) is True
    assert m({"a": 1}, f_nin) is False
    # None filter matches everything
    assert m({"a": 1}, None) is True


# ---------------------------------------------------------------------------
# filter_documents — real scan + pagination, returns ALL matches
# ---------------------------------------------------------------------------


def _store(client):
    return hs_mod.ProximaDBDocumentStore(
        client=client, collection_name="docs", embedding_dim=4
    )


def _result(rid, meta):
    return SearchResult(id=rid, score=0.0, metadata=meta, source=None)


def test_filter_documents_uses_scan_not_search():
    client = MagicMock()
    # ALL docs returned by the scan (no top_k cap, no ANN ordering).
    client.scan = MagicMock(
        return_value=[
            _result("1", {"author": "ann", "content": "doc one"}),
            _result("2", {"author": "ann", "content": "doc two"}),
        ]
    )
    # search must NOT be called anymore (no dummy-vector ANN path).
    client.search = MagicMock(side_effect=AssertionError("search must not be used"))

    store = _store(client)
    docs = store.filter_documents(
        {"field": "meta.author", "operator": "==", "value": "ann"}
    )

    assert [d.id for d in docs] == ["1", "2"]
    assert [d.content for d in docs] == ["doc one", "doc two"]
    # scan was invoked with the pushed-down AND-flattened filter.
    client.scan.assert_called_once()
    _, kwargs = client.scan.call_args
    assert kwargs["filter"] == [{"field": "author", "op": "eq", "value": "ann"}]
    assert kwargs["max_rows"] == store._max_filter_window
    client.search.assert_not_called()


def test_filter_documents_client_side_narrows_or_tree():
    # Server cannot push down OR, so scan returns the unfiltered superset; the
    # client-side matcher must narrow it to the true match set.
    client = MagicMock()
    client.scan = MagicMock(
        return_value=[
            _result("1", {"tier": "gold"}),
            _result("2", {"tier": "silver"}),
            _result("3", {"tier": "bronze"}),
        ]
    )
    store = _store(client)

    docs = store.filter_documents(
        {
            "operator": "OR",
            "conditions": [
                {"field": "tier", "operator": "==", "value": "gold"},
                {"field": "tier", "operator": "==", "value": "bronze"},
            ],
        }
    )

    assert sorted(d.id for d in docs) == ["1", "3"]
    # OR isn't pushable -> no server-side filter sent.
    _, kwargs = client.scan.call_args
    assert kwargs["filter"] is None


def test_filter_documents_unfiltered_returns_all():
    client = MagicMock()
    client.scan = MagicMock(
        return_value=[_result(str(i), {"content": f"d{i}"}) for i in range(5)]
    )
    store = _store(client)
    docs = store.filter_documents(None)
    assert len(docs) == 5
    _, kwargs = client.scan.call_args
    assert kwargs["filter"] is None
