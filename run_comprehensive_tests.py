#!/usr/bin/env python3
"""
Run comprehensive tests for 1K, 5K, and 25K vectors with detailed metrics
"""
import subprocess
import time
import glob
import os
import json
from datetime import datetime

def count_parquet_files():
    """Count and analyze Parquet files"""
    patterns = [
        "/workspace/data/disk2/storage/**/*.parquet",
        "/tmp/proximadb/**/*.parquet",
        "/data/proximadb/**/*.parquet"
    ]
    
    parquet_info = {}
    total_files = 0
    total_size = 0
    
    for pattern in patterns:
        files = glob.glob(pattern, recursive=True)
        for f in files:
            try:
                size = os.path.getsize(f)
                total_size += size
                total_files += 1
                
                # Extract collection name from path
                parts = f.split('/')
                for i, part in enumerate(parts):
                    if 'bert' in part or 'grpc' in part:
                        collection = part
                        if collection not in parquet_info:
                            parquet_info[collection] = {'files': 0, 'size': 0, 'paths': []}
                        parquet_info[collection]['files'] += 1
                        parquet_info[collection]['size'] += size
                        parquet_info[collection]['paths'].append(f)
                        break
            except:
                pass
    
    return {
        'total_files': total_files,
        'total_size': total_size,
        'collections': parquet_info
    }

def run_test(test_size, script_name):
    """Run a test and capture metrics"""
    print(f"\n{'='*80}")
    print(f"🚀 Running {test_size} Vector Test")
    print(f"{'='*80}")
    
    # Count files before test
    before_files = count_parquet_files()
    
    # Run test
    start_time = time.time()
    result = subprocess.run(
        f"PYTHONPATH=/workspace/clients/python/src python3 {script_name}",
        shell=True,
        capture_output=True,
        text=True
    )
    end_time = time.time()
    
    # Count files after test
    after_files = count_parquet_files()
    
    # Parse metrics from output
    metrics = {
        'test_size': test_size,
        'duration': end_time - start_time,
        'success': result.returncode == 0,
        'output': result.stdout,
        'errors': result.stderr,
        'parquet_before': before_files,
        'parquet_after': after_files,
        'new_files': after_files['total_files'] - before_files['total_files'],
        'size_increase': after_files['total_size'] - before_files['total_size']
    }
    
    # Extract key metrics from output
    output_lines = result.stdout.split('\n')
    for line in output_lines:
        if 'Vectors inserted:' in line and '/' in line:
            parts = line.split(':')[-1].strip().split('/')
            metrics['vectors_inserted'] = int(parts[0].replace(',', ''))
            metrics['vectors_total'] = int(parts[1].replace(',', ''))
        elif 'Insert rate:' in line:
            rate = line.split(':')[-1].strip().split()[0]
            metrics['insert_rate'] = float(rate)
        elif 'Search time:' in line:
            time_val = line.split(':')[-1].strip().split('s')[0]
            if 'search_times' not in metrics:
                metrics['search_times'] = []
            metrics['search_times'].append(float(time_val))
        elif 'Results found:' in line:
            count = int(line.split(':')[-1].strip())
            if 'search_results' not in metrics:
                metrics['search_results'] = []
            metrics['search_results'].append(count)
    
    return metrics

def generate_report(all_metrics):
    """Generate comprehensive report"""
    print(f"\n{'='*80}")
    print("📊 COMPREHENSIVE TEST REPORT")
    print(f"{'='*80}")
    
    print("\n📈 PERFORMANCE SUMMARY")
    print("-" * 60)
    print(f"{'Test Size':<10} {'Duration':<12} {'Insert Rate':<15} {'New Files':<12} {'Size Increase':<15}")
    print(f"{'='*10} {'='*12} {'='*15} {'='*12} {'='*15}")
    
    for m in all_metrics:
        duration = f"{m['duration']:.1f}s"
        rate = f"{m.get('insert_rate', 0):.1f} vec/s"
        new_files = str(m['new_files'])
        size_mb = f"{m['size_increase']/1024/1024:.2f} MB"
        print(f"{m['test_size']:<10} {duration:<12} {rate:<15} {new_files:<12} {size_mb:<15}")
    
    print("\n📁 PARQUET FILE DETAILS")
    print("-" * 60)
    
    # Show final state
    final_state = all_metrics[-1]['parquet_after'] if all_metrics else None
    if final_state and 'collections' in final_state:
        for collection, info in final_state['collections'].items():
            print(f"\n🗂️ Collection: {collection}")
            print(f"   Files: {info['files']}")
            print(f"   Total Size: {info['size']/1024/1024:.2f} MB")
            print(f"   Avg Size: {info['size']/info['files']/1024:.1f} KB")
            if len(info['paths']) <= 5:
                print(f"   Paths:")
                for p in info['paths']:
                    print(f"     - {p}")
    
    print("\n🔍 SEARCH PERFORMANCE")
    print("-" * 60)
    
    for m in all_metrics:
        if 'search_times' in m and m['search_times']:
            avg_search = sum(m['search_times']) / len(m['search_times'])
            avg_results = sum(m.get('search_results', [])) / len(m.get('search_results', [])) if m.get('search_results') else 0
            print(f"{m['test_size']}: Avg search time: {avg_search:.3f}s, Avg results: {avg_results:.1f}")
    
    # Save detailed report
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    report_file = f"test_report_{timestamp}.json"
    with open(report_file, 'w') as f:
        json.dump(all_metrics, f, indent=2, default=str)
    print(f"\n💾 Detailed report saved to: {report_file}")

def main():
    """Run all tests"""
    print("🏃 Running Comprehensive gRPC Performance Tests")
    print(f"Timestamp: {datetime.now()}")
    
    tests = [
        ("1K", "tests/python/performance/test_grpc_1k_bert.py"),
        ("5K", "tests/python/performance/test_grpc_5k_bert.py"),
        ("25K", "tests/python/performance/test_grpc_25k_bert.py")
    ]
    
    all_metrics = []
    
    for test_size, script in tests:
        if os.path.exists(script):
            metrics = run_test(test_size, script)
            all_metrics.append(metrics)
            
            # Brief pause between tests
            time.sleep(2)
        else:
            print(f"⚠️ Test script not found: {script}")
    
    # Generate comprehensive report
    generate_report(all_metrics)
    
    print("\n✅ All tests completed!")

if __name__ == "__main__":
    main()