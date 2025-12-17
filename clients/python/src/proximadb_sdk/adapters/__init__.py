"""
ProximaDB Protocol Adapters

Protocol adapters encapsulate protocol-specific logic for REST, gRPC, and embedded modes.
This enables the unified client to delegate operations without conditional branches.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

from .base import BaseProtocolAdapter

__all__ = [
    "BaseProtocolAdapter",
    "RestProtocolAdapter",
    "GrpcProtocolAdapter",
    "EmbeddedProtocolAdapter",
    "get_rest_adapter",
    "get_grpc_adapter",
    "get_embedded_adapter",
    "create_adapter",
]


# Lazy imports to avoid circular dependencies
def get_rest_adapter():
    """Get REST adapter class (lazy import)."""
    from .rest_adapter import RestProtocolAdapter
    return RestProtocolAdapter


def get_grpc_adapter():
    """Get gRPC adapter class (lazy import)."""
    from .grpc_adapter import GrpcProtocolAdapter
    return GrpcProtocolAdapter


def get_embedded_adapter():
    """Get embedded adapter class (lazy import)."""
    from .embedded_adapter import EmbeddedProtocolAdapter
    return EmbeddedProtocolAdapter


def create_adapter(protocol: str, **kwargs) -> BaseProtocolAdapter:
    """Factory function to create appropriate adapter based on protocol.

    Args:
        protocol: Protocol type ('rest', 'grpc', 'embedded', 'auto')
        **kwargs: Protocol-specific configuration

    Returns:
        Configured protocol adapter instance

    Raises:
        ValueError: If protocol is unknown
    """
    protocol = protocol.lower()

    if protocol == 'embedded':
        adapter_cls = get_embedded_adapter()
        return adapter_cls(**kwargs)

    elif protocol == 'grpc':
        adapter_cls = get_grpc_adapter()
        return adapter_cls(**kwargs)

    elif protocol == 'rest':
        adapter_cls = get_rest_adapter()
        return adapter_cls(**kwargs)

    elif protocol == 'auto':
        # Try gRPC first, fallback to REST
        try:
            adapter_cls = get_grpc_adapter()
            return adapter_cls(**kwargs)
        except ImportError:
            adapter_cls = get_rest_adapter()
            return adapter_cls(**kwargs)

    else:
        raise ValueError(f"Unknown protocol: {protocol}. Supported: rest, grpc, embedded, auto")


# Lazy class imports for direct class access
def __getattr__(name):
    """Lazy loading of adapter classes."""
    if name == 'RestProtocolAdapter':
        from .rest_adapter import RestProtocolAdapter
        return RestProtocolAdapter
    elif name == 'GrpcProtocolAdapter':
        from .grpc_adapter import GrpcProtocolAdapter
        return GrpcProtocolAdapter
    elif name == 'EmbeddedProtocolAdapter':
        from .embedded_adapter import EmbeddedProtocolAdapter
        return EmbeddedProtocolAdapter
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
