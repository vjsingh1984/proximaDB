"""
Integration tests for ProximaDB Time-Series API.

Tests the multi-model time-series storage functionality including:
- Time-series collection creation with value columns
- High-throughput data ingestion
- Time-range queries with aggregation
- OHLC (Open-High-Low-Close) aggregation for financial data
- Downsampling and rollup operations
"""

import os
import sys
from datetime import datetime, timedelta

import pytest

# Add the src directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from proximadb_sdk.timeseries import (
    AggregationType,
    ProximaDBTimeSeries,
    TimeSeriesCollectionConfig,
    ValueColumn,
    ValueType,
)


@pytest.fixture
def client(embedded_rest_client):
    """Create a ProximaDB client for testing."""
    return embedded_rest_client


@pytest.fixture
def timeseries_api(client):
    """Create a Time-Series API instance for testing."""
    return ProximaDBTimeSeries(client)


@pytest.fixture
def test_collection_name():
    """Name of the test collection."""
    return "test_timeseries_python_sdk"


class TestTimeSeriesAPI:
    """Test suite for Time-Series API operations."""

    def test_create_timeseries_collection(self, timeseries_api, test_collection_name):
        """Test creating a time-series collection with value columns."""
        # Create collection configuration
        config = TimeSeriesCollectionConfig(
            name=test_collection_name,
            timestamp_column="timestamp",
            value_columns=[
                ValueColumn(
                    name="cpu_usage",
                    data_type=ValueType.FLOAT,
                    aggregation=AggregationType.AVG,
                ),
                ValueColumn(
                    name="memory_usage",
                    data_type=ValueType.FLOAT,
                    aggregation=AggregationType.MAX,
                ),
                ValueColumn(
                    name="request_count",
                    data_type=ValueType.INT,
                    aggregation=AggregationType.SUM,
                ),
            ],
            tag_columns=["host", "region", "service"],
            retention_ms=7 * 24 * 60 * 60 * 1000,  # 7 days
        )

        # Create collection
        result = timeseries_api.create_collection(config=config)

        # Verify result
        assert result is not None
        assert result.get("success") is True or result.get("collection_id") is not None

    def test_ingest_timeseries_data(self, timeseries_api, test_collection_name):
        """Test ingesting time-series data points."""
        # Create test data points
        now = datetime.utcnow()
        points = []
        for i in range(100):
            timestamp = now + timedelta(seconds=i)
            points.append(
                {
                    "timestamp": timestamp.isoformat() + "Z",
                    "values": {
                        "cpu_usage": 50.0 + i % 50,
                        "memory_usage": 40.0 + i % 30,
                        "request_count": 100 + i * 10,
                    },
                    "tags": {
                        "host": f"server-{i % 5}",
                        "region": "us-west" if i % 2 == 0 else "us-east",
                        "service": "api",
                    },
                }
            )

        # Ingest data
        result = timeseries_api.ingest(
            collection_id=test_collection_name, points=points
        )

        # Verify result
        assert result is not None
        assert result.get("ingested_count", 0) > 0
        assert result.get("failed_count", 1) == 0

    def test_query_timeseries_raw(self, timeseries_api, test_collection_name):
        """Test querying raw time-series data."""
        now = datetime.utcnow()
        start_time = (now - timedelta(hours=1)).isoformat() + "Z"
        end_time = now.isoformat() + "Z"

        # Query raw data
        results = timeseries_api.query(
            collection_id=test_collection_name,
            start_time=start_time,
            end_time=end_time,
        )

        # Verify results
        assert results is not None
        points = results.get("raw_points", results.get("metrics", []))
        assert len(points) > 0

    def test_query_timeseries_with_aggregation(
        self, timeseries_api, test_collection_name
    ):
        """Test querying time-series data with aggregation."""
        now = datetime.utcnow()
        start_time = (now - timedelta(hours=1)).isoformat() + "Z"
        end_time = now.isoformat() + "Z"

        # Query with AVG aggregation and 1-minute buckets
        results = timeseries_api.query(
            collection_id=test_collection_name,
            start_time=start_time,
            end_time=end_time,
            aggregation="AVG",
            bucket_ms=60000,  # 1 minute
        )

        # Verify results
        assert results is not None
        metrics = results.get("metrics", [])
        assert len(metrics) > 0

        # Check aggregated values
        for metric in metrics[:3]:  # Check first few results
            assert metric.get("timestamp") is not None
            assert metric.get("value") is not None
            assert metric.get("count", 0) > 0

    def test_query_timeseries_with_tag_filters(
        self, timeseries_api, test_collection_name
    ):
        """Test querying time-series data with tag filters."""
        now = datetime.utcnow()
        start_time = (now - timedelta(hours=1)).isoformat() + "Z"
        end_time = now.isoformat() + "Z"

        # Query with tag filter
        results = timeseries_api.query(
            collection_id=test_collection_name,
            start_time=start_time,
            end_time=end_time,
            aggregation="AVG",
            tag_filters={"region": "us-west"},
        )

        # Verify results
        assert results is not None

        # If we have results, verify they match the filter
        metrics = results.get("metrics", results.get("raw_points", []))
        for metric in metrics[:5]:  # Check first few results
            # Note: Tags may or may not be included in results depending on the server implementation
            if isinstance(metric, dict) and "tags" in metric:
                assert metric["tags"].get("region") == "us-west"

    def test_query_timeseries_ohlc_aggregation(
        self, timeseries_api, test_collection_name
    ):
        """Test OHLC (Open-High-Low-Close) aggregation for financial data."""
        # Create a financial data collection
        financial_collection = "test_financial_ts"

        # Create collection with OHLC aggregation
        config = TimeSeriesCollectionConfig(
            name=financial_collection,
            timestamp_column="timestamp",
            value_columns=[
                ValueColumn(
                    name="price",
                    data_type=ValueType.FLOAT,
                    aggregation=AggregationType.OHLC,
                ),
                ValueColumn(
                    name="volume",
                    data_type=ValueType.INT,
                    aggregation=AggregationType.SUM,
                ),
            ],
            tag_columns=["symbol"],
        )

        timeseries_api.create_collection(config=config)

        # Ingest financial data
        now = datetime.utcnow()
        points = []
        base_price = 100.0
        for i in range(50):
            timestamp = now + timedelta(minutes=i)
            price = base_price + (i % 10) - 5 + (i % 3) * 0.1
            points.append(
                {
                    "timestamp": timestamp.isoformat() + "Z",
                    "values": {
                        "price": price,
                        "volume": 1000 + i * 100,
                    },
                    "tags": {"symbol": "AAPL"},
                }
            )

        timeseries_api.ingest(collection_id=financial_collection, points=points)

        # Query with OHLC aggregation
        start_time = now.isoformat() + "Z"
        end_time = (now + timedelta(hours=1)).isoformat() + "Z"

        results = timeseries_api.query(
            collection_id=financial_collection,
            start_time=start_time,
            end_time=end_time,
            aggregation="OHLC",
            bucket_ms=300000,  # 5 minutes
        )

        # Verify OHLC results
        assert results is not None
        metrics = results.get("metrics", [])
        if len(metrics) > 0:
            # Check OHLC fields
            ohlc_metric = metrics[0]
            assert "open" in ohlc_metric or "value" in ohlc_metric
            if "open" in ohlc_metric:
                assert "high" in ohlc_metric
                assert "low" in ohlc_metric
                assert "close" in ohlc_metric

        # Cleanup
        timeseries_api.delete_collection(collection_id=financial_collection)

    def test_list_timeseries_collections(self, timeseries_api):
        """Test listing time-series collections."""
        # List collections
        collections = timeseries_api.list_collections()

        # Verify results
        assert collections is not None
        assert isinstance(collections, list)

        # Find our test collection
        test_collection = None
        for coll in collections:
            if isinstance(coll, dict):
                if coll.get("name") == "test_timeseries_python_sdk":
                    test_collection = coll
                    break

        # Verify test collection exists
        assert test_collection is not None

    def test_timeseries_aggregate(self, timeseries_api, test_collection_name):
        """Test aggregation pipeline operations."""
        now = datetime.utcnow()
        start_time = (now - timedelta(hours=1)).isoformat() + "Z"
        end_time = now.isoformat() + "Z"

        # Create aggregation pipeline
        pipeline = [
            {
                "stage": "downsample",
                "bucket_ms": 60000,  # 1 minute
                "aggregation": "AVG",
                "value_columns": ["cpu_usage", "memory_usage"],
            },
            {
                "stage": "group_by",
                "tag_columns": ["region"],
                "aggregation": "AVG",
                "bucket_ms": 300000,  # 5 minutes
            },
        ]

        # Execute aggregation
        results = timeseries_api.aggregate(
            collection_id=test_collection_name,
            start_time=start_time,
            end_time=end_time,
            pipeline=pipeline,
        )

        # Verify results
        assert results is not None
        assert "results" in results or "metrics" in results

    def test_delete_timeseries_collection(self, timeseries_api, test_collection_name):
        """Test deleting a time-series collection."""
        # Delete collection
        result = timeseries_api.delete_collection(collection_id=test_collection_name)

        # Verify result
        assert result is True or result.get("success") is True

        # Verify collection is gone
        collections = timeseries_api.list_collections()
        test_collection_exists = False
        for coll in collections:
            if isinstance(coll, dict) and coll.get("name") == test_collection_name:
                test_collection_exists = True
                break

        assert not test_collection_exists

    def test_high_throughput_ingestion(self, timeseries_api):
        """Test high-throughput ingestion (100K+ points/sec capability)."""
        import time

        # Create a test collection for throughput testing
        collection = "test_throughput_ts"
        config = TimeSeriesCollectionConfig(
            name=collection,
            timestamp_column="timestamp",
            value_columns=[
                ValueColumn(
                    name="value",
                    data_type=ValueType.FLOAT,
                    aggregation=AggregationType.AVG,
                )
            ],
            tag_columns=["source"],
        )
        timeseries_api.create_collection(config=config)

        # Generate large batch of points
        now = datetime.utcnow()
        batch_size = 10000
        points = []
        for i in range(batch_size):
            timestamp = now + timedelta(microseconds=i * 100)  # 0.1ms intervals
            points.append(
                {
                    "timestamp": timestamp.isoformat() + "Z",
                    "values": {"value": float(i)},
                    "tags": {"source": f"src-{i % 10}"},
                }
            )

        # Measure ingestion time
        start_time = time.time()
        result = timeseries_api.ingest(collection_id=collection, points=points)
        end_time = time.time()

        elapsed = end_time - start_time
        throughput = batch_size / elapsed if elapsed > 0 else float("inf")

        # Verify
        assert result.get("ingested_count", 0) > 0

        # Print throughput (for debugging)
        print(f"\nIngestion throughput: {throughput:.0f} points/sec")

        # Cleanup
        timeseries_api.delete_collection(collection_id=collection)


