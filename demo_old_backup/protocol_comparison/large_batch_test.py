#!/usr/bin/env python3
"""
Large Batch Size Test for ProximaDB
Tests feasibility of 1000 and 2000 vector batches
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import numpy as np
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def test_batch_size_limits(protocol: str):
    """Test different batch sizes to find limits"""
    
    print(f"\n{'='*60}")
    print(f"Testing {protocol.upper()} Protocol Batch Size Limits")
    print(f"{'='*60}")
    
    # Connect to server
    if protocol == "rest":
        client = connect_rest("http://localhost:5678")
    else:
        client = connect_grpc("http://localhost:5679")
    
    # Create test collection
    collection_name = f"batch_test_{protocol}_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description=f"Batch size test - {protocol}"
    )
    
    collection = client.create_collection(collection_name, config)
    print(f"✅ Collection created: {collection_name}")
    
    # Test different batch sizes
    batch_sizes = [100, 500, 1000, 1500, 2000]
    results = {}
    
    for batch_size in batch_sizes:
        print(f"\n📊 Testing batch size: {batch_size}")
        
        # Generate vectors
        vectors = []
        for i in range(batch_size):
            vec = VectorRecord(
                id=f"vec_{batch_size}_{i}",
                vector=np.random.random(128).astype(np.float32).tolist(),
                metadata={"batch": batch_size, "index": i}
            )
            vectors.append(vec)
        
        # Calculate message size
        # Each vector: 128 floats * 4 bytes + metadata + overhead ≈ 600 bytes
        estimated_size_mb = (batch_size * 600) / (1024 * 1024)
        print(f"  Estimated message size: {estimated_size_mb:.2f} MB")
        
        try:
            start = time.time()
            result = client.insert_vectors(collection_name, vectors)
            insert_time = (time.time() - start) * 1000
            vectors_per_sec = (batch_size / insert_time) * 1000
            
            results[batch_size] = {
                "success": True,
                "time_ms": insert_time,
                "vectors_per_second": vectors_per_sec,
                "message_size_mb": estimated_size_mb
            }
            
            print(f"  ✅ Success: {insert_time:.2f}ms ({vectors_per_sec:.0f} vectors/sec)")
            
        except Exception as e:
            results[batch_size] = {
                "success": False,
                "error": str(e),
                "message_size_mb": estimated_size_mb
            }
            print(f"  ❌ Failed: {e}")
    
    # Cleanup
    try:
        client.delete_collection(collection_name)
    except:
        pass
    
    return results

def check_grpc_message_limits():
    """Check gRPC message size configuration"""
    
    print("\n📋 Checking gRPC Configuration:")
    print("  Default gRPC max message size: 4MB")
    print("  ProximaDB config should have: max_request_size_mb")
    
    # Let's check with a sanity test
    print("\n🔍 Running gRPC sanity test...")
    
    try:
        client = connect_grpc("http://localhost:5679")
        
        # Try small batch first
        collection_name = f"grpc_sanity_{int(time.time())}"
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER
        )
        
        collection = client.create_collection(collection_name, config)
        
        # Test with 10 vectors
        vectors = []
        for i in range(10):
            vec = VectorRecord(
                id=f"test_{i}",
                vector=np.random.random(128).astype(np.float32).tolist(),
                metadata={"test": True}
            )
            vectors.append(vec)
        
        result = client.insert_vectors(collection_name, vectors)
        print(f"  ✅ gRPC sanity test passed: inserted {result.success} vectors")
        
        client.delete_collection(collection_name)
        return True
        
    except Exception as e:
        print(f"  ❌ gRPC sanity test failed: {e}")
        return False

def main():
    """Run batch size tests"""
    
    print("🚀 ProximaDB Large Batch Size Feasibility Test")
    
    # Check gRPC first
    if not check_grpc_message_limits():
        print("\n⚠️  gRPC connection has issues, skipping gRPC large batch tests")
        test_protocols = ["rest"]
    else:
        test_protocols = ["rest", "grpc"]
    
    all_results = {}
    
    for protocol in test_protocols:
        results = test_batch_size_limits(protocol)
        all_results[protocol] = results
    
    # Print summary
    print("\n" + "="*80)
    print("BATCH SIZE FEASIBILITY SUMMARY")
    print("="*80)
    
    print(f"\n{'Protocol':<10} {'Batch Size':<12} {'Status':<10} {'Time (ms)':<12} {'Rate (vec/s)':<15} {'Size (MB)':<10}")
    print("-"*80)
    
    for protocol, results in all_results.items():
        for batch_size, result in sorted(results.items()):
            status = "✅ Success" if result["success"] else "❌ Failed"
            time_str = f"{result.get('time_ms', 0):.2f}" if result["success"] else "N/A"
            rate_str = f"{result.get('vectors_per_second', 0):.0f}" if result["success"] else "N/A"
            size_str = f"{result.get('message_size_mb', 0):.2f}"
            
            print(f"{protocol.upper():<10} {batch_size:<12} {status:<10} {time_str:<12} {rate_str:<15} {size_str:<10}")
    
    # Recommendations
    print("\n📌 RECOMMENDATIONS:")
    
    # Check if large batches work
    max_working_batch = {"rest": 0, "grpc": 0}
    for protocol, results in all_results.items():
        for batch_size, result in sorted(results.items(), reverse=True):
            if result["success"]:
                max_working_batch[protocol] = max(max_working_batch[protocol], batch_size)
    
    print(f"\n  REST API:")
    if max_working_batch.get("rest", 0) >= 2000:
        print(f"    ✅ Supports large batches up to 2000 vectors")
        print(f"    🚀 Optimal batch size: 1000-1500 for best throughput")
    elif max_working_batch.get("rest", 0) >= 1000:
        print(f"    ⚠️  Maximum working batch size: {max_working_batch['rest']} vectors")
        print(f"    📊 Consider using this as your batch limit")
    else:
        print(f"    ❌ Large batches not recommended, stick to < 1000")
    
    if "grpc" in all_results:
        print(f"\n  gRPC API:")
        if max_working_batch.get("grpc", 0) >= 2000:
            print(f"    ✅ Supports large batches up to 2000 vectors")
            print(f"    🚀 Message size handling is configured properly")
        elif max_working_batch.get("grpc", 0) >= 1000:
            print(f"    ⚠️  Maximum working batch size: {max_working_batch['grpc']} vectors")
            print(f"    📝 May need to increase max_request_size_mb in config")
        else:
            print(f"    ❌ Default 4MB gRPC limit detected")
            print(f"    📝 Increase max_request_size_mb in server config")

if __name__ == "__main__":
    main()