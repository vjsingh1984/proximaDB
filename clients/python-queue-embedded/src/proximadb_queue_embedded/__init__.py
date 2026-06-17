"""Public Python facade over the PyO3 `_native` extension.

Importing from ``proximadb_queue_embedded`` is the supported surface;
``_native`` is an implementation detail and may change.
"""
from __future__ import annotations

from ._native import (  # type: ignore[import-not-found]
    Consumer,
    Message,
    MessageReceipt,
    Producer,
    QueueClient,
    partition_for,
)

__all__ = [
    "Consumer",
    "Message",
    "MessageReceipt",
    "Producer",
    "QueueClient",
    "partition_for",
]
