#!/usr/bin/env python3
import os
import subprocess
import time
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))
from proximadb import ProximaDBClient

subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
time.sleep(2)

proc = subprocess.Popen(
    ["./target/release/proximadb-server", "--config", "config.toml"],
    stdout=open('/tmp/server.log', 'w'),
    stderr=subprocess.STDOUT,
    cwd="/workspace"
)

time.sleep(3)

try:
    client = ProximaDBClient("http://localhost:5678")
    collections = client.list_collections()
    print(f"Collections type: {type(collections)}")
    print(f"Collections content: {collections}")
    
    if collections:
        print(f"First collection type: {type(collections[0])}")
        print(f"First collection: {collections[0]}")
        
finally:
    proc.terminate()