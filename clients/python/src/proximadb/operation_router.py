"""
Operation-Specific Routing - Backward Compatibility Module

This module provides backward compatibility for the legacy OperationRouter interface.
All functionality has been moved to intelligent_router.py
"""

# Import everything from unified module for backward compatibility
from .intelligent_router import (
    OperationType,
    RoutingStrategy,
    ProtocolHealth,
    RoutingRule,
    ProtocolMetrics,
    RoutingConfig,
    IntelligentRouter,
)
from .config import ClientConfig

# Legacy aliases
OperationRouter = IntelligentRouter

# Legacy factory function for backward compatibility
def create_operation_router(
    config: RoutingConfig = None,
    client_config: ClientConfig = None,
    **kwargs
) -> OperationRouter:
    """Create an operation router with backward-compatible interface"""
    routing_config = config or RoutingConfig(**kwargs)
    return OperationRouter(config=routing_config, client_config=client_config)

__all__ = [
    "OperationType",
    "RoutingStrategy",
    "ProtocolHealth", 
    "RoutingRule",
    "ProtocolMetrics",
    "RoutingConfig",
    "IntelligentRouter",
    "OperationRouter",
    "create_operation_router",
]