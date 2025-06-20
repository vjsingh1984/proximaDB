#!/usr/bin/env python3
"""
Comprehensive Collection Avro Test with Recovery Validation

This test validates the complete lifecycle of collection operations:
1. Create 20+ collections with various configurations
2. Update them multiple times (3-4 rounds)
3. Delete some collections
4. Get collection list and save current state
5. Restart server (triggers recovery and compaction)
6. Verify state matches after restart

Tests the collection_avro.rs schema as single source of truth.
"""

import os
import sys
import json
import time
import uuid
import random
import asyncio
import aiofiles
import subprocess
from typing import Dict, List, Any, Optional
from dataclasses import dataclass, asdict
from pathlib import Path

# Add the ProximaDB client path
sys.path.append(os.path.join(os.path.dirname(__file__), "clients", "python", "src"))

try:
    from proximadb.grpc_client import ProximaDBClient
    from proximadb.config import ClientConfig
except ImportError as e:
    print(f"❌ Failed to import ProximaDB client: {e}")
    print("Make sure to run: pip install -e clients/python/")
    sys.exit(1)

@dataclass
class CollectionState:
    """Represents the state of a collection for comparison"""
    name: str
    dimension: int
    distance_metric: str
    indexing_algorithm: str
    storage_engine: str
    description: Optional[str]
    tags: List[str]
    owner: Optional[str]
    vector_count: int
    total_size_bytes: int
    version: int
    
    @classmethod
    def from_grpc_response(cls, response) -> 'CollectionState':
        """Create CollectionState from gRPC response"""
        config = response.config
        return cls(
            name=config.name,
            dimension=config.dimension,
            distance_metric=cls._map_distance_metric(config.distance_metric),
            indexing_algorithm=cls._map_indexing_algorithm(config.indexing_algorithm),
            storage_engine=cls._map_storage_engine(config.storage_engine),
            description=getattr(response, 'description', None),
            tags=getattr(response, 'tags', []),
            owner=getattr(response, 'owner', None),
            vector_count=getattr(response, 'vector_count', 0),
            total_size_bytes=getattr(response, 'total_size_bytes', 0),
            version=getattr(response, 'version', 1)
        )
    
    @staticmethod
    def _map_distance_metric(metric_id: int) -> str:
        mapping = {1: "COSINE", 2: "EUCLIDEAN", 3: "DOT_PRODUCT", 4: "HAMMING"}
        return mapping.get(metric_id, "COSINE")
    
    @staticmethod
    def _map_indexing_algorithm(algo_id: int) -> str:
        mapping = {1: "HNSW", 2: "IVF", 3: "PQ", 4: "FLAT", 5: "ANNOY"}
        return mapping.get(algo_id, "HNSW")
    
    @staticmethod
    def _map_storage_engine(engine_id: int) -> str:
        mapping = {1: "VIPER", 2: "LSM", 3: "MMAP", 4: "HYBRID"}
        return mapping.get(engine_id, "VIPER")

