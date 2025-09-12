#!/usr/bin/env python3
"""Generate test summary report"""
import subprocess
import os
import sys

# Ensure PYTHONPATH is set
os.environ['PYTHONPATH'] = 'src'

test_suites = {
    'Config Tests': 'tests/unit/test_config.py',
    'Exception Tests': 'tests/unit/test_exceptions.py',
    'Batching Tests': 'tests/unit/test_batching.py',
    'Chunking Tests': 'tests/unit/test_chunking.py',
    'Chunker Pooling Tests': 'tests/unit/test_chunker_pooling.py',
    'Collection Config Tests': 'tests/unit/test_collection_config.py',
    'Connection Pools Tests': 'tests/unit/test_connection_pools.py',
    'Embedding Interface Tests': 'tests/unit/test_embedding_interface.py',
    'Operation Router Tests': 'tests/unit/test_operation_router.py',
    'Protocol Selector Tests': 'tests/unit/test_protocol_selector.py',
    'Response Cache Tests': 'tests/unit/test_response_cache.py',
    'REST Batching Tests': 'tests/unit/test_rest_batching.py',
    'Semantic Chunking Tests': 'tests/unit/test_semantic_chunking.py',
}

results = {}
total_passed = 0
total_failed = 0
total_skipped = 0

print("ProximaDB Python SDK Test Summary")
print("=" * 80)
print()

for suite_name, test_file in test_suites.items():
    if not os.path.exists(test_file):
        print(f"❓ {suite_name}: File not found - {test_file}")
        continue
    
    print(f"Running {suite_name}...", end='', flush=True)
    
    try:
        # Run pytest with timeout
        cmd = ['python', '-m', 'pytest', test_file, '-v', '--tb=no', '--timeout=30']
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=35)
        
        output = result.stdout + result.stderr
        
        # Parse results
        passed = failed = skipped = 0
        for line in output.split('\n'):
            if ' passed' in line or ' failed' in line or ' skipped' in line:
                parts = line.split()
                for i, part in enumerate(parts):
                    if part == 'passed' and i > 0:
                        passed = int(parts[i-1])
                    elif part == 'failed' and i > 0:
                        failed = int(parts[i-1])
                    elif part == 'skipped' and i > 0:
                        skipped = int(parts[i-1])
        
        total = passed + failed + skipped
        if total > 0:
            if failed == 0:
                status = "✅"
            else:
                status = "❌"
            print(f"\r{status} {suite_name}: {passed} passed, {failed} failed, {skipped} skipped")
            
            total_passed += passed
            total_failed += failed
            total_skipped += skipped
        else:
            print(f"\r⚠️  {suite_name}: No test results found")
    
    except subprocess.TimeoutExpired:
        print(f"\r⏱️  {suite_name}: TIMEOUT (>30s)")
    except Exception as e:
        print(f"\r💥 {suite_name}: ERROR - {e}")

print()
print("=" * 80)
print(f"TOTAL: {total_passed} passed, {total_failed} failed, {total_skipped} skipped")
print(f"Success Rate: {total_passed / (total_passed + total_failed) * 100:.1f}%" if total_passed + total_failed > 0 else "No tests ran")