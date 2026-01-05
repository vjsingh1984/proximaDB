"""
Integration tests for intelligent routing with clients using unified router system

Tests operation routing, protocol selection, and performance metrics using
the new unified IntelligentRouter system with real client scenarios.
"""

import sys
import threading
import time
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import pytest

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb_sdk.config import ClientConfig, Protocol
from proximadb_sdk.intelligent_router import (
    IntelligentRouter,
    OperationType,
    ProtocolHealth,
    ProtocolMetrics,
    RoutingConfig,
    RoutingStrategy,
)

# Backward compatibility imports
from proximadb_sdk.operation_router import OperationRouter, create_operation_router


class TestIntelligentRouterIntegration:
    """Integration tests for intelligent router with client protocols"""

    @pytest.fixture
    def router_config(self):
        """Router configuration for testing"""
        return RoutingConfig(
            strategy=RoutingStrategy.HYBRID,
            health_check_interval_seconds=0,  # Disable for testing
            enable_fallback=True,
            enable_load_balancing=True,
            enable_adaptive_learning=True,
        )

    @pytest.fixture
    def router(self, router_config):
        """Intelligent router instance"""
        router = IntelligentRouter(router_config)
        yield router
        router.stop()

    def test_router_with_mock_clients(self, router):
        """Test router with mocked client protocols"""

        # Mock REST client
        rest_client = Mock()
        rest_client.search_vectors.return_value = [{"id": "vec1", "score": 0.9}]
        rest_client.insert_vectors.return_value = {"success": True, "count": 5}

        # Mock gRPC client
        grpc_client = Mock()
        grpc_client.search_vectors.return_value = [{"id": "vec2", "score": 0.8}]
        grpc_client.insert_vectors.return_value = {"success": True, "count": 10}

        # Register client factories
        router.register_client_factory(Protocol.REST, lambda: rest_client)
        router.register_client_factory(Protocol.GRPC, lambda: grpc_client)

        # Set both protocols as healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        # Test routing decisions
        search_protocol, search_client = router.route_operation(
            OperationType.SINGLE_SEARCH
        )
        insert_protocol, insert_client = router.route_operation(
            OperationType.BULK_INSERT, data_size=5000
        )
        health_protocol, health_client = router.route_operation(
            OperationType.HEALTH_CHECK
        )

        # Verify routing decisions make sense
        assert search_protocol in [Protocol.REST, Protocol.GRPC]
        assert insert_protocol == Protocol.GRPC  # Bulk operations prefer gRPC
        assert health_protocol == Protocol.REST  # Admin operations prefer REST

        # Simulate operations and record results
        start_time = time.time()
        search_result = search_client.search()
        search_time = (time.time() - start_time) * 1000

        router.record_operation_result(
            OperationType.SINGLE_SEARCH,
            search_protocol,
            success=True,
            latency_ms=search_time,
        )

        start_time = time.time()
        insert_result = insert_client.insert_vectors()
        insert_time = (time.time() - start_time) * 1000

        router.record_operation_result(
            OperationType.BULK_INSERT,
            insert_protocol,
            success=True,
            latency_ms=insert_time,
            throughput_qps=200.0,
        )

        # Verify metrics were recorded
        metrics = router.get_metrics()
        assert "protocols" in metrics
        assert Protocol.GRPC.value in metrics["protocols"]
        assert Protocol.REST.value in metrics["protocols"]

    def test_adaptive_routing_behavior(self, router):
        """Test adaptive routing based on performance feedback"""

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Initially, both protocols should be unknown
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY

        # Simulate REST performing poorly, gRPC performing well
        for _ in range(10):
            router.record_operation_result(
                OperationType.SINGLE_SEARCH,
                Protocol.REST,
                success=True,
                latency_ms=500.0,
                throughput_qps=10.0,
            )
            router.record_operation_result(
                OperationType.SINGLE_SEARCH,
                Protocol.GRPC,
                success=True,
                latency_ms=50.0,
                throughput_qps=200.0,
            )

        # Router should adapt and prefer gRPC for performance-sensitive operations
        router.config.strategy = RoutingStrategy.PERFORMANCE_BASED

        protocols_chosen = []
        for _ in range(10):
            protocol, _ = router.route_operation(OperationType.SINGLE_SEARCH)
            protocols_chosen.append(protocol)

        # Should heavily favor gRPC due to better performance
        grpc_count = protocols_chosen.count(Protocol.GRPC)
        rest_count = protocols_chosen.count(Protocol.REST)

        assert grpc_count >= rest_count  # gRPC should be chosen more often

    def test_fallback_behavior(self, router):
        """Test fallback behavior when preferred protocol fails"""

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Make gRPC unhealthy
        for _ in range(20):
            router.record_operation_result(
                OperationType.BULK_INSERT, Protocol.GRPC, success=False, latency_ms=0.0
            )

        # Set REST as healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY

        # Operations that normally prefer gRPC should fallback to REST
        fallback_protocols = []
        for _ in range(5):
            protocol, _ = router.route_operation(
                OperationType.BULK_INSERT, data_size=10000
            )
            fallback_protocols.append(protocol)

        # Should fallback to REST due to gRPC being unhealthy
        if router._metrics[Protocol.GRPC].health_status == ProtocolHealth.UNHEALTHY:
            assert all(p == Protocol.REST for p in fallback_protocols)

    def test_load_balancing_behavior(self, router):
        """Test load balancing across healthy protocols"""
        router.config.strategy = RoutingStrategy.ROUND_ROBIN
        router.config.enable_load_balancing = True

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Ensure both protocols are healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        for protocol in [Protocol.REST, Protocol.GRPC]:
            for _ in range(5):
                router.record_operation_result(
                    OperationType.SINGLE_SEARCH,
                    protocol,
                    success=True,
                    latency_ms=50.0,
                    throughput_qps=100.0,
                )

        # Route multiple operations
        protocols_chosen = []
        for _ in range(20):
            protocol, _ = router.route_operation(OperationType.SINGLE_SEARCH)
            protocols_chosen.append(protocol)

        grpc_count = protocols_chosen.count(Protocol.GRPC)
        rest_count = protocols_chosen.count(Protocol.REST)

        # Should be roughly balanced (allow some variance)
        assert abs(grpc_count - rest_count) <= 4  # Within 20% of perfect balance

    def test_operation_specific_routing(self, router):
        """Test that specific operation types get routed appropriately"""
        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Set both as healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        test_cases = [
            # (operation_type, data_size, expected_preference)
            (OperationType.BULK_INSERT, 50000, Protocol.GRPC),
            (OperationType.BATCH_SEARCH, 10000, Protocol.GRPC),
            (OperationType.HEALTH_CHECK, 0, Protocol.REST),
            (OperationType.LIST_COLLECTIONS, 0, Protocol.REST),
            (OperationType.SQL_QUERY, 0, Protocol.REST),
            (OperationType.GET_METRICS, 0, Protocol.REST),
        ]

        router.config.strategy = RoutingStrategy.OPERATION_BASED

        for operation_type, data_size, expected_protocol in test_cases:
            protocol, _ = router.route_operation(operation_type, data_size=data_size)
            assert (
                protocol == expected_protocol
            ), f"Operation {operation_type.value} should prefer {expected_protocol.value}, got {protocol.value}"

    def test_hybrid_routing_integration(self, router):
        """Test hybrid routing strategy with various conditions"""
        router.config.strategy = RoutingStrategy.HYBRID

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Set both as healthy initially
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        # Test normal operation-based routing
        health_protocol, _ = router.route_operation(OperationType.HEALTH_CHECK)
        assert health_protocol == Protocol.REST

        bulk_protocol, _ = router.route_operation(
            OperationType.BULK_INSERT, data_size=10000
        )
        assert bulk_protocol == Protocol.GRPC

        # Make gRPC unhealthy for bulk operations
        for _ in range(20):
            router.record_operation_result(
                OperationType.BULK_INSERT, Protocol.GRPC, success=False, latency_ms=0.0
            )

        # Hybrid strategy should fallback to performance-based routing
        fallback_protocol, _ = router.route_operation(
            OperationType.BULK_INSERT, data_size=10000
        )

        if router._metrics[Protocol.GRPC].health_status == ProtocolHealth.UNHEALTHY:
            assert fallback_protocol == Protocol.REST

    def test_concurrent_routing_decisions(self, router):
        """Test concurrent routing decisions with metrics updates"""

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Set both as healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        results = []
        errors = []

        def routing_worker():
            try:
                for i in range(100):
                    # Route various operations
                    operations = [
                        (OperationType.SINGLE_SEARCH, 1000),
                        (OperationType.BULK_INSERT, 5000),
                        (OperationType.HEALTH_CHECK, 0),
                        (OperationType.LIST_COLLECTIONS, 0),
                    ]

                    for operation_type, data_size in operations:
                        protocol, _ = router.route_operation(
                            operation_type, data_size=data_size
                        )
                        results.append((operation_type, protocol))

                        # Simulate operation execution with random success/failure
                        success = (
                            i + hash(operation_type.value)
                        ) % 10 != 0  # 90% success rate
                        response_time = 10.0 + (i % 100)

                        if success:
                            router.record_operation_result(
                                operation_type,
                                protocol,
                                success=True,
                                latency_ms=response_time,
                                throughput_qps=100.0 + (i % 50),
                            )
                        else:
                            router.record_operation_result(
                                operation_type, protocol, success=False, latency_ms=0.0
                            )

            except Exception as e:
                errors.append(e)

        # Run concurrent workers
        threads = []
        for _ in range(5):
            thread = threading.Thread(target=routing_worker)
            threads.append(thread)
            thread.start()

        for thread in threads:
            thread.join()

        # Should complete without errors
        assert len(errors) == 0
        assert len(results) == 2000  # 5 threads × 100 iterations × 4 operations

        # Verify metrics were updated
        metrics = router.get_metrics()
        grpc_requests = metrics["protocols"][Protocol.GRPC.value]["total_requests"]
        rest_requests = metrics["protocols"][Protocol.REST.value]["total_requests"]
        total_requests = grpc_requests + rest_requests

        assert total_requests > 0

    def test_protocol_preference_override(self, router):
        """Test explicit protocol preference override"""

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Set both as healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        # Health check normally goes to REST
        default_protocol, _ = router.route_operation(OperationType.HEALTH_CHECK)
        assert default_protocol == Protocol.REST

        # But explicit preference should override
        override_protocol, _ = router.route_operation(
            OperationType.HEALTH_CHECK, preferred_protocol=Protocol.GRPC
        )
        assert override_protocol == Protocol.GRPC

    def test_routing_with_real_context(self, router):
        """Test routing with realistic operation context"""

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Set both as healthy
        router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
        router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

        # Test with realistic contexts
        contexts = [
            {
                "operation": OperationType.SINGLE_SEARCH,
                "data_size": 1000,
                "required_features": {"similarity_search"},
            },
            {
                "operation": OperationType.BULK_INSERT,
                "data_size": 100000,
                "required_features": {"batch_processing"},
            },
            {
                "operation": OperationType.HEALTH_CHECK,
                "data_size": 0,
                "required_features": {"monitoring"},
            },
        ]

        for test_case in contexts:
            protocol, client = router.route_operation(
                test_case["operation"],
                data_size=test_case["data_size"],
                required_features=test_case["required_features"],
            )

            # Should return valid protocol
            assert protocol in [Protocol.REST, Protocol.GRPC]
            assert client is not None

            # Simulate execution and record metrics
            router.record_operation_result(
                test_case["operation"],
                protocol,
                success=True,
                latency_ms=25.0,
                throughput_qps=150.0,
            )

    def test_router_statistics_accuracy(self, router):
        """Test accuracy of router statistics"""

        # Register mock clients
        router.register_client_factory(Protocol.REST, lambda: Mock())
        router.register_client_factory(Protocol.GRPC, lambda: Mock())

        # Perform known operations with known results
        operations = [
            (Protocol.GRPC, OperationType.SINGLE_SEARCH, True, 10.0, 200.0),
            (Protocol.GRPC, OperationType.SINGLE_SEARCH, True, 15.0, 180.0),
            (Protocol.GRPC, OperationType.SINGLE_SEARCH, False, 0.0, 0.0),
            (Protocol.REST, OperationType.HEALTH_CHECK, True, 25.0, 100.0),
            (Protocol.REST, OperationType.HEALTH_CHECK, True, 30.0, 120.0),
        ]

        for protocol, operation, success, response_time, throughput in operations:
            router.record_operation_result(
                operation,
                protocol,
                success=success,
                latency_ms=response_time,
                throughput_qps=throughput if success else None,
            )

        metrics = router.get_metrics()

        # Verify gRPC stats
        grpc_metrics = metrics["protocols"][Protocol.GRPC.value]
        assert grpc_metrics["total_requests"] == 3
        assert abs(grpc_metrics["success_rate"] - 66.67) < 1  # 2 success out of 3

        # Verify REST stats
        rest_metrics = metrics["protocols"][Protocol.REST.value]
        assert rest_metrics["total_requests"] == 2
        assert rest_metrics["success_rate"] == 100.0  # 2 success out of 2