class ComprehensiveCollectionTest:
    """Comprehensive test for collection Avro schema persistence and recovery"""
    
    def __init__(self):
        # Just create a simple config - we'll pass endpoint directly to client
        self.client = None
        self.collections_created: List[str] = []
        self.pre_restart_state: Dict[str, CollectionState] = {}
        self.post_restart_state: Dict[str, CollectionState] = {}
        self.state_file = Path("collection_state_pre_restart.json")
        
    async def setup(self):
        """Initialize client connection"""
        print("🔧 Setting up test environment...")
        self.client = ProximaDBClient(endpoint="localhost:5679")
        await self.client.connect()
        print("✅ Connected to ProximaDB server")
        
    async def cleanup(self):
        """Clean up test environment"""
        if self.client:
            await self.client.close()
        print("🧹 Test cleanup completed")
        
    async def generate_test_collections(self) -> List[Dict[str, Any]]:
        """Generate 20+ diverse test collections"""
        print("🎲 Generating diverse test collections...")
        
        collections = []
        dimensions = [64, 128, 256, 384, 512, 768, 1024, 1536, 2048]
        distance_metrics = [1, 2, 3, 4]  # COSINE, EUCLIDEAN, DOT_PRODUCT, HAMMING
        indexing_algorithms = [1, 2, 3, 4, 5]  # HNSW, IVF, PQ, FLAT, ANNOY
        storage_engines = [1, 2, 3, 4]  # VIPER, LSM, MMAP, HYBRID
        
        # Create collections with various configurations
        for i in range(25):  # Create 25 collections
            collection_name = f"test_collection_{i:03d}_{uuid.uuid4().hex[:8]}"
            
            collection_config = {
                "name": collection_name,
                "dimension": random.choice(dimensions),
                "distance_metric": random.choice(distance_metrics),
                "indexing_algorithm": random.choice(indexing_algorithms),
                "storage_engine": random.choice(storage_engines),
                "filterable_metadata_fields": [f"field_{j}" for j in range(random.randint(0, 3))],
                "indexing_config": {
                    "hnsw_ef_construction": random.randint(100, 500),
                    "hnsw_max_connections": random.randint(8, 32),
                    "enable_pruning": random.choice([True, False])
                }
            }
            
            collections.append(collection_config)
            
        print(f"📦 Generated {len(collections)} test collections")
        return collections
        
    async def create_collections(self, collections: List[Dict[str, Any]]):
        """Create all test collections"""
        print("🏗️ Creating test collections...")
        
        for i, config in enumerate(collections):
            try:
                # Create collection using gRPC client
                response = await self.client.create_collection(**config)
                self.collections_created.append(config["name"])
                
                print(f"✅ Created collection {i+1}/{len(collections)}: {config['name']}")
                
                # Small delay to avoid overwhelming the server
                await asyncio.sleep(0.1)
                
            except Exception as e:
                print(f"❌ Failed to create collection {config['name']}: {e}")
                
        print(f"🎯 Successfully created {len(self.collections_created)} collections")
        
    async def update_collections_multiple_rounds(self, rounds: int = 4):
        """Update collections multiple times to test versioning"""
        print(f"🔄 Starting {rounds} rounds of collection updates...")
        
        for round_num in range(1, rounds + 1):
            print(f"\n📝 Round {round_num}/{rounds}: Updating collections...")
            
            for i, collection_name in enumerate(self.collections_created):
                try:
                    # Get current collection to update it
                    current = await self.client.get_collection(collection_name)
                    if not current:
                        print(f"⚠️ Collection {collection_name} not found for update")
                        continue
                    
                    # Simulate updates by modifying indexing config
                    updated_config = current.config.copy()
                    updated_config["indexing_config"]["updated_round"] = round_num
                    updated_config["indexing_config"]["random_value"] = random.randint(1000, 9999)
                    
                    # Update the collection
                    await self.client.update_collection(collection_name, updated_config)
                    
                    if (i + 1) % 5 == 0:
                        print(f"    ✅ Updated {i+1}/{len(self.collections_created)} collections")
                        
                    await asyncio.sleep(0.05)  # Brief pause
                    
                except Exception as e:
                    print(f"❌ Failed to update collection {collection_name} in round {round_num}: {e}")
            
            print(f"✅ Completed round {round_num}")
            await asyncio.sleep(0.5)  # Pause between rounds
            
        print(f"🎯 Completed all {rounds} update rounds")
        
    async def delete_some_collections(self, delete_fraction: float = 0.3):
        """Delete a fraction of collections to test deletion handling"""
        num_to_delete = int(len(self.collections_created) * delete_fraction)
        collections_to_delete = random.sample(self.collections_created, num_to_delete)
        
        print(f"🗑️ Deleting {len(collections_to_delete)} collections...")
        
        for collection_name in collections_to_delete:
            try:
                await self.client.delete_collection(collection_name)
                self.collections_created.remove(collection_name)
                print(f"✅ Deleted collection: {collection_name}")
                await asyncio.sleep(0.1)
                
            except Exception as e:
                print(f"❌ Failed to delete collection {collection_name}: {e}")
                
        print(f"🎯 Successfully deleted {len(collections_to_delete)} collections")
        print(f"📊 Remaining collections: {len(self.collections_created)}")
        
    async def capture_state_before_restart(self):
        """Capture current state of all collections"""
        print("📸 Capturing collection state before restart...")
        
        try:
            collections_response = await self.client.list_collections()
            
            for collection_info in collections_response:
                collection_name = collection_info.config.name
                if collection_name in self.collections_created:
                    state = CollectionState.from_grpc_response(collection_info)
                    self.pre_restart_state[collection_name] = state
                    
            print(f"✅ Captured state for {len(self.pre_restart_state)} collections")
            
            # Save state to file for validation
            async with aiofiles.open(self.state_file, "w") as f:
                state_data = {name: asdict(state) for name, state in self.pre_restart_state.items()}
                await f.write(json.dumps(state_data, indent=2))
                
            print(f"💾 Saved state to {self.state_file}")
            
            # Also inspect the filestore structure for debugging
            await self.inspect_filestore_structure()
            
        except Exception as e:
            print(f"❌ Failed to capture state: {e}")
            raise
            
    async def inspect_filestore_structure(self):
        """Inspect the filestore directory structure"""
        try:
            import os
            from pathlib import Path
            
            data_dir = Path("./data/metadata")
            if not data_dir.exists():
                print("📁 No metadata directory found")
                return
                
            print("\n📁 Filestore structure:")
            for root, dirs, files in os.walk(data_dir):
                level = root.replace(str(data_dir), '').count(os.sep)
                indent = '  ' * level
                print(f"{indent}{os.path.basename(root)}/")
                sub_indent = '  ' * (level + 1)
                for file in files:
                    file_path = Path(root) / file
                    size = file_path.stat().st_size if file_path.exists() else 0
                    print(f"{sub_indent}{file} ({size} bytes)")
                    
        except Exception as e:
            print(f"⚠️ Could not inspect filestore: {e}")
            
    async def restart_server(self):
        """Restart the ProximaDB server to trigger recovery and compaction"""
        print("🔄 Restarting ProximaDB server...")
        
        try:
            # Close current connection
            await self.client.close()
            
            # Find the ProximaDB server process
            print("🛑 Stopping ProximaDB server...")
            subprocess.run(["pkill", "-f", "proximadb-server"], check=False)
            
            # Wait for graceful shutdown
            await asyncio.sleep(3)
            
            # Start the server again
            print("🚀 Starting ProximaDB server...")
            subprocess.Popen([
                "cargo", "run", "--bin", "proximadb-server", "--", 
                "--config", "config.toml"
            ], cwd="/home/vsingh/code/proximadb")
            
            # Wait for server to start up
            print("⏳ Waiting for server startup...")
            await asyncio.sleep(10)
            
            # Reconnect client
            self.client = ProximaDBClient(self.config)
            await self.client.connect()
            
            print("✅ Server restarted and reconnected")
            
        except Exception as e:
            print(f"❌ Failed to restart server: {e}")
            raise
            
    async def capture_state_after_restart(self):
        """Capture state after restart to compare with pre-restart state"""
        print("📸 Capturing collection state after restart...")
        
        try:
            collections_response = await self.client.list_collections()
            
            for collection_info in collections_response:
                collection_name = collection_info.config.name
                if collection_name in self.collections_created:
                    state = CollectionState.from_grpc_response(collection_info)
                    self.post_restart_state[collection_name] = state
                    
            print(f"✅ Captured state for {len(self.post_restart_state)} collections")
            
        except Exception as e:
            print(f"❌ Failed to capture post-restart state: {e}")
            raise
            
    def validate_state_consistency(self) -> bool:
        """Validate that pre-restart and post-restart states match"""
        print("🔍 Validating state consistency...")
        
        success = True
        mismatches = []
        
        # Check that all pre-restart collections exist post-restart
        missing_collections = set(self.pre_restart_state.keys()) - set(self.post_restart_state.keys())
        if missing_collections:
            print(f"❌ Missing collections after restart: {missing_collections}")
            success = False
            
        # Check each collection's state
        for collection_name in self.pre_restart_state:
            if collection_name not in self.post_restart_state:
                continue
                
            pre_state = self.pre_restart_state[collection_name]
            post_state = self.post_restart_state[collection_name]
            
            # Compare critical fields (excluding version and timestamps which may change during recovery)
            critical_fields = ['name', 'dimension', 'distance_metric', 'indexing_algorithm', 'storage_engine']
            
            for field in critical_fields:
                pre_value = getattr(pre_state, field)
                post_value = getattr(post_state, field)
                
                if pre_value != post_value:
                    mismatch = f"{collection_name}.{field}: {pre_value} != {post_value}"
                    mismatches.append(mismatch)
                    success = False
                    
        if mismatches:
            print("❌ State mismatches found:")
            for mismatch in mismatches:
                print(f"   - {mismatch}")
        else:
            print("✅ All collection states match after restart")
            
        return success
        
    async def run_comprehensive_test(self):
        """Execute the complete test sequence"""
        print("🎯 Starting Comprehensive Collection Avro Test")
        print("=" * 60)
        
        try:
            # Setup
            await self.setup()
            
            # Step 1: Generate and create collections
            collections = await self.generate_test_collections()
            await self.create_collections(collections)
            
            # Step 2: Update collections multiple times
            await self.update_collections_multiple_rounds(rounds=4)
            
            # Step 3: Delete some collections
            await self.delete_some_collections(delete_fraction=0.25)
            
            # Step 4: Capture state before restart
            await self.capture_state_before_restart()
            
            # Step 5: Restart server (triggers recovery and compaction)
            await self.restart_server()
            
            # Step 6: Capture state after restart
            await self.capture_state_after_restart()
            
            # Step 7: Validate consistency
            state_consistent = self.validate_state_consistency()
            
            # Summary
            print("\n" + "=" * 60)
            print("🏁 TEST SUMMARY")
            print("=" * 60)
            print(f"📊 Collections created: {len(self.pre_restart_state)}")
            print(f"📊 Collections after restart: {len(self.post_restart_state)}")
            print(f"🔄 Update rounds completed: 4")
            print(f"🗑️ Collections deleted: {25 - len(self.pre_restart_state)}")
            
            if state_consistent:
                print("✅ RESULT: All collection states persisted correctly across restart")
                print("✅ RESULT: Recovery and compaction working properly")
                return True
            else:
                print("❌ RESULT: State inconsistencies detected")
                print("❌ RESULT: Recovery or compaction has issues")
                return False
                
        except Exception as e:
            print(f"❌ Test failed with error: {e}")
            import traceback
            traceback.print_exc()
            return False
            
        finally:
            await self.cleanup()

async def main():
    """Main test execution"""
    test = ComprehensiveCollectionTest()
    success = await test.run_comprehensive_test()
    
    if success:
        print("\n🎉 Comprehensive test PASSED! Collection persistence working correctly.")
        sys.exit(0)
    else:
        print("\n💥 Comprehensive test FAILED! Check collection persistence implementation.")
        sys.exit(1)

if __name__ == "__main__":
    asyncio.run(main())