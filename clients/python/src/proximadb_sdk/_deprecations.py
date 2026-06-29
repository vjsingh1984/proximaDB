"""Centralized deprecation signals for the ``proximadb_sdk`` public API.

One module = one source of truth for the deprecation message text and the
``stacklevel`` reasoning, so every public facade that must steer callers off a
legacy surface emits the identical, on-message warning (DRY). The matching
in-repo convention is a per-module ``_warn_*_deprecated()`` helper
(``chunking_strategies/code.py``, ``embedding_providers/__init__.py``); that
style is lifted to a shared module here only because the *same* message is shared
across three facades in three different modules (SRP: this module owns the
warning; the facades own document writes).
"""

from __future__ import annotations

import warnings

__all__ = ["warn_insert_document_deprecated"]


def warn_insert_document_deprecated(
    *, is_async: bool = False, stacklevel: int = 3
) -> None:
    """Emit the ADR-041 (P3) ``DeprecationWarning`` for ``insert_document``.

    Behavior-preserving: this ONLY warns; the legacy method still runs exactly as
    before. ``insert_document`` is removed in a future minor release (ADR-041 P5).

    Args:
        is_async: When ``True``, append a note that an async ``ingest_documents``
            variant is planned. The async ``EmbeddedProximaDB.insert_document``
            points at the sync surface for now.
        stacklevel: Forwarded to :func:`warnings.warn`. Lands the reported
            ``filename:lineno`` on the frame ``stacklevel - 1`` calls above the
            ``warn()`` call, so the user sees THEIR call site, not SDK internals.
            For the standard wiring (user -> deprecated public method -> this
            helper -> ``warn``), ``3`` is correct: ``1`` = this helper,
            ``2`` = the deprecated method body, ``3`` = the user's call.
    """
    async_note = (
        " An async ingest_documents variant is planned; until then call the "
        "sync ingest_documents, or run it in a worker thread."
        if is_async
        else ""
    )
    warnings.warn(
        "insert_document() is deprecated (ADR-041 spec-driven-primary P3). Use "
        "ingest_documents() for document writes; it targets the canonical "
        "POST /api/v2/collections/{collection_id}/documents route and surfaces "
        "server errors instead of silently falling back to an in-memory "
        "repository. insert_document() will be removed in a future minor "
        f"release.{async_note}",
        DeprecationWarning,
        stacklevel=stacklevel,
    )
