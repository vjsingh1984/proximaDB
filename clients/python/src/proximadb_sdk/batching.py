"""
ProximaDB Request Batching - Backward Compatibility Module

This module provides backward compatibility for the legacy batching interface.
All functionality has been moved to batching_unified.py
"""

# Import everything from unified module for backward compatibility
from .batching_unified import (
    AsyncBatchProcessor,
    BatchConfig,
    BatchMetrics,
    BatchOperationType,
    BatchProcessor,
    BatchRequest,
    BatchStrategy,
    RequestBatcher,
    RestBatchProcessor,
    ThreadedBatchProcessor,
    UnifiedBatchManager,
)

# Legacy alias
RequestBatcher = UnifiedBatchManager

__all__ = [
    "BatchStrategy",
    "BatchOperationType",
    "BatchConfig",
    "BatchMetrics",
    "BatchRequest",
    "BatchProcessor",
    "AsyncBatchProcessor",
    "ThreadedBatchProcessor",
    "UnifiedBatchManager",
    "RequestBatcher",
    "RestBatchProcessor",
]
