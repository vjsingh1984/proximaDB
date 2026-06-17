"""
ProximaDB Protocol Implementations

Internal protocol implementations for the unified client.
The Arrow Flight client is also exposed for high-throughput bulk operations.
"""

# Arrow Flight is exposed for direct bulk operations
try:
    from .arrow_flight import (
        ArrowFlightClient,
        FlightExchangeResult,
        FlightPutResult,
        FlightSearchResult,
        WriteMode,
        arrow_table_to_vectors,
        vectors_to_arrow_table,
    )

    ARROW_FLIGHT_AVAILABLE = True
except ImportError:
    ARROW_FLIGHT_AVAILABLE = False
    ArrowFlightClient = None
    FlightExchangeResult = None
    FlightPutResult = None
    FlightSearchResult = None
    WriteMode = None
    vectors_to_arrow_table = None
    arrow_table_to_vectors = None

__all__ = [
    "ArrowFlightClient",
    "FlightExchangeResult",
    "FlightPutResult",
    "FlightSearchResult",
    "WriteMode",
    "vectors_to_arrow_table",
    "arrow_table_to_vectors",
    "ARROW_FLIGHT_AVAILABLE",
]
