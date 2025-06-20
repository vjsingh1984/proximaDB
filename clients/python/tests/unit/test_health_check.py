#!/usr/bin/env python3
"""Simple health check for ProximaDB server"""

import grpc
import sys
import os

# Add the client path
sys.path.append('./clients/python/src')

try:
    import proximadb.proximadb_pb2 as pb2
    import proximadb.proximadb_pb2_grpc as pb2_grpc
    
    # Create channel and stub
    channel = grpc.insecure_channel('localhost:5678')
    stub = pb2_grpc.ProximaDBStub(channel)
    
    # Send health request
    request = pb2.HealthRequest()
    response = stub.Health(request, timeout=5)
    
    print(f"Health check successful: {response.status}")
    exit(0)
    
except Exception as e:
    print(f"Health check failed: {e}")
    exit(1)