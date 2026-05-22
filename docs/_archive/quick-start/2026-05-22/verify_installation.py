#!/usr/bin/env python3
"""
ProximaDB Installation Verification Script

Run this script to verify that `pip install proximadb-python` was successful.
This script tests basic functionality without requiring a running server.
"""

import sys


def test_imports():
    """Test that all main modules can be imported."""
    print("🔍 Testing imports...")

    try:
        import proximadb_sdk
        print("  ✅ proximadb_sdk")
    except ImportError as e:
        print(f"  ❌ proximadb_sdk: {e}")
        return False

    try:
        from proximadb_sdk import connect
        print("  ✅ connect()")
    except ImportError as e:
        print(f"  ❌ connect: {e}")
        return False

    try:
        from proximadb_sdk import ProximaDBClient
        print("  ✅ ProximaDBClient")
    except ImportError as e:
        print(f"  ❌ ProximaDBClient: {e}")
        return False

    try:
        from proximadb_sdk.models import CollectionConfig
        print("  ✅ CollectionConfig (models)")
    except ImportError as e:
        print(f"  ❌ models: {e}")
        return False

    try:
        from proximadb_sdk.auth import AuthConfig, AuthMethod
        print("  ✅ AuthConfig, AuthMethod")
    except ImportError as e:
        print(f"  ❌ auth: {e}")
        return False

    try:
        from proximadb_sdk.exceptions import ProximaDBError
        print("  ✅ ProximaDBError")
    except ImportError as e:
        print(f"  ❌ exceptions: {e}")
        return False

    return True


def test_dependencies():
    """Test that required dependencies are available."""
    print("\n🔍 Testing dependencies...")

    dependencies = [
        ("numpy", "numpy"),
        ("httpx", "httpx"),
        ("grpcio", "grpc"),
        ("protobuf", "google.protobuf"),
        ("pydantic", "pydantic"),
    ]

    all_ok = True
    for pkg_name, import_name in dependencies:
        try:
            __import__(import_name)
            print(f"  ✅ {pkg_name}")
        except ImportError:
            print(f"  ❌ {pkg_name} (not installed)")
            all_ok = False

    return all_ok


def test_client_creation():
    """Test that client can be instantiated (without connecting)."""
    print("\n🔍 Testing client creation...")

    try:
        from proximadb_sdk import ProximaDBClient

        # Create client without connecting
        client = ProximaDBClient(url="http://localhost:5678")
        print(f"  ✅ Client created: {type(client).__name__}")
        return True
    except Exception as e:
        print(f"  ❌ Client creation failed: {e}")
        return False


def test_config_models():
    """Test that configuration models work."""
    print("\n🔍 Testing configuration models...")

    try:
        from proximadb_sdk.models import CollectionConfig, DistanceMetric

        config = CollectionConfig(
            name="test_collection",
            dimension=384,
            distance_metric=DistanceMetric.COSINE
        )

        assert config.name == "test_collection"
        assert config.dimension == 384
        print(f"  ✅ CollectionConfig: {config.name} ({config.dimension}D)")
        return True
    except Exception as e:
        print(f"  ❌ Configuration models failed: {e}")
        return False


def test_auth_config():
    """Test that authentication configuration works."""
    print("\n🔍 Testing authentication configuration...")

    try:
        from proximadb_sdk.auth import AuthConfig, AuthMethod

        config = AuthConfig(
            method=AuthMethod.API_KEY,
            api_key="test-key-12345"
        )

        assert config.method == AuthMethod.API_KEY
        assert config.api_key == "test-key-12345"
        print(f"  ✅ AuthConfig: API_KEY method")
        return True
    except Exception as e:
        print(f"  ❌ Auth configuration failed: {e}")
        return False


def main():
    """Run all verification tests."""
    print("=" * 70)
    print("ProximaDB Python SDK Installation Verification")
    print("=" * 70)

    tests = [
        ("Imports", test_imports),
        ("Dependencies", test_dependencies),
        ("Client Creation", test_client_creation),
        ("Config Models", test_config_models),
        ("Auth Config", test_auth_config),
    ]

    results = []
    for test_name, test_func in tests:
        try:
            result = test_func()
            results.append((test_name, result))
        except Exception as e:
            print(f"\n❌ {test_name} failed with exception: {e}")
            results.append((test_name, False))

    # Print summary
    print("\n" + "=" * 70)
    print("VERIFICATION SUMMARY")
    print("=" * 70)

    passed = sum(1 for _, result in results if result)
    total = len(results)

    for test_name, result in results:
        status = "✅ PASS" if result else "❌ FAIL"
        print(f"  {status}: {test_name}")

    print(f"\nTotal: {passed}/{total} tests passed")

    if passed == total:
        print("\n🎉 Installation verified successfully!")
        print("\nNext steps:")
        print("  1. Start ProximaDB server: cargo run --bin proximadb-server")
        print("  2. Run examples: python clients/python/examples/basic_usage.py")
        print("  3. Read docs: docs/01-quick-start/legacy/QUICKSTART.adoc")
        return 0
    else:
        print("\n⚠️  Installation verification failed!")
        print("\nTroubleshooting:")
        print("  1. Reinstall package: pip install --force-reinstall proximadb-python")
        print("  2. Check Python version: python3 --version (requires 3.10+)")
        print("  3. Check dependencies: pip list | grep -E '(numpy|httpx|grpcio)'")
        return 1


if __name__ == "__main__":
    sys.exit(main())
