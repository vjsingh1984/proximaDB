"""
ProximaDB gRPC Client - Backward Compatibility Wrapper

This module is deprecated. Please use the unified client instead:

    from proximadb import ProximaDBClient, Protocol
    
    # For gRPC-specific usage:
    client = ProximaDBClient(url="localhost", protocol=Protocol.GRPC)

For async gRPC operations, the internal implementation is available at:
proximadb.protocols.grpc_async
"""

import warnings

# Import from protocols for backward compatibility
from .protocols.grpc_async import *

# Show deprecation warning
warnings.warn(
    "proximadb.grpc_client is deprecated. "
    "Please use ProximaDBClient(protocol=Protocol.GRPC) instead.",
    DeprecationWarning,
    stacklevel=2
)