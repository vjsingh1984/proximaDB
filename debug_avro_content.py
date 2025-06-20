#!/usr/bin/env python3
"""
Debug Avro file content
"""

import json

def debug_avro_file(file_path):
    """Debug the content of an Avro file"""
    print(f"🔍 Debugging Avro file: {file_path}")
    
    try:
        from avro.datafile import DataFileReader
        from avro.io import DatumReader
        
        with open(file_path, 'rb') as f:
            print(f"   📊 File size: {f.seek(0, 2)} bytes")
            f.seek(0)
            
            reader = DataFileReader(f, DatumReader())
            
            print(f"   📋 Schema: {reader.meta['avro.schema'].decode()}")
            
            for i, record in enumerate(reader):
                print(f"   📄 Record {i}: {record}")
                
                # Check collection_data JSON if present
                if 'collection_data' in record and record['collection_data']:
                    try:
                        collection_data = json.loads(record['collection_data'])
                        print(f"      ✅ Collection data parsed successfully")
                        print(f"      🏷️ Name: {collection_data.get('name', 'N/A')}")
                        print(f"      🆔 UUID: {collection_data.get('uuid', 'N/A')}")
                    except Exception as e:
                        print(f"      ❌ Failed to parse collection_data JSON: {e}")
                        print(f"      📄 Raw JSON: {record['collection_data'][:200]}...")
                
            reader.close()
            
    except Exception as e:
        print(f"   ❌ Error reading Avro file: {e}")
        print(f"   💡 Trying raw hex dump...")
        try:
            with open(file_path, 'rb') as f:
                data = f.read()
                print(f"   📊 File size: {len(data)} bytes")
                print(f"   🔍 First 200 bytes (hex): {data[:200].hex()}")
                print(f"   🔍 First 200 bytes (ascii): {data[:200]}")
        except Exception as e2:
            print(f"   ❌ Even raw read failed: {e2}")

if __name__ == "__main__":
    import glob
    
    # Find the most recent incremental operation file
    pattern = "/data/proximadb/1/metadata/incremental/op_*.avro"
    files = glob.glob(pattern)
    
    if files:
        latest_file = max(files)
        debug_avro_file(latest_file)
    else:
        print("❌ No incremental operation files found")
        
        # Check archive
        pattern = "/data/proximadb/1/metadata/archive/*/incremental/op_*.avro"
        archive_files = glob.glob(pattern)
        if archive_files:
            print("📁 Found files in archive:")
            for f in archive_files:
                print(f"   📄 {f}")
                debug_avro_file(f)
                break