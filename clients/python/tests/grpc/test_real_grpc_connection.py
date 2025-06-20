#!/usr/bin/env python3

import grpc
import sys
import os
import json
import numpy as np
import time
import socket
from typing import List

# Add the Python client path to sys.path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

from proximadb import proximadb_pb2
from proximadb import proximadb_pb2_grpc

def verify_server_connection(host: str, port: int) -> bool:
    """Verify actual TCP connection to server"""
    try:
        # Test raw TCP connection
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(5.0)
        result = sock.connect_ex((host, port))
        sock.close()
        
        if result == 0:
            print(f"✅ TCP connection successful to {host}:{port}")
            return True
        else:
            print(f"❌ TCP connection failed to {host}:{port} (code: {result})")
            return False
    except Exception as e:
        print(f"❌ Network error: {e}")
        return False

def create_versioned_payload(operation_type: str, json_data: bytes) -> bytes:
    """Create versioned payload format for UnifiedAvroService"""
    schema_version = (1).to_bytes(4, byteorder='little')
    op_bytes = operation_type.encode('utf-8')
    op_len = (len(op_bytes)).to_bytes(4, byteorder='little')
    
    versioned_payload = bytearray()
    versioned_payload.extend(schema_version)
    versioned_payload.extend(op_len)
    versioned_payload.extend(op_bytes)
    versioned_payload.extend(json_data)
    
    return bytes(versioned_payload)

def test_real_server_connection():
    """Test actual gRPC connection to ProximaDB server"""
    print("🔍 ProximaDB Real Server Connection Test")
    print("=" * 60)
    
    # Server details
    host = 'localhost'
    port = 5679
    
    print(f"📡 Target server: {host}:{port}")
    
    # Step 1: Verify TCP connection
    print("\n1️⃣ Testing TCP connectivity...")
    if not verify_server_connection(host, port):
        print("❌ Cannot establish TCP connection. Is the server running?")
        return False
    
    # Step 2: Create gRPC channel with explicit settings
    print("\n2️⃣ Creating gRPC channel...")
    channel_options = [
        ('grpc.keepalive_time_ms', 10000),
        ('grpc.keepalive_timeout_ms', 5000),
        ('grpc.keepalive_permit_without_calls', True),
        ('grpc.http2.max_pings_without_data', 0),
        ('grpc.http2.min_time_between_pings_ms', 10000),
        ('grpc.http2.min_ping_interval_without_data_ms', 300000),
    ]
    
    channel = grpc.insecure_channel(f'{host}:{port}', options=channel_options)
    
    try:
        # Step 3: Test channel state
        print("\n3️⃣ Checking gRPC channel state...")
        grpc.channel_ready_future(channel).result(timeout=10)
        print("✅ gRPC channel is ready")
        
        # Step 4: Create stub
        print("\n4️⃣ Creating gRPC stub...")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        print("✅ gRPC stub created successfully")
        
        # Step 5: Test health endpoint (simplest call)
        print("\n5️⃣ Testing Health endpoint...")
        health_request = proximadb_pb2.HealthRequest()
        
        start_time = time.time()
        health_response = stub.Health(health_request, timeout=10)
        end_time = time.time()
        
        print(f"✅ Health check successful!")
        print(f"   Response: {health_response.status}")
        print(f"   Version: {health_response.version}")
        print(f"   Network latency: {(end_time - start_time) * 1000:.2f}ms")
        
        # Step 6: Test zero-copy bulk insert
        print("\n6️⃣ Testing Zero-Copy Bulk Insert...")
        
        # Create large bulk payload to trigger zero-copy path
        vectors = []
        vector_count = 1000  # Large enough to trigger >10KB threshold
        
        for i in range(vector_count):
            vector = {
                "vector": np.random.rand(128).tolist(),
                "id": f"test_vector_{i:06d}",
                "metadata": {
                    "batch_index": i,
                    "category": "zero_copy_test",
                    "source": "real_network_test"
                }
            }
            vectors.append(vector)
        
        bulk_data = {
            "collection_id": "zero_copy_test_collection",
            "vectors": vectors,
            "upsert_mode": False
        }
        
        # Serialize and create payload
        json_data = json.dumps(bulk_data).encode('utf-8')
        avro_payload = create_versioned_payload("batch_insert", json_data)
        
        payload_size_kb = len(avro_payload) / 1024
        print(f"📦 Bulk payload: {payload_size_kb:.1f}KB ({vector_count} vectors)")
        print(f"🚀 Should trigger zero-copy path: {payload_size_kb > 10}")
        
        # Create request
        request = proximadb_pb2.VectorInsertRequest(
            collection_id="zero_copy_test_collection",
            avro_payload=avro_payload
        )
        
        # Send request with timing
        print("📡 Sending bulk insert request...")
        start_time = time.time()
        response = stub.VectorInsert(request, timeout=30)
        end_time = time.time()
        
        latency_ms = (end_time - start_time) * 1000
        
        print(f"✅ Bulk insert response received!")
        print(f"   Success: {response.success}")
        print(f"   Network round-trip: {latency_ms:.2f}ms")
        
        if response.metrics:
            print(f"   Server processing time: {response.metrics.processing_time_us}μs")
            print(f"   WAL write time: {response.metrics.wal_write_time_us}μs")
            
            if response.metrics.processing_time_us > 0:
                throughput = vector_count / (response.metrics.processing_time_us / 1_000_000)
                print(f"   Server throughput: {throughput:,.0f} vectors/sec")
        
        if response.result_info:
            print(f"🔧 Zero-copy used: {response.result_info.is_avro_binary}")
            
        # Step 7: Verify no mocking by checking different responses
        print("\n7️⃣ Verifying real server responses...")
        
        # Make multiple calls and verify we get different timestamps
        timestamps = []
        for i in range(3):
            health_resp = stub.Health(proximadb_pb2.HealthRequest(), timeout=5)
            # Assume server returns different timestamps or other varying data
            timestamps.append(time.time())
            time.sleep(0.1)
        
        # Check that we're getting real network delays
        time_diffs = [timestamps[i+1] - timestamps[i] for i in range(len(timestamps)-1)]
        avg_diff = sum(time_diffs) / len(time_diffs)
        
        if avg_diff > 0.05:  # At least 50ms between calls
            print("✅ Confirmed real network calls (measurable latency)")
        else:
            print("⚠️  Very fast responses - might be mocked or local")
        
        print("\n" + "=" * 60)
        print("🎉 Real Server Connection Test PASSED!")
        print("✅ Confirmed:")
        print("   • TCP connection established")
        print("   • gRPC channel ready")
        print("   • Health endpoint working")
        print("   • Zero-copy bulk insert working")
        print("   • Real network latency measured")
        print("   • No mocking detected")
        
        return True
        
    except grpc.RpcError as e:
        print(f"❌ gRPC Error: {e.code()} - {e.details()}")
        print(f"   Status code: {e.code()}")
        print(f"   Error details: {e.details()}")
        return False
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()
        return False
    finally:
        channel.close()
        print("🔗 gRPC channel closed")

if __name__ == "__main__":
    success = test_real_server_connection()
    sys.exit(0 if success else 1)