class TestTimeSeriesAdapterMethods:
    """Test suite for Time-Series adapter methods."""

    @pytest.fixture
    def setup_collection(self, client):
        """Set up a test collection."""
        test_collection = "test_adapter_timeseries"

        # Create collection via adapter
        try:
            client.create_timeseries_collection(
                name=test_collection,
                config={
                    "timestamp_column": "timestamp",
                    "value_columns": [
                        {"name": "metric", "data_type": "float", "aggregation": "avg"}
                    ],
                    "tag_columns": ["host"],
                },
            )
        except Exception:
            pass  # Collection might already exist

        yield test_collection

        # Cleanup
        try:
            client.delete_timeseries_collection(test_collection)
        except Exception:
            pass

    def test_adapter_create_collection(self, client):
        """Test creating a time-series collection via adapter."""
        result = client.create_timeseries_collection(
            name="test_adapter_ts",
            config={
                "timestamp_column": "timestamp",
                "value_columns": [
                    {"name": "value", "data_type": "float", "aggregation": "avg"}
                ],
            },
        )

        assert result is not None
        assert result.get("success") is True or result.get("collection_id") is not None

        # Cleanup
        client.delete_timeseries_collection("test_adapter_ts")

    def test_adapter_ingest_and_query(self, client, setup_collection):
        """Test ingesting and querying via adapter."""
        from datetime import datetime, timedelta

        now = datetime.utcnow()
        points = []
        for i in range(10):
            timestamp = now + timedelta(seconds=i)
            points.append(
                {
                    "timestamp": timestamp.isoformat() + "Z",
                    "values": {"metric": 10.0 + i},
                    "tags": {"host": "server1"},
                }
            )

        # Ingest
        result = client.ingest_timeseries(
            collection_name=setup_collection, points=points
        )

        assert result is not None
        assert result.get("ingested_count", 0) > 0

        # Query
        start_time = now.isoformat() + "Z"
        end_time = (now + timedelta(minutes=1)).isoformat() + "Z"

        results = client.query_timeseries(
            collection_name=setup_collection, start_time=start_time, end_time=end_time
        )

        assert results is not None
        assert len(results.get("raw_points", results.get("metrics", []))) > 0

    def test_adapter_query_with_aggregation(self, client, setup_collection):
        """Test querying with aggregation via adapter."""
        from datetime import datetime, timedelta

        now = datetime.utcnow()
        start_time = (now - timedelta(minutes=5)).isoformat() + "Z"
        end_time = now.isoformat() + "Z"

        results = client.query_timeseries(
            collection_name=setup_collection,
            start_time=start_time,
            end_time=end_time,
            aggregation="AVG",
            bucket_ms=60000,  # 1 minute
        )

        assert results is not None

    def test_adapter_list_collections(self, client):
        """Test listing time-series collections via adapter."""
        # Create a test collection first
        client.create_timeseries_collection(
            name="test_list_ts",
            config={
                "timestamp_column": "timestamp",
                "value_columns": [
                    {"name": "value", "data_type": "float", "aggregation": "avg"}
                ],
            },
        )

        # List collections
        collections = client.list_timeseries_collections()

        assert collections is not None
        assert isinstance(collections, list)

        # Cleanup
        client.delete_timeseries_collection("test_list_ts")

    def test_adapter_delete_collection(self, client):
        """Test deleting a time-series collection via adapter."""
        # Create a test collection
        client.create_timeseries_collection(
            name="test_delete_ts",
            config={
                "timestamp_column": "timestamp",
                "value_columns": [
                    {"name": "value", "data_type": "float", "aggregation": "avg"}
                ],
            },
        )

        # Delete
        result = client.delete_timeseries_collection("test_delete_ts")

        assert result is True


if __name__ == "__main__":
    # Run tests
    pytest.main([__file__, "-v", "--tb=short"])
