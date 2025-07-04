#!/usr/bin/env python3
"""
Monitor Parquet file creation during test runs
"""
import subprocess
import time
import glob
import os
import threading
from datetime import datetime

class ParquetMonitor:
    def __init__(self):
        self.monitoring = False
        self.file_snapshots = []
        
    def count_files(self):
        """Count Parquet files by collection"""
        patterns = [
            "/workspace/data/disk2/storage/**/*.parquet",
            "/tmp/proximadb/**/*.parquet",
            "/data/proximadb/**/*.parquet"
        ]
        
        collections = {}
        
        for pattern in patterns:
            files = glob.glob(pattern, recursive=True)
            for f in files:
                # Extract collection name
                parts = f.split('/')
                for part in parts:
                    if 'bert' in part or 'grpc' in part:
                        if part not in collections:
                            collections[part] = []
                        collections[part].append(f)
                        break
        
        return collections
    
    def monitor_loop(self):
        """Monitor files in background"""
        while self.monitoring:
            snapshot = {
                'timestamp': datetime.now(),
                'collections': self.count_files()
            }
            self.file_snapshots.append(snapshot)
            time.sleep(1)  # Check every second
    
    def start_monitoring(self):
        """Start monitoring thread"""
        self.monitoring = True
        self.monitor_thread = threading.Thread(target=self.monitor_loop)
        self.monitor_thread.start()
        
    def stop_monitoring(self):
        """Stop monitoring"""
        self.monitoring = False
        if hasattr(self, 'monitor_thread'):
            self.monitor_thread.join()
    
    def get_report(self):
        """Generate monitoring report"""
        if not self.file_snapshots:
            return "No snapshots collected"
        
        first = self.file_snapshots[0]
        last = self.file_snapshots[-1]
        
        report = []
        report.append(f"📊 Parquet File Monitoring Report")
        report.append(f"Duration: {(last['timestamp'] - first['timestamp']).total_seconds():.1f}s")
        report.append(f"Snapshots: {len(self.file_snapshots)}")
        report.append("")
        
        # Compare first and last snapshots
        for collection in set(list(first['collections'].keys()) + list(last['collections'].keys())):
            initial = len(first['collections'].get(collection, []))
            final = len(last['collections'].get(collection, []))
            
            if final > initial:
                report.append(f"📁 {collection}:")
                report.append(f"   Initial files: {initial}")
                report.append(f"   Final files: {final}")
                report.append(f"   New files: {final - initial}")
                
                # Show new files
                initial_files = set(first['collections'].get(collection, []))
                final_files = set(last['collections'].get(collection, []))
                new_files = final_files - initial_files
                
                if len(new_files) <= 5:
                    report.append(f"   New Parquet files:")
                    for f in sorted(new_files):
                        size = os.path.getsize(f) / 1024
                        report.append(f"     - {os.path.basename(f)} ({size:.1f} KB)")
                else:
                    report.append(f"   {len(new_files)} new Parquet files created")
        
        return "\n".join(report)

def run_monitored_test(test_name, command):
    """Run test with monitoring"""
    print(f"\n{'='*80}")
    print(f"🚀 Running {test_name} with Parquet Monitoring")
    print(f"{'='*80}")
    
    monitor = ParquetMonitor()
    monitor.start_monitoring()
    
    start_time = time.time()
    result = subprocess.run(command, shell=True, capture_output=True, text=True)
    end_time = time.time()
    
    # Wait a bit for final flush
    time.sleep(2)
    
    monitor.stop_monitoring()
    
    print(f"\n⏱️ Test duration: {end_time - start_time:.1f}s")
    print(f"Exit code: {result.returncode}")
    
    # Extract key metrics from output
    if result.stdout:
        lines = result.stdout.split('\n')
        for line in lines:
            if any(keyword in line for keyword in ['Vectors inserted:', 'Insert rate:', 'Search time:', 'Results found:', 'Files:']):
                print(f"   {line.strip()}")
    
    print(f"\n{monitor.get_report()}")
    
    return result

def main():
    """Run tests with monitoring"""
    print("🔍 Parquet File Monitoring for gRPC Tests")
    print(f"Timestamp: {datetime.now()}")
    
    tests = [
        ("1K BERT Test", "PYTHONPATH=/workspace/clients/python/src python3 tests/python/performance/test_grpc_1k_bert.py"),
        ("5K BERT Test", "PYTHONPATH=/workspace/clients/python/src python3 tests/python/performance/test_grpc_5k_bert.py"),
        ("25K BERT Test", "PYTHONPATH=/workspace/clients/python/src python3 tests/python/performance/test_grpc_25k_bert.py")
    ]
    
    for test_name, command in tests:
        run_monitored_test(test_name, command)
        time.sleep(3)  # Pause between tests

if __name__ == "__main__":
    main()