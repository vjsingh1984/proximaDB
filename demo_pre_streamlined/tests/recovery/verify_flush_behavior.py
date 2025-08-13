#!/usr/bin/env python3
"""
Flush Behavior Verification Script
Monitors WAL and storage behavior to verify flush triggers
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
import glob
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def monitor_data_directories():
    """Monitor data directories for flush activity"""
    
    # ProximaDB data directories
    data_dirs = {
        "metadata": "./test_data/metadata",
        "storage": "./test_data/storage", 
        "wal": "./test_data/wal",
        "lsm_wal": "./test_data/lsm_wal",
        "lsm_data": "./test_data/lsm_data"
    }
    
    print("📁 Monitoring data directories for flush activity...")
    
    initial_state = {}
    for name, path in data_dirs.items():
        try:
            if os.path.exists(path):
                files = glob.glob(f"{path}/**/*", recursive=True)
                file_info = {}
                for f in files:
                    if os.path.isfile(f):
                        stat = os.stat(f)
                        file_info[f] = {
                            "size": stat.st_size,
                            "mtime": stat.st_mtime
                        }
                initial_state[name] = file_info
            else:
                initial_state[name] = {}
        except Exception as e:
            print(f"Warning: Could not monitor {path}: {e}")
            initial_state[name] = {}
    
    return initial_state

def detect_flush_activity(initial_state, operation_desc):
    """Detect flush activity by comparing directory states"""
    
    print(f"\n🔍 Checking for flush activity after {operation_desc}...")
    
    # Wait a moment for potential flush
    time.sleep(2)
    
    data_dirs = {
        "metadata": "./test_data/metadata",
        "storage": "./test_data/storage", 
        "wal": "./test_data/wal",
        "lsm_wal": "./test_data/lsm_wal",
        "lsm_data": "./test_data/lsm_data"
    }
    
    current_state = {}
    changes_detected = {}
    
    for name, path in data_dirs.items():
        try:
            if os.path.exists(path):
                files = glob.glob(f"{path}/**/*", recursive=True)
                file_info = {}
                for f in files:
                    if os.path.isfile(f):
                        stat = os.stat(f)
                        file_info[f] = {
                            "size": stat.st_size,
                            "mtime": stat.st_mtime
                        }
                current_state[name] = file_info
            else:
                current_state[name] = {}
        except Exception as e:
            current_state[name] = {}
    
    # Compare states
    for dir_name in data_dirs.keys():
        initial = initial_state.get(dir_name, {})
        current = current_state.get(dir_name, {})
        
        changes = []
        
        # Check for new files
        for file_path in current:
            if file_path not in initial:
                changes.append(f"NEW: {file_path} ({current[file_path]['size']} bytes)")
        
        # Check for modified files
        for file_path in current:
            if file_path in initial:
                if (current[file_path]['size'] != initial[file_path]['size'] or
                    current[file_path]['mtime'] != initial[file_path]['mtime']):
                    old_size = initial[file_path]['size']
                    new_size = current[file_path]['size']
                    changes.append(f"MODIFIED: {file_path} ({old_size} -> {new_size} bytes)")
        
        # Check for deleted files
        for file_path in initial:
            if file_path not in current:
                changes.append(f"DELETED: {file_path}")
        
        if changes:
            changes_detected[dir_name] = changes
    
    return changes_detected, current_state

def test_flush_triggers():
    """Test flush triggers with different batch sizes and engines"""
    
    print("🚀 Testing Flush Triggers in ProximaDB")
    print("="*80)
    
    # Initial state
    initial_state = monitor_data_directories()
    
    # Test configurations
    test_configs = [
        {
            "name": "Small Batch gRPC VIPER",
            "client": connect_grpc("http://localhost:5679"),
            "collection": "flush_test_grpc_viper_small",
            "engine": StorageEngine.VIPER,
            "batch_size": 500,
            "num_batches": 5,
            "expected_flush": False
        },
        {
            "name": "Large Batch gRPC VIPER",
            "client": connect_grpc("http://localhost:5679"),
            "collection": "flush_test_grpc_viper_large",
            "engine": StorageEngine.VIPER,
            "batch_size": 5000,
            "num_batches": 5,
            "expected_flush": True
        },
        {
            "name": "Small Batch gRPC LSM",
            "client": connect_grpc("http://localhost:5679"),
            "collection": "flush_test_grpc_lsm_small",
            "engine": StorageEngine.LSM,
            "batch_size": 500,
            "num_batches": 5,
            "expected_flush": False
        },
        {
            "name": "Large Batch gRPC LSM",
            "client": connect_grpc("http://localhost:5679"),
            "collection": "flush_test_grpc_lsm_large",
            "engine": StorageEngine.LSM,
            "batch_size": 5000,
            "num_batches": 5,
            "expected_flush": True
        }
    ]
    
    flush_results = {}
    current_state = initial_state
    
    for config in test_configs:
        print(f"\n{'='*60}")
        print(f"Testing: {config['name']}")
        print(f"Engine: {config['engine']}, Batch: {config['batch_size']}, Batches: {config['num_batches']}")
        print(f"{'='*60}")
        
        try:
            # Create collection
            collection_config = CollectionConfig(
                name=config['collection'],
                dimension=128,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=config['engine'],
                description=f"Flush test: {config['name']}"
            )
            
            collection = config['client'].create_collection(config['collection'], collection_config)
            print(f"✅ Collection created: {config['collection']}")
            
            # Insert batches and monitor flush activity
            total_vectors = 0
            
            for batch_num in range(config['num_batches']):
                # Generate batch
                vectors = []
                for i in range(config['batch_size']):
                    vec_id = f"{config['collection']}_batch_{batch_num}_vec_{i}"
                    vec_data = np.random.randn(128).astype(np.float32)
                    vec_data = vec_data / np.linalg.norm(vec_data)
                    
                    vectors.append(VectorRecord(
                        id=vec_id,
                        vector=vec_data.tolist(),
                        metadata={
                            "batch_num": batch_num,
                            "vector_index": i,
                            "test_config": config['name']
                        }
                    ))
                
                # Insert batch
                start_time = time.time()
                config['client'].insert_vectors(config['collection'], vectors)
                insert_time = time.time() - start_time
                
                total_vectors += len(vectors)
                print(f"  Batch {batch_num + 1}: {len(vectors)} vectors in {insert_time:.2f}s")
                
                # Check for flush activity after each batch
                changes, current_state = detect_flush_activity(
                    current_state, 
                    f"batch {batch_num + 1} ({total_vectors} total vectors)"
                )
                
                if changes:
                    print(f"  🔄 Flush detected after batch {batch_num + 1}:")
                    for dir_name, change_list in changes.items():
                        print(f"    {dir_name}:")
                        for change in change_list:
                            print(f"      - {change}")
                else:
                    print(f"  ⏳ No flush detected after batch {batch_num + 1}")
            
            # Wait for potential delayed flush
            print(f"\n⏳ Waiting for potential delayed flush...")
            time.sleep(5)
            
            # Final flush check
            final_changes, current_state = detect_flush_activity(
                current_state,
                f"final check ({total_vectors} total vectors)"
            )
            
            flush_occurred = bool(final_changes)
            
            if final_changes:
                print(f"  🔄 Final flush detected:")
                for dir_name, change_list in final_changes.items():
                    print(f"    {dir_name}:")
                    for change in change_list:
                        print(f"      - {change}")
            
            # Verify data persistence with search
            print(f"\n🔍 Verifying data with search...")
            query = np.random.randn(128).astype(np.float32)
            query = query / np.linalg.norm(query)
            
            search_results = config['client'].search(config['collection'], query.tolist(), top_k=10)
            search_success = len(search_results) > 0
            
            print(f"  Search results: {len(search_results)} vectors found")
            
            # Store results
            flush_results[config['name']] = {
                "total_vectors": total_vectors,
                "batch_size": config['batch_size'],
                "num_batches": config['num_batches'],
                "engine": str(config['engine']),
                "flush_occurred": flush_occurred,
                "expected_flush": config['expected_flush'],
                "flush_behavior_correct": flush_occurred == config['expected_flush'],
                "search_success": search_success,
                "file_changes": final_changes
            }
            
            # Cleanup
            config['client'].delete_collection(config['collection'])
            
        except Exception as e:
            print(f"❌ Error testing {config['name']}: {e}")
            flush_results[config['name']] = {
                "error": str(e),
                "flush_occurred": False,
                "expected_flush": config['expected_flush'],
                "flush_behavior_correct": False
            }
    
    # Save results
    with open("flush_behavior_results.json", "w") as f:
        json.dump(flush_results, f, indent=2)
    
    # Print summary
    print("\n" + "="*80)
    print("FLUSH BEHAVIOR SUMMARY")
    print("="*80)
    
    for test_name, result in flush_results.items():
        if "error" in result:
            print(f"❌ {test_name}: ERROR - {result['error']}")
        else:
            flush_status = "✅" if result['flush_behavior_correct'] else "❌"
            expected = "Yes" if result['expected_flush'] else "No"
            occurred = "Yes" if result['flush_occurred'] else "No"
            print(f"{flush_status} {test_name}:")
            print(f"    Expected flush: {expected}, Occurred: {occurred}")
            print(f"    Vectors: {result['total_vectors']}, Search: {'✅' if result['search_success'] else '❌'}")
    
    return flush_results

if __name__ == "__main__":
    results = test_flush_triggers()
    print("\n📊 Results saved to flush_behavior_results.json")