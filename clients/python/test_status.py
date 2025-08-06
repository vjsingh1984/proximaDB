#!/usr/bin/env python3
"""Get quick status of all tests"""
import subprocess
import sys
import time

test_groups = [
    ("Config Tests", ["tests/unit/test_config.py"]),
    ("Exception Tests", ["tests/unit/test_exceptions.py"]),
    ("Batching Tests", ["tests/unit/test_batching.py"]),
    ("Chunking Tests", ["tests/unit/test_chunking.py", "tests/unit/test_chunker_pooling.py"]),
    ("Collection Config Tests", ["tests/unit/test_collection_config.py"]),
    ("Other Unit Tests", ["tests/unit/test_*.py"])
]

total_passed = 0
total_failed = 0
failed_tests = []

print("ProximaDB Python SDK Test Status")
print("=" * 50)
print()

for group_name, test_files in test_groups:
    if group_name == "Other Unit Tests":
        # Skip already tested files
        cmd = ["python", "-m", "pytest"] + test_files + [
            "--ignore=tests/unit/test_config.py",
            "--ignore=tests/unit/test_exceptions.py", 
            "--ignore=tests/unit/test_batching.py",
            "--ignore=tests/unit/test_chunking.py",
            "--ignore=tests/unit/test_chunker_pooling.py",
            "--ignore=tests/unit/test_collection_config.py",
            "-q", "--tb=no"
        ]
    else:
        cmd = ["python", "-m", "pytest"] + test_files + ["-q", "--tb=no"]
    
    print(f"Running {group_name}...")
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    
    # Parse output
    output = result.stdout + result.stderr
    
    # Look for summary line
    for line in output.split('\n'):
        if ' passed' in line or ' failed' in line:
            print(f"  {line.strip()}")
            
            # Extract counts
            if ' passed' in line:
                parts = line.split()
                for i, part in enumerate(parts):
                    if part == 'passed':
                        passed = int(parts[i-1])
                        total_passed += passed
                    elif part == 'failed':
                        failed = int(parts[i-1])
                        total_failed += failed
                        failed_tests.append(group_name)
            break
    
    time.sleep(0.5)  # Brief pause between test groups

print()
print("=" * 50)
print(f"OVERALL: {total_passed} passed, {total_failed} failed")
print(f"Success Rate: {total_passed / (total_passed + total_failed) * 100:.1f}%")

if failed_tests:
    print(f"\nFailed test groups: {', '.join(set(failed_tests))}")
else:
    print("\n✅ All tests passing!")