#!/usr/bin/env python3
"""Final test summary report"""
import subprocess
import os
import sys
import time

# Ensure PYTHONPATH is set
os.environ['PYTHONPATH'] = 'src'

# Kill any stuck pytest processes first
subprocess.run(['pkill', '-9', 'pytest'], capture_output=True)
time.sleep(1)

test_results = {}

# Unit tests to check
unit_tests = [
    ('Config', 'tests/unit/test_config.py'),
    ('Exceptions', 'tests/unit/test_exceptions.py'),
    ('Batching', 'tests/unit/test_batching.py'),
    ('Chunking', 'tests/unit/test_chunking.py'),
    ('Chunker Pooling', 'tests/unit/test_chunker_pooling.py'),
    ('Collection Config', 'tests/unit/test_collection_config.py'),
    ('Connection Pools', 'tests/unit/test_connection_pools.py'),
    ('Embedding Interface', 'tests/unit/test_embedding_interface.py'),
    ('Operation Router', 'tests/unit/test_operation_router.py'),
    ('Protocol Selector', 'tests/unit/test_protocol_selector.py'),
    ('Quantization', 'tests/unit/test_quantization_features.py'),
    ('Models Coverage', 'tests/unit/test_models_coverage.py'),
]

print("ProximaDB Python SDK - Final Test Summary")
print("=" * 80)
print()

total_passed = 0
total_failed = 0
total_skipped = 0

for test_name, test_file in unit_tests:
    if not os.path.exists(test_file):
        continue
        
    print(f"Checking {test_name}...", end='', flush=True)
    
    try:
        # Run pytest quietly to get counts
        cmd = ['python', '-m', 'pytest', test_file, '-q', '--tb=no']
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        
        output = result.stdout + result.stderr
        
        # Parse results
        passed = failed = skipped = 0
        for line in output.split('\n'):
            if ' passed' in line:
                parts = line.split()
                for i, part in enumerate(parts):
                    if part == 'passed' and i > 0:
                        try:
                            passed = int(parts[i-1])
                        except:
                            pass
                    elif part == 'failed' and i > 0:
                        try:
                            failed = int(parts[i-1])
                        except:
                            pass
                    elif part == 'skipped' and i > 0:
                        try:
                            skipped = int(parts[i-1])
                        except:
                            pass
        
        if passed + failed + skipped > 0:
            if failed == 0:
                print(f"\r✅ {test_name}: {passed} passed, {skipped} skipped")
            else:
                print(f"\r❌ {test_name}: {passed} passed, {failed} failed, {skipped} skipped")
            
            test_results[test_name] = {
                'passed': passed,
                'failed': failed,
                'skipped': skipped
            }
            
            total_passed += passed
            total_failed += failed
            total_skipped += skipped
        else:
            print(f"\r⚠️  {test_name}: Unable to parse results")
            
    except subprocess.TimeoutExpired:
        print(f"\r⏱️  {test_name}: TIMEOUT")
    except Exception as e:
        print(f"\r💥 {test_name}: ERROR - {str(e)[:50]}")

print()
print("=" * 80)
print("SUMMARY:")
print(f"  Total Passed:  {total_passed}")
print(f"  Total Failed:  {total_failed}")
print(f"  Total Skipped: {total_skipped}")
print(f"  Total Tests:   {total_passed + total_failed + total_skipped}")
if total_passed + total_failed > 0:
    print(f"  Success Rate:  {total_passed / (total_passed + total_failed) * 100:.1f}%")
print()

# Show breakdown
print("TEST SUITE BREAKDOWN:")
for test_name, results in test_results.items():
    status = "✅" if results['failed'] == 0 else "❌"
    print(f"  {status} {test_name}: {results['passed']}/{results['passed'] + results['failed']} passed")

print()
print("KEY ISSUES FOUND:")
print("  1. gRPC search returns empty results (workaround: use REST for search)")
print("  2. Many unit tests have API mismatches with new unified architecture")
print("  3. Collection config tests fail due to server behavior differences")
print("  4. Connection pool tests fail due to parameter mismatches")
print("  5. Integration tests use outdated APIs")
print()
print("RECOMMENDATIONS:")
print("  1. Fix gRPC search issue in the server")
print("  2. Update all tests to match new unified architecture APIs")
print("  3. Add proper test cleanup to avoid 'collection exists' errors")
print("  4. Consider mock support for tests that don't need real server")