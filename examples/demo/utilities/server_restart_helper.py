#!/usr/bin/env python3
"""
Server Restart Helper Script
Helps with server restart process and measures recovery time
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import subprocess
import signal
import json
import requests
import grpc
from proximadb import connect_rest, connect_grpc

def find_server_process():
    """Find the ProximaDB server process"""
    try:
        result = subprocess.run(['pgrep', '-f', 'proximadb-server'], 
                              capture_output=True, text=True)
        if result.returncode == 0:
            pids = result.stdout.strip().split('\n')
            return [int(pid) for pid in pids if pid]
        return []
    except:
        return []

def stop_server():
    """Stop the ProximaDB server gracefully"""
    print("🛑 Stopping ProximaDB server...")
    
    pids = find_server_process()
    if not pids:
        print("✅ No server process found")
        return True
    
    for pid in pids:
        try:
            print(f"   Stopping process {pid}...")
            os.kill(pid, signal.SIGTERM)
            time.sleep(1)
            
            # Check if process is still running
            try:
                os.kill(pid, 0)  # Check if process exists
                print(f"   Force killing process {pid}...")
                os.kill(pid, signal.SIGKILL)
                time.sleep(1)
            except ProcessLookupError:
                pass  # Process already terminated
        except Exception as e:
            print(f"   Error stopping process {pid}: {e}")
    
    # Wait and verify all processes are stopped
    time.sleep(2)
    remaining_pids = find_server_process()
    if remaining_pids:
        print(f"❌ Server processes still running: {remaining_pids}")
        return False
    
    print("✅ Server stopped successfully")
    return True

def start_server():
    """Start the ProximaDB server"""
    print("🚀 Starting ProximaDB server...")
    
    # Change to server directory
    server_dir = "/home/vsingh/code/proximaDB"
    os.chdir(server_dir)
    
    # Start server in background
    cmd = ["cargo", "run", "--release", "--bin", "proximadb-server"]
    
    with open("server_restart.log", "w") as log_file:
        process = subprocess.Popen(
            cmd,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            cwd=server_dir
        )
    
    print(f"✅ Server started with PID: {process.pid}")
    return process

def wait_for_server_ready():
    """Wait for server to be ready and measure startup time"""
    print("⏳ Waiting for server to be ready...")
    
    start_time = time.time()
    rest_ready = False
    grpc_ready = False
    
    while not (rest_ready and grpc_ready):
        # Test REST API
        if not rest_ready:
            try:
                response = requests.get("http://localhost:5678/health", timeout=1)
                if response.status_code == 200:
                    rest_ready = True
                    print("✅ REST API ready")
            except:
                pass
        
        # Test gRPC API
        if not grpc_ready:
            try:
                client = connect_grpc("http://localhost:5679")
                # Try a simple operation
                client.get_collection("health_check")
                grpc_ready = True
                print("✅ gRPC API ready")
            except:
                pass
        
        time.sleep(0.5)
        
        # Timeout after 30 seconds
        elapsed = time.time() - start_time
        if elapsed > 30:
            print("❌ Server startup timeout!")
            return None
    
    startup_time = time.time() - start_time
    print(f"✅ Server ready in {startup_time:.2f}s")
    return startup_time

def restart_server():
    """Restart the server and measure recovery time"""
    print("🔄 Restarting ProximaDB server...")
    print("="*60)
    
    # Stop server
    if not stop_server():
        print("❌ Failed to stop server")
        return None
    
    # Wait a moment for cleanup
    time.sleep(2)
    
    # Start server
    process = start_server()
    
    # Wait for server to be ready
    startup_time = wait_for_server_ready()
    
    if startup_time is None:
        print("❌ Server restart failed")
        return None
    
    print(f"✅ Server restart completed in {startup_time:.2f}s")
    return {
        "startup_time_s": startup_time,
        "server_pid": process.pid,
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S")
    }

def main():
    if len(sys.argv) < 2:
        print("Usage: python server_restart_helper.py <action>")
        print("Actions: restart, stop, start, status")
        return
    
    action = sys.argv[1].lower()
    
    if action == "restart":
        result = restart_server()
        if result:
            with open("server_restart_metrics.json", "w") as f:
                json.dump(result, f, indent=2)
            print("📊 Restart metrics saved to server_restart_metrics.json")
    
    elif action == "stop":
        stop_server()
    
    elif action == "start":
        process = start_server()
        startup_time = wait_for_server_ready()
        if startup_time:
            print(f"✅ Server started and ready in {startup_time:.2f}s")
    
    elif action == "status":
        pids = find_server_process()
        if pids:
            print(f"✅ Server running with PIDs: {pids}")
        else:
            print("❌ Server not running")
    
    else:
        print(f"❌ Unknown action: {action}")

if __name__ == "__main__":
    main()