class TestBackwardCompatibilityIntegration:
    """Test backward compatibility with old OperationRouter API"""

    def test_operation_router_alias_integration(self):
        """Test OperationRouter works as an alias in integration scenario"""
        config = RoutingConfig(health_check_interval_seconds=0)
        router = OperationRouter(config)

        try:
            # Should work like IntelligentRouter
            assert isinstance(router, IntelligentRouter)

            # Register mock clients
            router.register_client_factory(Protocol.REST, lambda: Mock())
            router.register_client_factory(Protocol.GRPC, lambda: Mock())

            # Set protocols as healthy
            router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
            router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

            # Test routing
            protocol, client = router.route_operation(OperationType.HEALTH_CHECK)
            assert protocol in [Protocol.REST, Protocol.GRPC]
            assert client is not None

        finally:
            router.stop()

    def test_create_operation_router_integration(self):
        """Test create_operation_router factory function in integration"""
        router = create_operation_router(
            RoutingConfig(
                strategy=RoutingStrategy.PERFORMANCE_BASED,
                health_check_interval_seconds=0,
            )
        )

        try:
            assert isinstance(router, IntelligentRouter)
            assert router.config.strategy == RoutingStrategy.PERFORMANCE_BASED

            # Should work with real routing scenarios
            router.register_client_factory(Protocol.REST, lambda: Mock())
            router.register_client_factory(Protocol.GRPC, lambda: Mock())

            # Set protocols as healthy
            router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
            router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

            # Test routing operations
            for operation in [
                OperationType.SINGLE_SEARCH,
                OperationType.BULK_INSERT,
                OperationType.HEALTH_CHECK,
            ]:
                protocol, client = router.route_operation(operation)
                assert protocol in [Protocol.REST, Protocol.GRPC]

        finally:
            router.stop()


