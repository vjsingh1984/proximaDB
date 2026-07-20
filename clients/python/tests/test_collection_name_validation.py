"""Regression: short collection names must be accepted (TD-SDK-1 S2a / ADR-068).

The SDK's ``CollectionConfig`` used to reject names shorter than 8 characters
(``min_length=8`` + a ``validate_name_length`` field validator) to avoid
colliding with 7-char base62 collection IDs. The server long since relaxed that
vector-collection-era constraint for relational DDL (tables like ``part``,
``orders``, ``region``) and resolves collections by name OR id — so the SDK was
strictly stricter than the server and blocked valid creates. A local Azurite
round-trip (2026-07-20) caught it: ``create_collection("aztest")`` succeeded on
the server but the SDK rejected ``sdktest`` (7 chars) client-side.

This is the kind of facade bug the codegen-drift gate cannot see; it is owned by
the round-trip correctness gate (ADR-068 D5). These tests pin the relaxed
contract so it does not regress.
"""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from proximadb_sdk.models import CollectionConfig


@pytest.mark.parametrize("name", ["ab", "part", "orders", "region", "sdktest"])
def test_short_collection_names_are_accepted(name: str) -> None:
    """Names shorter than the old 8-char floor must validate (server allows them)."""
    cfg = CollectionConfig(name=name, dimension=4)
    assert cfg.name == name


def test_empty_collection_name_is_still_rejected() -> None:
    """The non-empty floor stays — only the >=8 floor was lifted."""
    with pytest.raises(ValidationError):
        CollectionConfig(name="", dimension=4)
    with pytest.raises(ValidationError):
        CollectionConfig(name="   ", dimension=4)


def test_whitespace_only_name_rejected() -> None:
    with pytest.raises(ValidationError):
        CollectionConfig(name="\t\n", dimension=4)
