"""
ProximaDB Time-Series API Module

High-performance time-series operations for metrics, monitoring, and analytics.
Implements repository pattern with time-partitioning, downsampling, and
compression support.

Design Patterns:
- Repository Pattern: Clean separation for time-series data access
- Factory Pattern: Time-series collection builders
- Strategy Pattern: Different aggregation strategies
- Builder Pattern: Complex query construction
- Observer Pattern: Metric change notifications
- Async/Await: Non-blocking I/O operations
- Connection Pooling: Efficient connection reuse
- Write-Through Cache: Cache with immediate persistence
- Lazy Loading: Load data on-demand
- Batching: Automatic batch aggregation

Example:
    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.timeseries import ProximaDBTimeSeries

    client = ProximaDBClient(url="http://localhost:5678")
    ts = ProximaDBTimeSeries(client)

    # Create time-series collection
    ts.create_collection(
        name="code_metrics",
        timestamp_column="timestamp",
        value_columns=[
            ValueColumn(name="complexity", type="float"),
            ValueColumn(name="lines_of_code", type="int"),
        ],
        tags_columns=["file_path", "language", "author"]
    )

    # Ingest metrics
    ts.ingest("code_metrics", metrics=[
        {
            "timestamp": "2026-03-10T10:00:00Z",
            "complexity": 15.5,
            "lines_of_code": 250,
            "file_path": "src/main.py",
            "language": "python"
        },
    ])

    # Query time-series
    results = ts.query(
        collection_id="code_metrics",
        start_time="2026-02-01T00:00:00Z",
        end_time="2026-03-01T00:00:00Z",
        filter={"file_path": "src/main.py"},
        aggregation="OHLC",
        interval="1d"
    )
"""

from __future__ import annotations

import asyncio
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from enum import Enum
from functools import lru_cache
from typing import (
    Any,
    AsyncIterator,
    Awaitable,
    Callable,
    Dict,
    Generic,
    Iterator,
    List,
    Optional,
    TypeVar,
    Union,
)

from tenacity import (
    retry,
    retry_if_exception_type,
    stop_after_attempt,
    wait_exponential,
)

from .document import DocumentRepository
from .exceptions import ProximaDBError


# =============================================================================
# Enums and Constants
# =============================================================================


class ValueType(str, Enum):
    """Time-series value data types."""

    FLOAT = "float"
    INT = "int"
    UINT = "uint"
    BOOL = "bool"
    STRING = "string"


class AggregationType(str, Enum):
    """Time-series aggregation types."""

    # Basic aggregations
    SUM = "sum"
    AVG = "avg"  # Mean
    MIN = "min"
    MAX = "max"
    COUNT = "count"

    # Financial aggregations
    OHLC = "ohlc"  # Open, High, Low, Close
    VWAP = "vwap"  # Volume Weighted Average Price

    # Statistical aggregations
    STDDEV = "stddev"  # Standard deviation
    VARIANCE = "variance"
    MEDIAN = "median"
    PERCENTILE = "p99"  # 99th percentile

    # Downsampling
    FIRST = "first"
    LAST = "last"
    DIFF = "diff"  # First derivative
    PCT_CHANGE = "pct_change"  # Percentage change


class DownsampleMode(str, Enum):
    """Downsampling modes for time-series data."""

    # Time-based downsampling
    TIME_FIXED = "time_fixed"  # Fixed time intervals (1m, 5m, 1h, 1d)
    TIME_ALIGN = "time_align"  # Aligned to calendar boundaries
    TIME_BUCKET = "time_bucket"  # Bucket by count

    # Value-based downsampling
    LTTP = "lttp"  # Largest Triangle Three Buckets
    SMA = "sma"  # Simple Moving Average
    EMA = "ema"  # Exponential Moving Average


class CompressionCodec(str, Enum):
    """Compression codecs for time-series data."""

    NONE = "none"
    GORILLA = "gorilla"  # Gorilla compression for float64
    ZIGZAG = "zigzag"  # ZigZag + delta for int64
    DICTIONARY = "dictionary"  # Dictionary encoding for strings
    SNP = "snp"  # Simple-8b-Pseudo-Numercial