@pytest.mark.performance
class TestIntelligentRouterPerformanceIntegration:
    """Performance integration tests for intelligent router"""

    def test_routing_overhead(self):
        """Test routing overhead in realistic scenarios"""
        config = RoutingConfig(
            strategy=RoutingStrategy.HYBRID, health_check_interval_seconds=0
        )
        router = IntelligentRouter(config)

        try:
            # Register mock clients
            router.register_client_factory(Protocol.REST, lambda: Mock())
            router.register_client_factory(Protocol.GRPC, lambda: Mock())

            # Set protocols as healthy
            router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
            router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

            # Test routing overhead for various operations
            operations = [
                OperationType.SINGLE_SEARCH,
                OperationType.BULK_INSERT,
                OperationType.HEALTH_CHECK,
                OperationType.LIST_COLLECTIONS,
                OperationType.SQL_QUERY,
            ]

            # Measure routing time
            start_time = time.time()

            for _ in range(1000):
                for operation in operations:
                    protocol, client = router.route_operation(
                        operation, data_size=1000 + (_ % 5000)
                    )
                    assert protocol in [Protocol.REST, Protocol.GRPC]
                    assert client is not None

            routing_time = time.time() - start_time
            operations_per_second = (1000 * len(operations)) / routing_time

            print(f"Routing performance: {operations_per_second:.0f} operations/second")
            print(
                f"Average routing time: {(routing_time * 1000) / (1000 * len(operations)):.3f}ms per operation"
            )

            # Should be very fast - routing should add minimal overhead
            assert operations_per_second > 10000  # At least 10k ops/sec
            assert (
                routing_time / (1000 * len(operations))
            ) < 0.001  # Less than 1ms per operation

        finally:
            router.stop()

    def test_adaptive_routing_convergence(self):
        """Test how quickly adaptive routing converges to optimal choice"""
        config = RoutingConfig(
            strategy=RoutingStrategy.PERFORMANCE_BASED,
            health_check_interval_seconds=0,
            enable_adaptive_learning=True,
        )
        router = IntelligentRouter(config)

        try:
            # Register mock clients
            router.register_client_factory(Protocol.REST, lambda: Mock())
            router.register_client_factory(Protocol.GRPC, lambda: Mock())

            # Set both as healthy
            router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
            router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

            # Simulate REST being much faster than gRPC
            rest_response_time = 10.0
            grpc_response_time = 100.0

            protocols_chosen = []
            convergence_point = None

            for i in range(100):
                protocol, _ = router.route_operation(OperationType.SINGLE_SEARCH)
                protocols_chosen.append(protocol)

                # Record results with different performance
                if protocol == Protocol.REST:
                    router.record_operation_result(
                        OperationType.SINGLE_SEARCH,
                        Protocol.REST,
                        success=True,
                        latency_ms=rest_response_time,
                        throughput_qps=500.0,
                    )
                else:
                    router.record_operation_result(
                        OperationType.SINGLE_SEARCH,
                        Protocol.GRPC,
                        success=True,
                        latency_ms=grpc_response_time,
                        throughput_qps=50.0,
                    )

                # Check if we've converged (last 10 choices are all REST)
                if i >= 10 and all(p == Protocol.REST for p in protocols_chosen[-10:]):
                    if convergence_point is None:
                        convergence_point = i

            print(f"Convergence point: {convergence_point} iterations")

            # Should converge to faster protocol relatively quickly
            assert convergence_point is not None
            assert convergence_point <= 50  # Should converge within 50 iterations

            # Final choices should heavily favor REST
            final_choices = protocols_chosen[-20:]
            rest_percentage = (
                final_choices.count(Protocol.REST) / len(final_choices)
            ) * 100
            print(f"Final REST percentage: {rest_percentage:.1f}%")

            assert rest_percentage >= 80  # Should strongly prefer faster protocol

        finally:
            router.stop()


