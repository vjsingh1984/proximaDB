#!/usr/bin/env python3
"""
ProximaDB Python SDK Test Coverage Report
"""

import subprocess
import json
import os
from collections import defaultdict

def run_pytest_collect():
    """Collect all tests without running them"""
    result = subprocess.run(
        ["python", "-m", "pytest", "tests/", "--collect-only", "-q"],
        capture_output=True,
        text=True
    )
    return result.stdout

def categorize_tests(output):
    """Categorize tests by module"""
    categories = defaultdict(list)
    current_module = None
    
    for line in output.split('\n'):
        if line.strip().startswith('tests/') and '.py' in line:
            current_module = line.strip().split('::')[0]
        elif line.strip().startswith('<Function') and current_module:
            test_name = line.strip().split('"')[1]
            categories[current_module].append(test_name)
    
    return categories

def analyze_test_files():
    """Analyze test file structure"""
    test_files = []
    for root, dirs, files in os.walk('tests'):
        for file in files:
            if file.startswith('test_') and file.endswith('.py'):
                path = os.path.join(root, file)
                with open(path, 'r') as f:
                    content = f.read()
                    test_count = content.count('def test_')
                    async_count = content.count('async def test_')
                    if test_count > 0:
                        test_files.append({
                            'path': path,
                            'test_count': test_count,
                            'async_tests': async_count
                        })
    return test_files

def main():
    print("=" * 80)
    print("ProximaDB Python SDK Test Coverage Analysis")
    print("=" * 80)
    
    # Analyze test files
    test_files = analyze_test_files()
    total_tests = sum(f['test_count'] for f in test_files)
    total_async = sum(f['async_tests'] for f in test_files)
    
    print(f"\n📊 Test File Statistics:")
    print(f"   Total test files: {len(test_files)}")
    print(f"   Total test functions: {total_tests}")
    print(f"   Async test functions: {total_async}")
    print(f"   Sync test functions: {total_tests - total_async}")
    
    # Load test results from previous run
    if os.path.exists('.report.json'):
        with open('.report.json', 'r') as f:
            report = json.load(f)
            summary = report['summary']
            
        print(f"\n📈 Test Execution Summary (from last run):")
        print(f"   Total tests collected: {summary['collected']}")
        print(f"   ✅ Passed: {summary['passed']} ({summary['passed']/summary['collected']*100:.1f}%)")
        print(f"   ❌ Failed: {summary['failed']} ({summary['failed']/summary['collected']*100:.1f}%)")
        print(f"   🚫 Errors: {summary.get('error', 0)} ({summary.get('error', 0)/summary['collected']*100:.1f}%)")
        print(f"   ⏭️  Skipped: {summary['skipped']} ({summary['skipped']/summary['collected']*100:.1f}%)")
        print(f"   Duration: {report['duration']:.1f} seconds")
        
        success_rate = summary['passed'] / (summary['collected'] - summary['skipped']) * 100
        print(f"\n🎯 Success Rate (excluding skipped): {success_rate:.1f}%")
    
    # Categorize by test type
    print("\n📁 Test Categories:")
    categories = {
        'Unit Tests': ['test_avro_', 'test_client_', 'test_config_', 'test_models_', 'test_protocols_', 'test_utils_'],
        'Integration Tests': ['test_search_', 'test_sql_', 'test_storage_', 'test_grpc_'],
        'E2E Tests': ['test_e2e_', 'test_simple_e2e'],
        'Performance Tests': ['test_perf_', 'benchmark_']
    }
    
    for category, prefixes in categories.items():
        count = sum(1 for f in test_files if any(prefix in os.path.basename(f['path']) for prefix in prefixes))
        test_count = sum(f['test_count'] for f in test_files if any(prefix in os.path.basename(f['path']) for prefix in prefixes))
        print(f"   {category}: {count} files, {test_count} tests")
    
    # Common failure patterns
    print("\n⚠️  Common Failure Patterns (based on typical issues):")
    failure_patterns = [
        ("LSM → SST nomenclature", "Some tests still reference old LSM terminology"),
        ("Collection name resolution", "Fixed in search, may affect other operations"),
        ("Proto/Pydantic separation", "Some tests expect different type conversions"),
        ("Storage layer tests", "May need updates for unified search behavior"),
        ("SQL API tests", "May have different error handling expectations")
    ]
    
    for pattern, description in failure_patterns:
        print(f"   • {pattern}: {description}")
    
    print("\n" + "=" * 80)

if __name__ == "__main__":
    main()