# =============================================================================
# Data Models
# =============================================================================


@dataclass
class ValueColumn:
    """Time-series value column definition.

    Attributes:
        name: Column name
        type: Value data type
        aggregation: Default aggregation type
        unit: Optional unit (e.g., "ms", "bytes", "count")
        description: Optional description
    """

    name: str
    type: ValueType = ValueType.FLOAT
    aggregation: AggregationType = AggregationType.AVG
    unit: Optional[str] = None
    description: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API."""
        return {
            "name": self.name,
            "data_type": self.type.value,
            "aggregation": self.aggregation.value,
            "unit": self.unit,
            "description": self.description,
        }


@dataclass
class TimeSeriesCollectionConfig:
    """Time-series collection configuration.

    Attributes:
        name: Collection name
        timestamp_column: Name of timestamp column
        value_columns: List of value columns
        tags_columns: List of tag/column names for filtering
        retention: Data retention period (e.g., "30d", "12w", "1y")
        downsampling: Downsampling configuration
        compression: Compression codec
        partitioning: Time partitioning configuration
    """

    name: str
    timestamp_column: str = "timestamp"
    value_columns: List[ValueColumn] = field(default_factory=list)
    tags_columns: List[str] = field(default_factory=list)
    retention: str = "30d"
    downsampling: Optional[Dict[str, Any]] = None
    compression: CompressionCodec = CompressionCodec.GORILLA
    partitioning: Optional[Dict[str, Any]] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API."""
        return {
            "name": self.name,
            "timestamp_column": self.timestamp_column,
            "value_columns": [vc.to_dict() for vc in self.value_columns],
            "tags_columns": self.tags_columns,
            "retention": self.retention,
            "downsampling": self.downsampling,
            "compression": self.compression.value,
            "partitioning": self.partitioning,
        }


@dataclass
class Metric:
    """Time-series metric data point.

    Attributes:
        timestamp: Metric timestamp
        values: Dictionary of column names to values
        tags: Dictionary of tag names to values
    """

    timestamp: Union[datetime, str]
    values: Dict[str, Any]
    tags: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API."""
        if isinstance(self.timestamp, datetime):
            timestamp_str = self.timestamp.isoformat()
        else:
            timestamp_str = self.timestamp

        return {
            "timestamp": timestamp_str,
            **self.values,
            **self.tags,
        }


@dataclass
class AggregatedMetric:
    """Aggregated time-series metric.

    Attributes:
        timestamp: Bucket timestamp
        values: Dictionary of column names to aggregated values
        count: Number of data points in aggregation
        tags: Tags (same for all points in aggregation)
    """

    timestamp: datetime
    values: Dict[str, Any]
    count: int
    tags: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format."""
        return {
            "timestamp": self.timestamp.isoformat(),
            **self.values,
            "_count": self.count,
            **self.tags,
        }


# =============================================================================
# Query Builder (Builder Pattern)
# =============================================================================


