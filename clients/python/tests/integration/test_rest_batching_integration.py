"""
Integration tests for REST request batching

This test module validates the batching configuration and initialization.
Full end-to-end batching tests would require a live server and are better suited
for E2E test suites.
"""

from unittest.mock import patch

import pytest

from proximadb_sdk.batching_unified import (
    BatchConfig,
    BatchMetrics,
    BatchStrategy,
    ThreadedBatchProcessor,
)
from proximadb_sdk.config import ClientConfig
from proximadb_sdk.protocols.rest_sync import ProximaDBClient


class TestRestBatchingConfiguration:
    """Tests for REST batching configuration and initialization"""

    @pytest.fixture
    def config(self):
        """Client configuration"""
        return ClientConfig(url="http://localhost:5678", timeout=30.0)

    @pytest.fixture
    def batch_config(self):
        """Batch configuration for testing"""
        return BatchConfig(
            max_batch_size=10, max_wait_time_ms=100.0, strategy=BatchStrategy.HYBRID
        )

    def test_client_initialization_with_batching(self, config, batch_config):
        """Test client initialization with batching enabled"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):
            client = ProximaDBClient(
                config=config, enable_batching=True, batch_config=batch_config
            )

            # Verify batching is enabled
            assert client.enable_batching is True
            assert client._batch_processor is not None
            assert isinstance(client._batch_processor, ThreadedBatchProcessor)
            assert client._batch_processor.config == batch_config
            assert client._batch_processor.config.max_batch_size == 10
            assert client._batch_processor.config.strategy == BatchStrategy.HYBRID

            client.close()

    def test_client_initialization_without_batching(self, config):
        """Test client initialization without batching"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):
            client = ProximaDBClient(config=config)

            assert client.enable_batching is False
            assert client._batch_processor is None

            client.close()

    def test_batching_disabled_error(self, config):
        """Test error when trying to use batching when disabled"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):
            client = ProximaDBClient(config=config)  # Batching disabled

            try:
                with pytest.raises(RuntimeError, match="Batching is not enabled"):
                    client.insert_vectors_batched(
                        collection_id="test", vectors=[[1.0, 2.0]], ids=["vec_1"]
                    )

                with pytest.raises(RuntimeError, match="Batching is not enabled"):
                    client.get_batch_metrics()

            finally:
                client.close()

    def test_batch_metrics_structure(self, config, batch_config):
        """Test batch metrics structure and accessibility"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):

            client = ProximaDBClient(
                config=config, enable_batching=True, batch_config=batch_config
            )

            try:
                # Get metrics - should return BatchMetrics object
                metrics = client.get_batch_metrics()

                # Check if it's a BatchMetrics dataclass
                assert isinstance(metrics, BatchMetrics)
                assert hasattr(metrics, "total_batches")
                assert hasattr(metrics, "total_requests")
                assert hasattr(metrics, "avg_batch_size")
                assert hasattr(metrics, "total_latency_ms")
                assert hasattr(metrics, "avg_latency_ms")
                assert hasattr(metrics, "memory_usage_mb")

                # Initial metrics should be zero
                assert metrics.total_requests == 0
                assert metrics.total_batches == 0

            finally:
                client.close()

    def test_validation_errors(self, config, batch_config):
        """Test validation errors in batched operations"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):

            client = ProximaDBClient(
                config=config, enable_batching=True, batch_config=batch_config
            )

            try:
                # Mismatched vectors and IDs
                with pytest.raises(ValueError, match="Number of vectors must match"):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[1.0, 2.0], [3.0, 4.0]],
                        ids=["vec_1"],  # Only 1 ID for 2 vectors
                    )

                # Mismatched metadata
                with pytest.raises(
                    ValueError, match="Number of metadata items must match"
                ):
                    client.insert_vectors_batched(
                        collection_id="test",
                        vectors=[[1.0, 2.0]],
                        ids=["vec_1"],
                        metadata=[
                            {"tag": "1"},
                            {"tag": "2"},
                        ],  # 2 metadata for 1 vector
                    )

            finally:
                client.close()

    def test_context_manager_with_batching(self, config, batch_config):
        """Test client as context manager with batching"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):

            with ProximaDBClient(
                config=config, enable_batching=True, batch_config=batch_config
            ) as client:
                assert client.enable_batching is True
                assert client._batch_processor is not None

            # Should be closed after context manager exit
            assert client._batch_processor is None

    def test_different_batch_strategies(self, config):
        """Test different batching strategies"""
        strategies = [
            BatchStrategy.SIZE_BASED,
            BatchStrategy.TIME_BASED,
            BatchStrategy.ADAPTIVE,
            BatchStrategy.HYBRID,
        ]

        for strategy in strategies:
            batch_config = BatchConfig(max_batch_size=5, strategy=strategy)

            with patch(
                "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
            ):

                client = ProximaDBClient(
                    config=config, enable_batching=True, batch_config=batch_config
                )

                try:
                    # Verify strategy is set correctly in config
                    assert client._batch_processor.config.strategy == strategy

                finally:
                    client.close()

    def test_batch_size_configuration(self, config):
        """Test that batch size configuration works correctly"""
        test_sizes = [5, 10, 20, 50, 100]

        for batch_size in test_sizes:
            with patch(
                "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
            ):

                batch_config = BatchConfig(max_batch_size=batch_size)
                client = ProximaDBClient(
                    config=config, enable_batching=True, batch_config=batch_config
                )

                try:
                    assert client._batch_processor.config.max_batch_size == batch_size
                finally:
                    client.close()

    def test_wait_time_configuration(self, config):
        """Test that wait time configuration works correctly"""
        test_wait_times = [50.0, 100.0, 200.0, 500.0]

        for wait_time in test_wait_times:
            with patch(
                "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
            ):

                batch_config = BatchConfig(max_wait_time_ms=wait_time)
                client = ProximaDBClient(
                    config=config, enable_batching=True, batch_config=batch_config
                )

                try:
                    assert client._batch_processor.config.max_wait_time_ms == wait_time
                finally:
                    client.close()

    def test_batch_processor_lifecycle(self, config, batch_config):
        """Test batch processor starts and stops correctly"""
        with patch(
            "proximadb_sdk.protocols.rest_sync.ProximaDBClient._create_http_client"
        ):

            client = ProximaDBClient(
                config=config, enable_batching=True, batch_config=batch_config
            )

            # Processor should be created and started
            assert client._batch_processor is not None
            assert client._batch_processor._running is True

            # Close should stop the processor
            client.close()
            assert client._batch_processor is None


if __name__ == "__main__":
    # Run tests
    pytest.main([__file__, "-v"])
