"""A failed query leg must be distinguishable from one that matched nothing.

A multi-modal query runs several legs and fuses what they return. Each leg used
to `except Exception: return []`, so a leg that FAILED and a leg that
legitimately matched nothing produced the same thing, and the caller received a
partial answer presented as complete.

For a database those are different facts. These tests assert both directions,
because asserting only the failure case would pass against a build that flagged
*everything* partial.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

from proximadb_sdk.multimodal_query import MultiModalQueryResult


def _result(**kw):
    base = dict(
        records=[],
        total_count=0,
        query_time_ms=1.0,
        component_times={"vector_0": 1.0},
        fusion_strategy="rrf",
    )
    base.update(kw)
    return MultiModalQueryResult(**base)


def test_an_empty_but_successful_query_is_not_partial():
    """The direction that stops this becoming "always partial"."""
    assert _result().partial is False
    _result().raise_if_partial()  # must not raise


def test_a_failed_component_makes_the_result_partial():
    r = _result(component_errors={"vector_0": "ConnectionError: refused"})
    assert r.partial is True
    with pytest.raises(RuntimeError, match="partial"):
        r.raise_if_partial()


def test_the_failure_names_the_component_and_the_cause():
    """A partial flag with no attribution just moves the mystery."""
    r = _result(component_errors={"graph_1": "TimeoutError: deadline exceeded"})
    with pytest.raises(RuntimeError) as excinfo:
        r.raise_if_partial()
    message = str(excinfo.value)
    assert "graph_1" in message
    assert "TimeoutError" in message


def test_a_leg_that_raises_is_recorded_rather_than_swallowed():
    """The end-to-end property, exercised through `execute`.

    This is the test that fails against the pre-fix code: the leg raised, the
    old `except Exception: return []` ate it, and the result looked complete.
    """
    from proximadb_sdk.multimodal_query import MultiModalQueryExecutor

    client = MagicMock()
    client.search = MagicMock(side_effect=ConnectionError("refused"))
    engine = MultiModalQueryExecutor(client)

    query = MagicMock()
    query.components = [
        {"type": "vector", "collection": "c", "query_vector": [0.1], "top_k": 5}
    ]
    query.joins = []
    query.fusion_strategy = "rrf"
    query.fusion_weights = None
    query.time_decay = None
    query.custom_scorer = None
    query.offset = 0
    query.limit = 10

    result = engine.execute(query)

    assert result.partial is True, (
        "a leg raised ConnectionError and the result did not say so -- that is "
        "the silent-partial defect this module exists to prevent"
    )
    assert "vector_0" in result.component_errors
    assert "ConnectionError" in result.component_errors["vector_0"]
    assert result.records == []