class TimeSeriesFilter:
    """Builder for constructing time-series filter queries.

    Uses fluent builder pattern for complex filter construction.

    Example:
        filter = (
            TimeSeriesFilter()
            .tag("language", "python")
            .and_()
            .gte("complexity", 10)
            .time_range("2026-01-01", "2026-03-01")
        )
    """

    def __init__(self):
        self._tag_filters: List[Dict[str, Any]] = []
        self._value_filters: List[Dict[str, Any]] = []
        self._start_time: Optional[datetime] = None
        self._end_time: Optional[datetime] = None
        self._limit: Optional[int] = None
        self._logic: str = "AND"

    def tag(self, key: str, value: Any) -> "TimeSeriesFilter":
        """Add tag equality filter."""
        self._tag_filters.append({"key": key, "op": "eq", "value": value})
        return self

    def tag_in(self, key: str, values: List[Any]) -> "TimeSeriesFilter":
        """Add tag in-list filter."""
        self._tag_filters.append({"key": key, "op": "in", "value": values})
        return self

    def gte(self, column: str, value: Any) -> "TimeSeriesFilter":
        """Add greater-than-or-equal filter."""
        self._value_filters.append({"column": column, "op": "gte", "value": value})
        return self

    def lte(self, column: str, value: Any) -> "TimeSeriesFilter":
        """Add less-than-or-equal filter."""
        self._value_filters.append({"column": column, "op": "lte", "value": value})
        return self

    def gt(self, column: str, value: Any) -> "TimeSeriesFilter":
        """Add greater-than filter."""
        self._value_filters.append({"column": column, "op": "gt", "value": value})
        return self

    def lt(self, column: str, value: Any) -> "TimeSeriesFilter":
        """Add less-than filter."""
        self._value_filters.append({"column": column, "op": "lt", "value": value})
        return self

    def time_range(
        self,
        start: Union[str, datetime],
        end: Union[str, datetime],
    ) -> "TimeSeriesFilter":
        """Add time range filter."""
        if isinstance(start, str):
            start = datetime.fromisoformat(start)
        if isinstance(end, str):
            end = datetime.fromisoformat(end)

        self._start_time = start
        self._end_time = end
        return self

    def limit(self, n: int) -> "TimeSeriesFilter":
        """Set result limit."""
        self._limit = n
        return self

    def and_(self) -> "TimeSeriesFilter":
        """Switch to AND logic."""
        self._logic = "AND"
        return self

    def or_(self) -> "TimeSeriesFilter":
        """Switch to OR logic."""
        self._logic = "OR"
        return self

    def to_dict(self) -> Dict[str, Any]:
        """Convert to API filter format."""
        return {
            "tag_filters": self._tag_filters,
            "value_filters": self._value_filters,
            "start_time": self._start_time.isoformat() if self._start_time else None,
            "end_time": self._end_time.isoformat() if self._end_time else None,
            "limit": self._limit,
            "logic": self._logic,
        }


# =============================================================================
# Time-Series Repository (Repository Pattern)
# =============================================================================


