#!/usr/bin/env python3
"""
Unit tests for ProximaDB SDK imports and exports

This ensures all public APIs are properly exported and accessible.
"""

import importlib

import pytest


class TestSDKImports:
    """Test ProximaDB SDK imports and module structure"""

    def test_compression_config_import(self):
        """Test that CompressionConfig can be imported from main module"""
        from proximadb_sdk import CompressionConfig

        # Test instantiation with actual fields
        config = CompressionConfig(enabled=True, algorithm="gzip")
        assert config.enabled is True
        assert config.algorithm == "gzip"

    def test_compression_config_in_demo_usage(self):
        """Test the specific import pattern used in demos"""
        # This is the exact pattern that was failing
        from proximadb_sdk import ClientConfig, CompressionConfig

        # Test ClientConfig
        client_config = ClientConfig(url="http://localhost:5678")
        assert client_config.url == "http://localhost:5678"

        # Test CompressionConfig with actual fields
        compression_config = CompressionConfig(enabled=False, algorithm="deflate")
        assert compression_config.enabled is False

    def test_all_config_exports(self):
        """Test all exports from config module"""
        from proximadb_sdk import config

        expected_exports = [
            "ClientConfig",
            "CompressionConfig",
            "Protocol",
            "LogLevel",
            "RetryConfig",
        ]

        for export in expected_exports:
            assert hasattr(config, export), f"Missing config export: {export}"

    def test_demo_setup_imports(self):
        """Test the exact import pattern from demo_setup.py"""
        try:
            from proximadb_sdk import (
                ClientConfig,
                CollectionConfig,
                CompressionConfig,
                DistanceMetric,
                Protocol,
                ProximaDBClient,
                QuantizationConfig,
                QuantizationType,
                SearchOptimization,
                StorageEngine,
                VectorRecord,
            )

            # All imports should succeed
            assert ProximaDBClient is not None
            assert Protocol is not None
            assert ClientConfig is not None
            assert CompressionConfig is not None
            assert CollectionConfig is not None

        except ImportError as e:
            pytest.fail(f"Demo import pattern failed: {e}")

    def test_module_reload_stability(self):
        """Test that module can be reloaded without issues"""
        import proximadb_sdk

        # Get initial references
        initial_client = proximadb_sdk.ProximaDBClient
        initial_compression = proximadb_sdk.CompressionConfig

        # Reload module
        importlib.reload(proximadb_sdk)

        # Check references are still valid
        assert proximadb_sdk.ProximaDBClient is not None
        assert proximadb_sdk.CompressionConfig is not None

    def test_import_error_messages(self):
        """Test helpful error messages for common import mistakes"""
        # Test importing non-existent item.
        # `noqa: F401` keeps ruff from re-stripping the intentional
        # never-resolves-to-anything import that's the whole point of
        # this test. The previous ruff --fix pass replaced the line
        # with `pass`, making the `pytest.raises(ImportError)` block
        # silently never raise.
        with pytest.raises(ImportError) as exc_info:
            from proximadb_sdk import NonExistentClass  # noqa: F401

        # The error should mention the module
        assert "proximadb" in str(exc_info.value)

    def test_submodule_imports(self):
        """Test that submodules are importable"""
        submodules = [
            "proximadb_sdk.config",
            "proximadb_sdk.models",
            "proximadb_sdk.exceptions",
            "proximadb_sdk.chunking",
            "proximadb_sdk.filters",
            "proximadb_sdk.unified_client",
        ]

        for module_name in submodules:
            try:
                module = importlib.import_module(module_name)
                assert module is not None
            except ImportError as e:
                # Some modules might be optional (like gRPC)
                if "grpc" not in module_name:
                    pytest.fail(f"Failed to import {module_name}: {e}")

    def test_public_api_completeness(self):
        """Test that all documented public APIs are available"""
        import proximadb_sdk

        # Core client APIs
        client_apis = [
            "ProximaDBClient",
            "connect",
            "connect_rest",
            "connect_grpc",
            "Protocol",
        ]

        # Configuration APIs
        config_apis = ["ClientConfig", "CompressionConfig"]

        # Model APIs
        model_apis = [
            "Collection",
            "CollectionConfig",
            "VectorRecord",
            "SearchResult",
            "DistanceMetric",
            "StorageEngine",
            "IndexType",
        ]

        # Check all APIs are present
        all_apis = client_apis + config_apis + model_apis

        for api in all_apis:
            assert hasattr(proximadb_sdk, api), f"Missing public API: {api}"
            # Also check it's in __all__
            assert api in proximadb_sdk.__all__, f"API {api} not in __all__"


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
