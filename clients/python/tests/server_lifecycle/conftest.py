"""
Server lifecycle test configuration

These tests manage the server process and should be run in isolation.
"""

import pytest


def pytest_configure(config):
    """Register custom markers"""
    config.addinivalue_line(
        "markers",
        "server_lifecycle: marks tests that manage server process (start/stop/restart)"
    )


def pytest_collection_modifyitems(config, items):
    """Automatically mark all tests in this directory"""
    for item in items:
        if "server_lifecycle" in str(item.fspath):
            item.add_marker(pytest.mark.server_lifecycle)


@pytest.fixture(scope="session", autouse=True)
def warn_server_lifecycle():
    """Warn that these tests manage the server"""
    print("\n" + "=" * 70)
    print("⚠️  WARNING: Running Server Lifecycle Tests")
    print("=" * 70)
    print("These tests will START/STOP/RESTART the ProximaDB server.")
    print("Do NOT run these tests concurrently with other test suites!")
    print("=" * 70 + "\n")
    yield
    print("\n" + "=" * 70)
    print("✅ Server Lifecycle Tests Complete")
    print("=" * 70 + "\n")