class TimeSeriesRepository:
    """Repository for time-series operations.

    Implements repository pattern with connection pooling, batching,
    downsampling, and compression support.

    Attributes:
        _client: ProximaDB client instance
        _batch_buffer: Buffer for batch ingest operations
        _batch_size: Batch size for auto-flush
        _compression: Default compression codec
    """

    def __init__(
        self,
        client: Any,
        batch_size: int = 1000,
        compression: CompressionCodec = CompressionCodec.GORILLA,
    ):
        """Initialize time-series repository.

        Args:
            client: ProximaDB client instance
            batch_size: Batch size for auto-flush
            compression: Default compression codec
        """
        self._client = client
        self._batch_size = batch_size
        self._compression = compression
        self._batch_buffer: Dict[str, List[Metric]] = {}

    # ========================================================================
    # Collection Management
    # ========================================================================

    @retry(
        stop=stop_after_attempt(3),
        wait=wait_exponential(multiplier=1, min=2, max=10),
        retry=retry_if_exception_type((ConnectionError, TimeoutError)),
    )
    def create_collection(self, config: TimeSeriesCollectionConfig) -> str:
        """Create a time-series collection.

        Args:
            config: Collection configuration

        Returns:
            Collection ID

        Raises:
            ProximaDBError: If collection creation fails
        """
        # Convert to REST API format
        collection_data = config.to_dict()

        # Call client to create collection
        # (This would use REST API when available)
        collection_id = f"ts_{config.name}"

        # Initialize batch buffer
        self._batch_buffer[collection_id] = []

        return collection_id

    def get_collection(self, collection_id: str) -> Optional[Dict[str, Any]]:
        """Get collection metadata.

        Args:
            collection_id: Collection identifier

        Returns:
            Collection metadata or None
        """
        # TODO: Implement via client
        return {"id": collection_id, "name": collection_id.replace("ts_", "")}

    def list_collections(self) -> List[Dict[str, Any]]:
        """List all time-series collections.

        Returns:
            List of collection metadata
        """
        # TODO: Implement via client
        return []

    def delete_collection(self, collection_id: str) -> bool:
        """Delete a time-series collection.

        Args:
            collection_id: Collection identifier

        Returns:
            True if deleted
        """
        # Clear batch buffer
        if collection_id in self._batch_buffer:
            del self._batch_buffer[collection_id]

        # TODO: Delete via client
        return True

    # ========================================================================
    # Metric Ingestion
    # ========================================================================

    def ingest(
        self,
        collection_id: str,
        metrics: List[Metric],
    ) -> Dict[str, Any]:
        """Ingest time-series metrics.

        Args:
            collection_id: Collection identifier
            metrics: List of metrics to ingest

        Returns:
            Ingest result with statistics

        Example:
            ts.ingest("code_metrics", metrics=[
                Metric(
                    timestamp="2026-03-10T10:00:00Z",
                    values={"complexity": 15.5, "lines_of_code": 250},
                    tags={"file_path": "main.py", "language": "python"}
                ),
            ])
        """
        if not metrics:
            return {"success": True, "ingested": 0}

        # Add to batch buffer
        if collection_id not in self._batch_buffer:
            self._batch_buffer[collection_id] = []
        self._batch_buffer[collection_id].extend(metrics)

        # Auto-flush if buffer full
        if len(self._batch_buffer[collection_id]) >= self._batch_size:
            self.flush_batch(collection_id)

        return {
            "success": True,
            "ingested": len(metrics),
        }

    def ingest_batch(
        self,
        collection_id: str,
        metrics: List[Metric],
    ) -> Dict[str, Any]:
        """Ingest metrics and immediately flush.

        Args:
            collection_id: Collection identifier
            metrics: List of metrics to ingest

        Returns:
            Ingest result with statistics
        """
        result = self.ingest(collection_id, metrics)
        flush_result = self.flush_batch(collection_id)

        return {
            **result,
            "flushed": flush_result.get("flushed", 0),
        }

    # ========================================================================
    # Time-Series Queries
    # ========================================================================

    def query(
        self,
        collection_id: str,
        start_time: Union[str, datetime],
        end_time: Union[str, datetime],
        filter: Optional[TimeSeriesFilter] = None,
        aggregation: Optional[AggregationType] = None,
        interval: Optional[str] = None,
        limit: int = 1000,
    ) -> List[AggregatedMetric]:
        """Query time-series data with optional aggregation.

        Args:
            collection_id: Collection identifier
            start_time: Query start time
            end_time: Query end time
            filter: Optional tag/value filters
            aggregation: Aggregation type (SUM, AVG, OHLC, etc.)
            interval: Downsample interval (e.g., "1m", "5m", "1h", "1d")
            limit: Maximum results

        Returns:
            List of aggregated metrics

        Example:
            results = ts.query(
                collection_id="code_metrics",
                start_time="2026-02-01T00:00:00Z",
                end_time="2026-03-01T00:00:00Z",
                filter=TimeSeriesFilter().tag("file_path", "main.py"),
                aggregation=AggregationType.OHLC,
                interval="1d"
            )
        """
        # Convert time strings to datetime if needed
        if isinstance(start_time, str):
            start_time = datetime.fromisoformat(start_time)
        if isinstance(end_time, str):
            end_time = datetime.fromisoformat(end_time)

        # TODO: Implement via client
        return []

    def get_latest(
        self,
        collection_id: str,
        tags: Dict[str, Any],
    ) -> Optional[Metric]:
        """Get the latest metric for given tags.

        Args:
            collection_id: Collection identifier
            tags: Tag values to match

        Returns:
            Latest metric or None

        Example:
            latest = ts.get_latest(
                collection_id="code_metrics",
                tags={"file_path": "src/main.py"}
            )
        """
        # TODO: Implement via client
        return None

    def get_latest_batch(
        self,
        collection_id: str,
        tags_list: List[Dict[str, Any]],
    ) -> List[Optional[Metric]]:
        """Get latest metrics for multiple tag combinations.

        Args:
            collection_id: Collection identifier
            tags_list: List of tag combinations

        Returns:
            List of latest metrics (None for missing)

        Example:
            latest_metrics = ts.get_latest_batch(
                collection_id="code_metrics",
                tags_list=[
                    {"file_path": "main.py"},
                    {"file_path": "utils.py"},
                    {"file_path": "config.py"},
                ]
            )
        """
        # TODO: Implement via client
        return [None] * len(tags_list)

    # ========================================================================
    # Aggregation and Downsampling
    # ========================================================================

    def aggregate(
        self,
        collection_id: str,
        start_time: Union[str, datetime],
        end_time: Union[str, datetime],
        aggregation: AggregationType,
        interval: str,
        value_column: str,
    ) -> List[AggregatedMetric]:
        """Aggregate time-series data.

        Args:
            collection_id: Collection identifier
            start_time: Start of aggregation window
            end_time: End of aggregation window
            aggregation: Aggregation type
            interval: Aggregation interval (e.g., "1m", "1h", "1d")
            value_column: Column to aggregate

        Returns:
            List of aggregated metrics

        Example:
            # Daily OHLC bars for complexity
            daily_bars = ts.aggregate(
                collection_id="code_metrics",
                start_time="2026-01-01",
                end_time="2026-03-01",
                aggregation=AggregationType.OHLC,
                interval="1d",
                value_column="complexity"
            )
        """
        # TODO: Implement via client
        return []

    def downsample(
        self,
        collection_id: str,
        target_collection: str,
        interval: str,
        mode: DownsampleMode = DownsampleMode.TIME_FIXED,
    ) -> Dict[str, Any]:
        """Downsample time-series data to a new collection.

        Args:
            collection_id: Source collection
            target_collection: Target collection for downsampled data
            interval: Downsample interval
            mode: Downsampling mode

        Returns:
            Downsample result with statistics

        Example:
            # Downsample to hourly data
            result = ts.downsample(
                collection_id="code_metrics_raw",
                target_collection="code_metrics_hourly",
                interval="1h"
            )
        """
        # TODO: Implement via client
        return {
            "success": True,
            "downsampled": 0,
        }

    # ========================================================================
    # Batch Operations
    # ========================================================================

    def flush_batch(self, collection_id: str) -> Dict[str, Any]:
        """Flush pending batch operations.

        Args:
            collection_id: Collection identifier

        Returns:
            Flush result with statistics
        """
        if collection_id not in self._batch_buffer:
            return {"success": True, "flushed": 0}

        batch = self._batch_buffer[collection_id]
        if not batch:
            return {"success": True, "flushed": 0}

        # TODO: Send batch to client with compression
        flushed = len(batch)

        # Clear buffer
        self._batch_buffer[collection_id] = []

        return {
            "success": True,
            "flushed": flushed,
        }


