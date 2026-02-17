"""
Intelligent Protocol Selection - Backward Compatibility Module

This module provides backward compatibility for the legacy ProtocolSelector interface.
All functionality has been moved to intelligent_router.py
"""

from .config import ClientConfig

# Import everything from unified module for backward compatibility
from .intelligent_router import (
    IntelligentRouter,
    OperationType,
    ProtocolHealth,
    ProtocolMetrics,
    RoutingConfig,
    RoutingRule,
)
from .intelligent_router import (
    RoutingStrategy as SelectionStrategy,  # Map to legacy name
)

# Legacy aliases
ProtocolSelector = IntelligentRouter


# Legacy factory function for backward compatibility
def create_protocol_selector(
    config: ClientConfig,
    strategy: SelectionStrategy = SelectionStrategy.BALANCED,
    grpc_factory=None,
    rest_factory=None,
    **kwargs,
) -> ProtocolSelector:
    """Create a protocol selector with backward-compatible interface"""
    from .config import Protocol

    # Filter out client factory functions from RoutingConfig kwargs
    routing_kwargs = {
        k: v for k, v in kwargs.items() if k not in ["grpc_factory", "rest_factory"]
    }

    routing_config = RoutingConfig(strategy=strategy, **routing_kwargs)
    selector = ProtocolSelector(config=routing_config, client_config=config)

    # Register the client factories if provided
    if grpc_factory:
        selector.register_client_factory(Protocol.GRPC, grpc_factory)
    if rest_factory:
        selector.register_client_factory(Protocol.REST, rest_factory)

    return selector


__all__ = [
    "OperationType",
    "SelectionStrategy",
    "ProtocolHealth",
    "RoutingRule",
    "ProtocolMetrics",
    "RoutingConfig",
    "IntelligentRouter",
    "ProtocolSelector",
    "create_protocol_selector",
]
