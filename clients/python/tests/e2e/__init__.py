"""
End-to-End Test Suite for ProximaDB Python SDK

This package contains comprehensive end-to-end tests that verify:
- Complete SDK functionality across gRPC and REST protocols
- BERT embedding integration with real-world data
- Multi-disk assignment service and persistence
- Large-scale operations and performance characteristics
- ID-based search, filtered metadata search, and similarity search
- WAL flush behavior and recovery mechanisms

Test Categories:
- Comprehensive SDK Suite: Core functionality across all protocols
- gRPC-Specific Operations: Protocol-specific features and performance
- Multi-Disk Persistence: Assignment service and storage distribution
- Performance Tests: Large-scale operations and benchmarking

Usage:
    # Run all e2e tests
    pytest tests/e2e/ -v
    
    # Run with coverage
    pytest tests/e2e/ --cov=proximadb --cov-report=html
    
    # Run specific test categories
    pytest tests/e2e/ -m "integration and not large_scale"
    pytest tests/e2e/ -m "grpc and performance"
    
    # Run with custom server configuration
    pytest tests/e2e/ --grpc-port=5679 --rest-port=5678 --server-host=localhost
"""

__version__ = "1.0.0"
__author__ = "ProximaDB Development Team"