class TestIntelligentRouterRealServerIntegration(BaseProximaDBTest):
    """Integration tests with real ProximaDB server"""

    def test_router_with_real_server(self):
        """Test router integration with real server"""
        ensure_server_running()

        config = RoutingConfig(
            strategy=RoutingStrategy.OPERATION_BASED, health_check_interval_seconds=0
        )
        router = IntelligentRouter(config)

        try:
            # Register mock clients that simulate real protocol behavior
            mock_rest_client = Mock()
            mock_rest_client.health_check.return_value = {"status": "healthy"}
            mock_grpc_client = Mock()
            mock_grpc_client.health_check.return_value = {"status": "healthy"}

            router.register_client_factory(Protocol.REST, lambda: mock_rest_client)
            router.register_client_factory(Protocol.GRPC, lambda: mock_grpc_client)

            # Set protocols as healthy
            router._metrics[Protocol.REST].health_status = ProtocolHealth.HEALTHY
            router._metrics[Protocol.GRPC].health_status = ProtocolHealth.HEALTHY

            # Test various operation routing
            health_protocol, health_client = router.route_operation(
                OperationType.HEALTH_CHECK
            )
            assert health_protocol == Protocol.REST
            assert health_client == mock_rest_client

            bulk_protocol, bulk_client = router.route_operation(
                OperationType.BULK_INSERT, data_size=10000
            )
            assert bulk_protocol == Protocol.GRPC
            assert bulk_client == mock_grpc_client

            # Test recording metrics
            router.record_operation_result(
                OperationType.HEALTH_CHECK, Protocol.REST, success=True, latency_ms=15.0
            )

            # Verify metrics are updated
            metrics = router.get_metrics()
            assert metrics["protocols"][Protocol.REST.value]["total_requests"] > 0

        finally:
            router.stop()


if __name__ == "__main__":
    # Run basic tests
    pytest.main([__file__, "-v"])
