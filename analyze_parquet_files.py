#!/usr/bin/env python3
import pyarrow.parquet as pq
import pandas as pd
import os
import glob
from collections import defaultdict

def analyze_parquet_files(collection_name):
    """Analyze all Parquet files for a collection"""
    # Find all parquet files
    pattern = f"/workspace/data/*/storage/{collection_name}/vectors/*.parquet"
    files = glob.glob(pattern)
    
    print(f"\n📊 Analysis for collection: {collection_name}")
    print(f"{'='*60}")
    print(f"Total Parquet files: {len(files)}")
    
    if not files:
        print("No Parquet files found!")
        return
    
    total_rows = 0
    total_size = 0
    file_stats = []
    
    # Analyze each file
    for idx, file_path in enumerate(sorted(files)):
        try:
            # Get file size
            file_size = os.path.getsize(file_path)
            total_size += file_size
            
            # Read Parquet metadata
            parquet_file = pq.ParquetFile(file_path)
            metadata = parquet_file.metadata
            
            num_rows = metadata.num_rows
            total_rows += num_rows
            
            # Get schema
            schema = parquet_file.schema
            
            file_stats.append({
                'file': os.path.basename(file_path),
                'rows': num_rows,
                'size_mb': file_size / (1024 * 1024),
                'row_groups': metadata.num_row_groups,
                'columns': len(schema)
            })
            
            # Show first file details
            if idx == 0:
                print(f"\nFirst file schema:")
                print(f"  Columns: {schema.names}")
                print(f"  Types: {[str(field.type) for field in schema]}")
                
                # Read first few rows
                df = parquet_file.read(columns=['id', 'collection_id']).to_pandas()
                print(f"\n  Sample data (first 5 rows):")
                print(df.head())
                
        except Exception as e:
            print(f"Error reading {file_path}: {e}")
    
    # Summary statistics
    print(f"\n📈 Summary Statistics:")
    print(f"  Total files: {len(files)}")
    print(f"  Total rows: {total_rows:,}")
    print(f"  Total size: {total_size / (1024**3):.2f} GB")
    print(f"  Average rows per file: {total_rows / len(files):.1f}")
    print(f"  Average file size: {total_size / len(files) / (1024**2):.1f} MB")
    
    # Show largest files
    print(f"\n🔝 Largest files:")
    sorted_files = sorted(file_stats, key=lambda x: x['size_mb'], reverse=True)
    for f in sorted_files[:5]:
        print(f"  {f['file']}: {f['size_mb']:.1f} MB, {f['rows']:,} rows")
    
    # Show files with most rows
    print(f"\n📊 Files with most rows:")
    sorted_by_rows = sorted(file_stats, key=lambda x: x['rows'], reverse=True)
    for f in sorted_by_rows[:5]:
        print(f"  {f['file']}: {f['rows']:,} rows, {f['size_mb']:.1f} MB")
    
    return total_rows, total_size

# Analyze both collections
baseline_rows, baseline_size = analyze_parquet_files("viper_baseline_10k")
comparison_rows, comparison_size = analyze_parquet_files("viper_comparison_10k")

print(f"\n🔍 Comparison:")
print(f"{'='*60}")
print(f"Baseline collection:")
print(f"  Total rows: {baseline_rows:,}")
print(f"  Total size: {baseline_size / (1024**3):.2f} GB")
print(f"  Bytes per row: {baseline_size / baseline_rows if baseline_rows > 0 else 0:.0f}")

print(f"\nComparison collection:")
print(f"  Total rows: {comparison_rows:,}")
print(f"  Total size: {comparison_size / (1024**3):.2f} GB")
print(f"  Bytes per row: {comparison_size / comparison_rows if comparison_rows > 0 else 0:.0f}")

if baseline_size > 0 and comparison_size > 0:
    print(f"\nSize reduction: {(1 - comparison_size/baseline_size)*100:.1f}%")