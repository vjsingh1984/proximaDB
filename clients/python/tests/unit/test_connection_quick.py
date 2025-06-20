#!/usr/bin/env python3
import asyncio
import subprocess
import sys
import os
import grpc

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

async def test_connection():
    print("Starting server...")
    
    # Start server
    os.chdir("/home/vsingh/code/proximadb")
    cmd = ["cargo", "run", "--bin", "proximadb-server"]
    server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    
    try:
        # Wait for server startup
        await asyncio.sleep(15)
        
        # Test connection
        channel = grpc.aio.insecure_channel("localhost:5679")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        health_request = proximadb_pb2.HealthRequest()
        health_response = await stub.Health(health_request)
        print(f"Health check: {health_response.status}")
        
        await channel.close()
        print("Connection successful!")
        return True
        
    except Exception as e:
        print(f"Connection failed: {e}")
        return False
    finally:
        server_process.terminate()
        server_process.wait()
        print("Server stopped")

if __name__ == "__main__":
    success = asyncio.run(test_connection())
    sys.exit(0 if success else 1)