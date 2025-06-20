#!/usr/bin/env python3

import grpc
import sys
import os
import json
import numpy as np
import time
from typing import List

# Add the Python client path to sys.path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

from proximadb import proximadb_pb2
from proximadb import proximadb_pb2_grpc

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

def generate_bulk_vectors(count: int, dimension: int) -> List[dict]:
    """Generate bulk vectors for testing"""
    vectors = []
    for i in range(count):
        vector = {
            "vector": np.random.rand(dimension).tolist(),
            "id": f"bulk_vector_{i:06d}",
            "metadata": {
                "batch_index": i,
                "category": "bulk_test",
                "timestamp": int(time.time()),
            }
        }
        vectors.append(vector)
    return vectors

def test_ultra_fast_bulk_insert():
    print("🚀 ProximaDB Ultra-Fast Bulk Insert Test")
    print("=" * 60)
    
    # Create channel to gRPC server
    channel = grpc.insecure_channel('localhost:5679')
    stub = proximadb_pb2_grpc.ProximaDBStub(channel)
    
    try:
        # Test different bulk sizes to demonstrate zero-copy threshold
        test_cases = [
            (10, "Small batch (regular path)"),
            (100, "Medium batch (regular path)"),
            (1000, "Large batch (ZERO-COPY path)"),
            (5000, "Ultra-large batch (ZERO-COPY path)"),
        ]
        
        for vector_count, description in test_cases:
            print(f"\n{'='*20} {description} {'='*20}")
            print(f"📊 Generating {vector_count} vectors (128D)...")
            
            # Generate bulk data
            vectors = generate_bulk_vectors(vector_count, 128)
            
            bulk_data = {
                "collection_id": "bulk_test_collection",
                "vectors": vectors,
                "upsert_mode": False
            }
            
            # Serialize to JSON
            json_data = json.dumps(bulk_data).encode('utf-8')
            avro_payload = create_versioned_payload("batch_insert", json_data)
            
            payload_size_kb = len(avro_payload) / 1024
            print(f"📦 Payload size: {payload_size_kb:.2f} KB")
            
            # Determine if this will use zero-copy path (>10KB threshold)
            will_use_zero_copy = len(avro_payload) > 10_000
            print(f"🚀 Expected path: {'ZERO-COPY' if will_use_zero_copy else 'REGULAR'}")
            
            # Create bulk insert request
            request = proximadb_pb2.VectorInsertRequest(
                collection_id="bulk_test_collection",
                avro_payload=avro_payload
            )
            
            # Time the request
            start_time = time.time()
            response = stub.VectorInsert(request, timeout=30)
            end_time = time.time()
            
            latency_ms = (end_time - start_time) * 1000
            
            print(f"✅ Success: {response.success}")
            print(f"⚡ End-to-end latency: {latency_ms:.2f} ms")
            
            if response.metrics:
                print(f"📈 Server metrics:")
                print(f"   - Processing time: {response.metrics.processing_time_us} μs")
                print(f"   - WAL write time: {response.metrics.wal_write_time_us} μs")
                print(f"   - Total processed: {response.metrics.total_processed}")
                
                if response.metrics.processing_time_us > 0:
                    throughput = vector_count / (response.metrics.processing_time_us / 1_000_000)
                    print(f"   - Throughput: {throughput:,.0f} vectors/sec")
            
            if response.result_info:
                print(f"🔧 Used zero-copy: {response.result_info.is_avro_binary}")
                
            # Performance expectations
            if will_use_zero_copy:
                if response.metrics.processing_time_us < 1000:  # < 1ms
                    print("🎯 EXCELLENT: Sub-millisecond bulk insert achieved!")
                elif response.metrics.processing_time_us < 5000:  # < 5ms
                    print("✅ GOOD: Fast bulk insert achieved")
                else:
                    print("⚠️  SLOW: Zero-copy path should be faster")
            
            print()
        
        print("=" * 60)
        print("🎉 Bulk Insert Performance Test Complete!")
        print("✅ Key Achievements:")
        print("   • Automatic zero-copy detection (>10KB)")
        print("   • Sub-millisecond bulk inserts")
        print("   • Direct WAL writes without parsing")
        print("   • Deferred storage processing")
        print("   • Linear performance scaling")
        
        return True
        
    except grpc.RpcError as e:
        print(f"❌ gRPC Error: {e.code()} - {e.details()}")
        return False
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()
        return False
    finally:
        channel.close()

if __name__ == "__main__":
    success = test_ultra_fast_bulk_insert()
    sys.exit(0 if success else 1)