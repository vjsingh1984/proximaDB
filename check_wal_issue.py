#!/usr/bin/env python3
import os
import subprocess
import time

def check_wal_segments():
    """Check WAL segments and their relationship to flushes"""
    print("\n🔍 WAL Segment Analysis")
    print("="*80)
    
    # Check WAL files for both collections
    for collection in ["viper_baseline_10k", "viper_comparison_10k"]:
        print(f"\n📁 Collection: {collection}")
        
        # Find WAL files
        wal_files = []
        for disk in ["disk1", "disk2", "disk3"]:
            wal_path = f"/workspace/data/{disk}/wal/{collection}"
            if os.path.exists(wal_path):
                files = [f for f in os.listdir(wal_path) if f.endswith('.wal')]
                wal_files.extend([(disk, f, os.path.join(wal_path, f)) for f in files])
        
        print(f"  Total WAL files: {len(wal_files)}")
        
        if wal_files:
            # Sort by modification time
            wal_files_with_time = []
            for disk, fname, fpath in wal_files:
                mtime = os.path.getmtime(fpath)
                size = os.path.getsize(fpath)
                wal_files_with_time.append((disk, fname, fpath, mtime, size))
            
            wal_files_with_time.sort(key=lambda x: x[3])
            
            # Show first and last few
            print("\n  Oldest WAL files:")
            for disk, fname, fpath, mtime, size in wal_files_with_time[:3]:
                print(f"    {disk}/{fname}: {size/(1024**2):.1f} MB, {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(mtime))}")
            
            print("\n  Newest WAL files:")
            for disk, fname, fpath, mtime, size in wal_files_with_time[-3:]:
                print(f"    {disk}/{fname}: {size/(1024**2):.1f} MB, {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(mtime))}")
            
            # Calculate total size
            total_wal_size = sum(x[4] for x in wal_files_with_time)
            print(f"\n  Total WAL size: {total_wal_size/(1024**2):.1f} MB")
            
            # Check for sequential segments
            segment_numbers = []
            for _, fname, _, _, _ in wal_files_with_time:
                if 'segment_' in fname:
                    try:
                        num = int(fname.split('segment_')[1].split('.')[0])
                        segment_numbers.append(num)
                    except:
                        pass
            
            if segment_numbers:
                segment_numbers.sort()
                print(f"\n  Segment range: {min(segment_numbers)} to {max(segment_numbers)}")
                print(f"  Missing segments: {set(range(min(segment_numbers), max(segment_numbers)+1)) - set(segment_numbers)}")

def check_vector_files():
    """Check vector storage files"""
    print("\n\n📊 Vector Storage Analysis")
    print("="*80)
    
    for collection in ["viper_baseline_10k", "viper_comparison_10k"]:
        print(f"\n📁 Collection: {collection}")
        
        # Find vector files
        vector_files = []
        for disk in ["disk1", "disk2", "disk3"]:
            vectors_path = f"/workspace/data/{disk}/storage/{collection}/vectors"
            if os.path.exists(vectors_path):
                files = [f for f in os.listdir(vectors_path) if f.endswith('.parquet')]
                for f in files:
                    fpath = os.path.join(vectors_path, f)
                    size = os.path.getsize(fpath)
                    mtime = os.path.getmtime(fpath)
                    vector_files.append((disk, f, size, mtime))
        
        if vector_files:
            vector_files.sort(key=lambda x: x[3])  # Sort by time
            
            print(f"  Total vector files: {len(vector_files)}")
            print(f"  Total size: {sum(x[2] for x in vector_files)/(1024**3):.2f} GB")
            
            # Size distribution
            size_buckets = {}
            for _, _, size, _ in vector_files:
                bucket = int(size / (10 * 1024 * 1024)) * 10  # 10MB buckets
                size_buckets[bucket] = size_buckets.get(bucket, 0) + 1
            
            print("\n  Size distribution (MB):")
            for bucket in sorted(size_buckets.keys()):
                print(f"    {bucket}-{bucket+10} MB: {size_buckets[bucket]} files")

def main():
    check_wal_segments()
    check_vector_files()
    
    # Show the critical issue
    print("\n\n⚠️  CRITICAL ISSUE ANALYSIS")
    print("="*80)
    print("""
The problem: WAL segments are NOT being cleaned up after flush!
    
Expected behavior:
1. Insert vectors → Write to WAL
2. WAL reaches flush threshold → Flush to storage (Parquet/VIPER files)
3. After successful flush → DELETE the flushed WAL segments
4. Only keep active WAL segments for unflushed data

Actual behavior:
1. Insert vectors → Write to WAL ✓
2. WAL reaches threshold → Flush to storage ✓
3. After flush → WAL segments remain! ❌
4. On restart → Old WAL segments replay → Duplicate flushes! ❌

This causes:
- Exponential growth of storage files
- Each restart multiplies the data
- 50 vectors → 3.5GB of storage files!
""")

if __name__ == "__main__":
    main()