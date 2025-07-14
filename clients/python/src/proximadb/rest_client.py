"""
ProximaDB REST Client - Backward Compatibility Wrapper

This module is deprecated. Please use the unified client instead:

    from proximadb import ProximaDBClient, Protocol
    
    # For REST-specific usage:
    client = ProximaDBClient(url="localhost", protocol=Protocol.REST)

The unified client provides all the same functionality with automatic
protocol selection and better error handling.
"""

import warnings
from typing import Any, Dict, List, Optional, Union

from .unified_client import ProximaDBClient, Protocol
from .config import ClientConfig

class ProximaDBRestClient(ProximaDBClient):
    """Backward compatibility wrapper for REST client
    
    This is a thin wrapper around ProximaDBClient that forces REST protocol.
    All functionality is preserved.
    """
    
    def __init__(self, config: Optional[ClientConfig] = None, **kwargs):
        """Initialize REST client (deprecated - use ProximaDBClient instead)"""
        warnings.warn(
            "ProximaDBRestClient is deprecated. "
            "Please use ProximaDBClient(protocol=Protocol.REST) instead.",
            DeprecationWarning,
            stacklevel=2
        )
        
        # Force REST protocol
        super().__init__(
            config=config,
            protocol=Protocol.REST,
            **kwargs
        )

# For imports like "from proximadb.rest_client import ProximaDBClient"
ProximaDBClient = ProximaDBRestClient