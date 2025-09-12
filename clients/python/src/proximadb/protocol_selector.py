"""
Intelligent Protocol Selection - Backward Compatibility Module

This module provides backward compatibility for the legacy ProtocolSelector interface.
All functionality has been moved to intelligent_router.py
"""

# Import everything from unified module for backward compatibility
from .intelligent_router import (
    OperationType,
    RoutingStrategy as SelectionStrategy,  # Map to legacy name
    ProtocolHealth,
    RoutingRule,
    ProtocolMetrics,
    RoutingConfig,
    IntelligentRouter,
)
from .config import ClientConfig

# Legacy aliases
ProtocolSelector = IntelligentRouter

# Legacy factory function for backward compatibility
def create_protocol_selector(
    config: ClientConfig,
    strategy: SelectionStrategy = SelectionStrategy.BALANCED,
    **kwargs
) -> ProtocolSelector:
    """Create a protocol selector with backward-compatible interface"""
    routing_config = RoutingConfig(strategy=strategy, **kwargs)
    return ProtocolSelector(config=routing_config, client_config=config)

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