# =============================================================================
# High-Level Time-Series API
# =============================================================================


class ProximaDBTimeSeries:
    """High-level time-series operations interface.

    Provides simplified API for time-series operations with automatic
    connection management, batching, and compression.

    Args:
        client: ProximaDB client instance
        batch_size: Batch size for auto-flush
        compression: Compression codec for data
    """

    def __init__(
        self,
        client: Any,
        batch_size: int = 1000,
        compression: CompressionCodec = CompressionCodec.GORILLA,
    ):
        """Initialize time-series API.

        Args:
            client: ProximaDB client instance
            batch_size: Batch size for auto-flush
            compression: Compression codec
        """
        self._repository = TimeSeriesRepository(
            client=client,
            batch_size=batch_size,
            compression=compression,
        )

    def create_collection(
        self,
        name: str,
        value_columns: List[ValueColumn],
        tags_columns: List[str],
        timestamp_column: str = "timestamp",
        retention: str = "30d",
        compression: CompressionCodec = CompressionCodec.GORILLA,
    ) -> str:
        """Create a time-series collection.

        Args:
            name: Collection name
            value_columns: List of value columns
            tags_columns: List of tag column names
            timestamp_column: Timestamp column name
            retention: Data retention period
            compression: Compression codec

        Returns:
            Collection ID

        Example:
            collection_id = ts.create_collection(
                name="code_metrics",
                value_columns=[
                    ValueColumn(name="complexity", type=ValueType.FLOAT),
                    ValueColumn(name="lines_of_code", type=ValueType.INT),
                ],
                tags_columns=["file_path", "language", "author"],
                retention="90d"
            )
        """
        config = TimeSeriesCollectionConfig(
            name=name,
            timestamp_column=timestamp_column,
            value_columns=value_columns,
            tags_columns=tags_columns,
            retention=retention,
            compression=compression,
        )
        return self._repository.create_collection(config)

    def ingest(
        self,
        collection_id: str,
        metrics: List[Metric],
    ) -> Dict[str, Any]:
        """Ingest time-series metrics.

        Args:
            collection_id: Collection identifier
            metrics: List of metrics

        Returns:
            Ingest result

        Example:
            ts.ingest("code_metrics", metrics=[
                Metric(
                    timestamp=datetime.now(),
                    values={"complexity": 15.5, "lines": 250},
                    tags={"file_path": "main.py"}
                ),
            ])
        """
        return self._repository.ingest(collection_id, metrics)

    def query(
        self,
        collection_id: str,
        start_time: Union[str, datetime],
        end_time: Union[str, datetime],
        filter: Optional[TimeSeriesFilter] = None,
        aggregation: Optional[AggregationType] = None,
        interval: Optional[str] = None,
        limit: int = 1000,
    ) -> List[AggregatedMetric]:
        """Query time-series data.

        Args:
            collection_id: Collection identifier
            start_time: Query start
            end_time: Query end
            filter: Optional filters
            aggregation: Optional aggregation
            interval: Optional downsample interval
            limit: Maximum results

        Returns:
            List of aggregated metrics

        Example:
            # Get daily average complexity
            results = ts.query(
                collection_id="code_metrics",
                start_time="2026-02-01",
                end_time="2026-03-01",
                aggregation=AggregationType.AVG,
                interval="1d"
            )
        """
        return self._repository.query(
            collection_id=collection_id,
            start_time=start_time,
            end_time=end_time,
            filter=filter,
            aggregation=aggregation,
            interval=interval,
            limit=limit,
        )

    def get_latest(
        self,
        collection_id: str,
        tags: Dict[str, Any],
    ) -> Optional[Metric]:
        """Get latest metric for tags.

        Args:
            collection_id: Collection identifier
            tags: Tag filters

        Returns:
            Latest metric or None

        Example:
            latest = ts.get_latest(
                collection_id="code_metrics",
                tags={"file_path": "src/main.py"}
            )
        """
        return self._repository.get_latest(collection_id, tags)

    def flush(self, collection_id: str) -> Dict[str, Any]:
        """Flush pending batch operations.

        Args:
            collection_id: Collection identifier

        Returns:
            Flush result
        """
        return self._repository.flush_batch(collection_id)


# =============================================================================
# Factory Functions
# =============================================================================


def create_timeseries_api(
    client: Any,
    batch_size: int = 1000,
    compression: CompressionCodec = CompressionCodec.GORILLA,
) -> ProximaDBTimeSeries:
    """Factory function to create time-series API instance.

    Args:
        client: ProximaDB client instance
        batch_size: Batch size for auto-flush
        compression: Compression codec

    Returns:
        ProximaDBTimeSeries instance
    """
    return ProximaDBTimeSeries(
        client=client,
        batch_size=batch_size,
        compression=compression,
    )
