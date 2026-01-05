"""
ProximaDB gRPC Async Client - DEPRECATED

DEPRECATION NOTICE: This async gRPC client is deprecated and has been replaced by:
- ProximaDBSyncGrpcClient (src/proximadb/protocols/grpc_sync.py) for synchronous gRPC
- REST client (src/proximadb/protocols/rest_sync.py) for HTTP/JSON operations

This module now redirects to the synchronous gRPC client.

Copyright 2025 ProximaDB
"""

import warnings
from .grpc_sync import ProximaDBSyncGrpcClient

# Issue deprecation warning when module is imported
warnings.warn(
    "grpc_async.ProximaDBClient is deprecated. "
    "Use grpc_sync.ProximaDBSyncGrpcClient instead.",
    DeprecationWarning,
    stacklevel=2,
)


class ProximaDBClient(ProximaDBSyncGrpcClient):
    """
    DEPRECATED: Use grpc_sync.ProximaDBSyncGrpcClient instead.

    This class now inherits from ProximaDBSyncGrpcClient for backward compatibility.
    All async functionality has been removed during v1 proto migration.

    Migration guide:
        # Old (deprecated):
        from proximadb_sdk.protocols.grpc_async import ProximaDBClient
        client = ProximaDBClient("localhost:5679")

        # New (recommended):
        from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient
        client = ProximaDBSyncGrpcClient("localhost:5679")
    """

    def __init__(self, endpoint: str = "localhost:5679", **kwargs):
        """
        Initialize gRPC client (deprecated, redirects to sync client).

        Args:
            endpoint: gRPC server endpoint (host:port)
            **kwargs: Additional arguments passed to ProximaDBSyncGrpcClient
        """
        warnings.warn(
            "grpc_async.ProximaDBClient is deprecated. "
            "Use grpc_sync.ProximaDBSyncGrpcClient instead.",
            DeprecationWarning,
            stacklevel=2,
        )
        super().__init__(endpoint, **kwargs)


# Alias for backward compatibility
AsyncGrpcClient = ProximaDBClient
