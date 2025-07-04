#!/usr/bin/env python3
"""
Comprehensive Vector Discovery Debugging Tool
Systematically checks each component of the storage and search pipeline
"""

import asyncio
import json
import os
import sys
import subprocess
import time
from pathlib import Path
from typing import List, Dict, Any
import struct

# Add the Python client to the path
sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

from proximadb.grpc_client import ProximaDBClient

class VectorDiscoveryDebugger:
    """Debug tool for vector discovery issues"""
    
    def __init__(self):
        self.client = None
        self.server_process = None
        self.collection_name = "debug_collection"
        self.test_vectors = []
        self.results = {
            "wal_check": {},
            "storage_check": {},
            "metadata_check": {},
            "search_check": {},
            "issues_found": []
        }
    
    async def start_server(self) -> bool:
        """Start ProximaDB server with debug logging"""
        print("🚀 Starting server with debug logging...")
        
        # Set debug environment variables
        env = os.environ.copy()
        env["RUST_LOG"] = "proximadb=debug,proximadb::storage=trace,proximadb::services=trace"
        
        self.server_process = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--config", "test_config.toml"
        ], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
        
        # Wait for server to start
        await asyncio.sleep(10)
        
        if self.server_process.poll() is not None:
            stdout, _ = self.server_process.communicate()
            print("❌ Server failed to start:")
            print(stdout[-2000:])
            return False
            
        print("✅ Server started with debug logging")
        return True
    
    async def setup_client(self) -> bool:
        """Setup gRPC client"""
        try:
            self.client = ProximaDBClient("localhost:5679")
            health = await self.client.health_check()
            print(f"✅ Client connected - Server status: {health.status}")
            return True
        except Exception as e:
            print(f"❌ Client connection failed: {e}")
            return False
    
    async def create_test_collection(self) -> bool:
        """Create a fresh test collection"""
        print(f"\n📦 Creating test collection: {self.collection_name}")
        
        try:
            # Clean up any existing collection
            try:
                await self.client.delete_collection(self.collection_name)
                await asyncio.sleep(1)
            except:
                pass
            
            # Create new collection
            collection = await self.client.create_collection(
                name=self.collection_name,
                dimension=128,  # Smaller for faster testing
                distance_metric=1,  # COSINE
                storage_engine=1,   # VIPER
            )
            
            print(f"✅ Collection created: {collection.name} (ID: {collection.id})")
            
            # Store collection ID for later checks
            self.results["metadata_check"]["collection_id"] = collection.id
            self.results["metadata_check"]["collection_name"] = collection.name
            
            return True
            
        except Exception as e:
            print(f"❌ Collection creation failed: {e}")
            self.results["issues_found"].append(f"Collection creation error: {e}")
            return False
    
    async def insert_test_vectors(self) -> bool:
        """Insert test vectors and monitor the process"""
        print("\n📝 Inserting test vectors with monitoring...")
        
        # Create simple test vectors
        self.test_vectors = []
        for i in range(10):
            self.test_vectors.append({
                "id": f"debug_vec_{i:03d}",
                "vector": [float(i) / 10.0] * 128,  # Simple pattern
                "metadata": {
                    "index": str(i),
                    "category": "debug",
                    "timestamp": str(int(time.time()))
                }
            })
        
        try:
            # Insert vectors
            insert_start = time.time()
            result = self.client.insert_vectors(
                collection_id=self.collection_name,
                vectors=self.test_vectors,
                upsert=False
            )
            insert_time = time.time() - insert_start
            
            print(f"✅ Insertion result: {result.count} vectors in {insert_time:.2f}s")
            
            self.results["wal_check"]["vectors_inserted"] = result.count
            self.results["wal_check"]["insertion_time"] = insert_time
            
            # Wait for potential flush
            print("⏳ Waiting 3s for potential WAL processing...")
            await asyncio.sleep(3)
            
            return True
            
        except Exception as e:
            print(f"❌ Vector insertion failed: {e}")
            self.results["issues_found"].append(f"Insertion error: {e}")
            return False
    
    async def check_wal_files(self) -> bool:
        """Check WAL files directly"""
        print("\n🔍 Checking WAL files...")
        
        # Look for WAL files
        wal_path = Path("/workspace/data/wal")
        if not wal_path.exists():
            print("❌ WAL directory not found!")
            self.results["issues_found"].append("WAL directory missing")
            return False
        
        # Find WAL files for our collection
        wal_files = list(wal_path.glob("**/*.wal"))
        avro_files = list(wal_path.glob("**/*.avro"))
        
        print(f"📁 Found {len(wal_files)} WAL files, {len(avro_files)} Avro files")
        
        self.results["wal_check"]["wal_files"] = len(wal_files)
        self.results["wal_check"]["avro_files"] = len(avro_files)
        
        # Check file sizes
        total_size = 0
        for f in wal_files + avro_files:
            size = f.stat().st_size
            total_size += size
            if size > 0:
                print(f"  • {f.name}: {size:,} bytes")
        
        self.results["wal_check"]["total_size"] = total_size
        
        if total_size == 0:
            print("❌ All WAL files are empty!")
            self.results["issues_found"].append("Empty WAL files")
            return False
        
        return True
    
    async def check_viper_storage(self) -> bool:
        """Check VIPER storage files"""
        print("\n🔍 Checking VIPER storage...")
        
        # Look for Parquet files
        collections_path = Path("/workspace/data/collections")
        if not collections_path.exists():
            print("❌ Collections directory not found!")
            self.results["issues_found"].append("Collections directory missing")
            return False
        
        # Find Parquet files
        parquet_files = list(collections_path.glob("**/*.parquet"))
        print(f"📁 Found {len(parquet_files)} Parquet files")
        
        self.results["storage_check"]["parquet_files"] = len(parquet_files)
        
        # Check for our collection
        collection_path = collections_path / self.collection_name
        if collection_path.exists():
            print(f"✅ Collection directory exists: {collection_path}")
            
            # Check for vector data
            vector_files = list(collection_path.glob("vectors/*.parquet"))
            print(f"  • Vector files: {len(vector_files)}")
            
            self.results["storage_check"]["collection_exists"] = True
            self.results["storage_check"]["vector_files"] = len(vector_files)
            
            if len(vector_files) == 0:
                print("⚠️ No vector Parquet files found - vectors may be in WAL only")
                self.results["issues_found"].append("No flushed vector files")
        else:
            print(f"⚠️ Collection directory not found: {collection_path}")
            self.results["storage_check"]["collection_exists"] = False
            self.results["issues_found"].append("Collection directory missing")
        
        return True
    
    async def check_collection_metadata(self) -> bool:
        """Check collection metadata and vector count"""
        print("\n🔍 Checking collection metadata...")
        
        try:
            # Get collection info
            collection = await self.client.get_collection(self.collection_name)
            
            if collection:
                print(f"✅ Collection found: {collection.name}")
                print(f"  • ID: {collection.id}")
                print(f"  • Vector count: {collection.vector_count}")
                print(f"  • Dimension: {collection.dimension}")
                print(f"  • Storage: {collection.storage_engine}")
                
                self.results["metadata_check"]["vector_count"] = collection.vector_count
                self.results["metadata_check"]["found"] = True
                
                if collection.vector_count == 0:
                    print("❌ Vector count is 0 despite insertion!")
                    self.results["issues_found"].append("Vector count not updated")
                
                return True
            else:
                print("❌ Collection not found in metadata!")
                self.results["metadata_check"]["found"] = False
                self.results["issues_found"].append("Collection not in metadata")
                return False
                
        except Exception as e:
            print(f"❌ Metadata check failed: {e}")
            self.results["issues_found"].append(f"Metadata error: {e}")
            return False
    
    async def trace_search_path(self) -> bool:
        """Trace the search execution path"""
        print("\n🔍 Tracing search path...")
        
        try:
            # Test with simple query
            query_vector = [0.5] * 128
            
            print("📡 Sending search request...")
            search_start = time.time()
            
            results = self.client.search_vectors(
                collection_id=self.collection_name,
                query_vectors=[query_vector],
                top_k=5,
                include_metadata=True,
                include_vectors=False
            )
            
            search_time = time.time() - search_start
            
            print(f"📊 Search completed in {search_time*1000:.1f}ms")
            print(f"  • Results found: {len(results)}")
            
            self.results["search_check"]["search_time_ms"] = search_time * 1000
            self.results["search_check"]["results_count"] = len(results)
            
            if len(results) > 0:
                print("✅ Search found results!")
                for i, result in enumerate(results):
                    print(f"  {i+1}. ID: {result.id}, Score: {result.score:.4f}")
                return True
            else:
                print("❌ Search returned 0 results")
                self.results["issues_found"].append("Zero search results")
                return False
                
        except Exception as e:
            print(f"❌ Search failed: {e}")
            self.results["issues_found"].append(f"Search error: {e}")
            return False
    
    async def check_server_logs(self):
        """Check server logs for errors"""
        print("\n📋 Checking server logs...")
        
        if self.server_process:
            # Read some recent output
            try:
                import select
                import fcntl
                
                # Make stdout non-blocking
                fd = self.server_process.stdout.fileno()
                fl = fcntl.fcntl(fd, fcntl.F_GETFL)
                fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)
                
                # Read available output
                logs = []
                while True:
                    ready, _, _ = select.select([self.server_process.stdout], [], [], 0.1)
                    if ready:
                        line = self.server_process.stdout.readline()
                        if line:
                            logs.append(line.strip())
                            if len(logs) > 100:  # Keep last 100 lines
                                logs.pop(0)
                    else:
                        break
                
                # Look for errors
                errors = [l for l in logs if "ERROR" in l or "WARN" in l]
                if errors:
                    print("⚠️ Found errors in logs:")
                    for err in errors[-10:]:  # Last 10 errors
                        print(f"  • {err}")
                    self.results["issues_found"].append(f"Server errors: {len(errors)}")
                else:
                    print("✅ No errors in recent logs")
                    
            except Exception as e:
                print(f"⚠️ Could not read server logs: {e}")
    
    def generate_diagnosis(self):
        """Generate comprehensive diagnosis"""
        print("\n" + "="*60)
        print("🔬 VECTOR DISCOVERY DIAGNOSIS REPORT")
        print("="*60)
        
        print("\n📊 Summary:")
        print(f"  • Vectors inserted: {self.results['wal_check'].get('vectors_inserted', 0)}")
        print(f"  • WAL files: {self.results['wal_check'].get('wal_files', 0)}")
        print(f"  • Parquet files: {self.results['storage_check'].get('parquet_files', 0)}")
        print(f"  • Collection vector count: {self.results['metadata_check'].get('vector_count', 0)}")
        print(f"  • Search results: {self.results['search_check'].get('results_count', 0)}")
        
        print("\n🔍 Issues Found:")
        if self.results["issues_found"]:
            for issue in self.results["issues_found"]:
                print(f"  ❌ {issue}")
        else:
            print("  ✅ No issues detected")
        
        print("\n💡 Diagnosis:")
        
        # Analyze the pattern
        if self.results["wal_check"].get("vectors_inserted", 0) > 0:
            if self.results["metadata_check"].get("vector_count", 0) == 0:
                print("  🔴 CRITICAL: Vectors inserted but metadata not updated")
                print("     → Collection service not updating vector count")
                print("     → Possible issue in insert_vectors → collection metadata update")
            
            if self.results["storage_check"].get("vector_files", 0) == 0:
                print("  🟡 WARNING: No Parquet files found")
                print("     → Vectors still in WAL, not flushed to VIPER")
                print("     → WAL→VIPER flush coordination may be disabled")
            
            if self.results["search_check"].get("results_count", 0) == 0:
                print("  🔴 CRITICAL: Search returning zero results")
                print("     → Polymorphic search not checking WAL")
                print("     → Search engine factory not finding vectors")
        
        print("\n🔧 Recommended Fixes:")
        print("  1. Check collection metadata update in insert_vectors")
        print("  2. Verify WAL search is included in polymorphic search")
        print("  3. Check flush coordination configuration")
        print("  4. Validate search engine selection logic")
    
    async def cleanup(self):
        """Clean up resources"""
        if self.client:
            await self.client.close()
        
        if self.server_process:
            self.server_process.terminate()
            try:
                await asyncio.wait_for(wait_for_process(self.server_process), timeout=5.0)
            except asyncio.TimeoutError:
                self.server_process.kill()

async def wait_for_process(process):
    while process.poll() is None:
        await asyncio.sleep(0.1)

async def main():
    print("🔍 VECTOR DISCOVERY DEBUGGING TOOL")
    print("="*40)
    
    debugger = VectorDiscoveryDebugger()
    
    try:
        # Run systematic checks
        if not await debugger.start_server():
            return 1
        
        if not await debugger.setup_client():
            return 1
        
        if not await debugger.create_test_collection():
            return 1
        
        if not await debugger.insert_test_vectors():
            return 1
        
        # Run all diagnostic checks
        await debugger.check_wal_files()
        await debugger.check_viper_storage()
        await debugger.check_collection_metadata()
        await debugger.trace_search_path()
        await debugger.check_server_logs()
        
        # Generate diagnosis
        debugger.generate_diagnosis()
        
        # Save results
        with open("vector_discovery_diagnosis.json", "w") as f:
            json.dump(debugger.results, f, indent=2)
        print("\n💾 Full diagnosis saved to: vector_discovery_diagnosis.json")
        
        return 0
        
    except Exception as e:
        print(f"💥 Debugging failed: {e}")
        import traceback
        traceback.print_exc()
        return 1
    finally:
        await debugger.cleanup()

if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)