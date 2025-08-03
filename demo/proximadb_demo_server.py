#!/usr/bin/env python3
"""
ProximaDB Unified Demo Server

A comprehensive demo server that combines:
- Static UI serving for ProximaDB interaction
- WebSocket support for real-time demo execution
- REST API endpoints for all ProximaDB features
- Live demo runner with streaming results
"""

import asyncio
import json
import os
import sys
import subprocess
from pathlib import Path
from datetime import datetime
from typing import Dict, List, Any, Optional
import uuid
import argparse

from aiohttp import web
import aiohttp_cors
from aiohttp import web_ws

# Add parent directory for imports
sys.path.append(str(Path(__file__).parent))
from utils.demo_logger import DemoLogger


class ProximaDBDemoServer:
    """Unified demo server for ProximaDB"""
    
    def __init__(self, port=8080, proximadb_url="http://localhost:5678", grpc_url="localhost:5679"):
        self.port = port
        self.proximadb_url = proximadb_url
        self.grpc_url = grpc_url
        self.app = web.Application()
        self.runners = {}  # Active demo runners
        self.websockets = set()  # Connected clients
        self.demo_results = {}  # Recent demo results
        self.static_dir = Path(__file__).parent / "static"
        
        # Available demos with categories
        self.demos = {
            "hybrid_search": {
                "name": "Hybrid Search Comprehensive",
                "script": "hybrid_search_comprehensive_demo.py",
                "description": "All 7 search combinations with query planning and BERT embeddings",
                "category": "core",
                "icon": "fa-search-plus",
                "color": "blue"
            },
            "sql": {
                "name": "SQL Comprehensive",
                "script": "sql_comprehensive_demo.py",
                "description": "Complete SQL interface with query optimization and caching",
                "category": "core",
                "icon": "fa-database",
                "color": "green"
            },
            "storage_engines": {
                "name": "Storage Engine Performance",
                "script": "storage_engine_performance_demo.py",
                "description": "SST vs VIPER comprehensive benchmarking across protocols",
                "category": "benchmark",
                "icon": "fa-server",
                "color": "purple"
            },
            "ecommerce": {
                "name": "E-commerce Real World",
                "script": "ecommerce_demo.py",
                "description": "Real product search with BERT embeddings and natural language",
                "category": "core",
                "icon": "fa-shopping-cart",
                "color": "orange"
            },
            "metadata_filtering": {
                "name": "Advanced Metadata Filtering",
                "script": "server_side_metadata_filtering_demo.py",
                "description": "VIPER optimization with complex filtering scenarios",
                "category": "advanced",
                "icon": "fa-filter",
                "color": "indigo"
            },
            "integration_matrix": {
                "name": "Integration Test Matrix",
                "script": "integration_test_matrix.py",
                "description": "Comprehensive component testing across all features",
                "category": "advanced",
                "icon": "fa-th",
                "color": "red"
            },
            "wal_strategies": {
                "name": "WAL Strategies",
                "script": "wal_strategies_comprehensive_demo.py",
                "description": "Write-ahead logging strategies and recovery scenarios",
                "category": "advanced",
                "icon": "fa-hdd",
                "color": "gray"
            },
            "feature_showcase": {
                "name": "Feature Showcase",
                "script": "feature_showcase.py",
                "description": "Interactive demonstration of all ProximaDB features",
                "category": "core",
                "icon": "fa-star",
                "color": "yellow"
            }
        }
        
        # Setup routes
        self.setup_routes()
        
    def setup_routes(self):
        """Setup HTTP routes and WebSocket endpoints"""
        # CORS setup for API calls
        cors = aiohttp_cors.setup(self.app, defaults={
            "*": aiohttp_cors.ResourceOptions(
                allow_credentials=True,
                expose_headers="*",
                allow_headers="*",
                allow_methods="*"
            )
        })
        
        # API routes for demo runner
        self.app.router.add_get('/api/demos', self.get_demos)
        self.app.router.add_post('/api/demos/{demo_id}/run', self.run_demo)
        self.app.router.add_get('/api/demos/{demo_id}/status', self.get_demo_status)
        self.app.router.add_get('/api/demos/{demo_id}/results', self.get_demo_results)
        self.app.router.add_get('/api/demos/recent', self.get_recent_results)
        self.app.router.add_get('/api/demos/results/{filename}', self.get_result_details)
        
        # ProximaDB proxy routes (for UI integration)
        self.app.router.add_get('/api/health', self.health_check)
        self.app.router.add_get('/api/collections', self.list_collections)
        self.app.router.add_post('/api/collections/{collection_id}', self.create_collection)
        self.app.router.add_delete('/api/collections/{collection_id}', self.delete_collection)
        self.app.router.add_post('/api/collections/{collection_id}/search', self.search_vectors)
        self.app.router.add_post('/api/collections/{collection_id}/insert', self.insert_vectors)
        self.app.router.add_post('/api/sql/execute', self.execute_sql)
        
        # WebSocket for live updates
        self.app.router.add_get('/ws', self.websocket_handler)
        
        # Static files (unified UI)
        self.app.router.add_get('/', self.serve_index)
        self.app.router.add_static('/', path=self.static_dir, name='static')
        
        # Apply CORS to all routes
        for route in list(self.app.router.routes()):
            if not isinstance(route.resource, web.StaticResource):
                cors.add(route)
    
    async def serve_index(self, request):
        """Serve the main UI file"""
        index_path = self.static_dir / "proximadb-ui.html"
        if index_path.exists():
            return web.FileResponse(index_path)
        return web.Response(text="UI file not found", status=404)
    
    # Demo API endpoints
    async def get_demos(self, request):
        """Get list of available demos"""
        return web.json_response({
            "success": True,
            "demos": self.demos
        })
    
    async def run_demo(self, request):
        """Run a specific demo"""
        demo_id = request.match_info['demo_id']
        
        if demo_id not in self.demos:
            return web.json_response({
                "success": False,
                "error": f"Demo '{demo_id}' not found"
            }, status=404)
        
        demo = self.demos[demo_id]
        run_id = str(uuid.uuid4())
        
        # Create a runner for this demo
        runner = DemoRunner(
            demo_id=demo_id,
            run_id=run_id,
            demo_config=demo,
            websockets=self.websockets
        )
        
        self.runners[run_id] = runner
        
        # Start the demo asynchronously
        asyncio.create_task(runner.run())
        
        return web.json_response({
            "success": True,
            "run_id": run_id,
            "demo": demo,
            "status": "started"
        })
    
    async def get_demo_status(self, request):
        """Get status of a running demo"""
        demo_id = request.match_info['demo_id']
        run_id = request.query.get('run_id')
        
        if run_id and run_id in self.runners:
            runner = self.runners[run_id]
            return web.json_response({
                "success": True,
                "status": runner.status,
                "progress": runner.progress,
                "current_section": runner.current_section
            })
        
        return web.json_response({
            "success": False,
            "error": "Demo run not found"
        }, status=404)
    
    async def get_demo_results(self, request):
        """Get results of a completed demo"""
        demo_id = request.match_info['demo_id']
        run_id = request.query.get('run_id')
        
        results_file = Path(f"demo_results/{demo_id}_{run_id}.json")
        
        if results_file.exists():
            with open(results_file, 'r') as f:
                results = json.load(f)
            
            return web.json_response({
                "success": True,
                "results": results
            })
        
        return web.json_response({
            "success": False,
            "error": "Results not found"
        }, status=404)
    
    async def get_recent_results(self, request):
        """Get recent demo results"""
        results_dir = Path("demo_results")
        recent_results = []
        
        if results_dir.exists():
            # Get last 20 result files
            result_files = sorted(
                results_dir.glob("*.json"),
                key=lambda p: p.stat().st_mtime,
                reverse=True
            )[:20]
            
            for file in result_files:
                try:
                    with open(file, 'r') as f:
                        data = json.load(f)
                        recent_results.append({
                            "filename": file.name,
                            "demo_name": data.get("demo_name", "Unknown"),
                            "timestamp": data.get("timestamp", ""),
                            "duration": data.get("duration_seconds", 0),
                            "metrics": data.get("metrics", {}),
                            "errors": len(data.get("errors", []))
                        })
                except:
                    continue
        
        return web.json_response({
            "success": True,
            "results": recent_results
        })
    
    async def get_result_details(self, request):
        """Get detailed result from file"""
        filename = request.match_info['filename']
        results_file = Path(f"demo_results/{filename}")
        
        if results_file.exists() and results_file.suffix == '.json':
            with open(results_file, 'r') as f:
                result = json.load(f)
            
            return web.json_response({
                "success": True,
                "result": result
            })
        
        return web.json_response({
            "success": False,
            "error": "Result file not found"
        }, status=404)
    
    # ProximaDB proxy endpoints
    async def health_check(self, request):
        """Check ProximaDB health"""
        try:
            async with self.app['session'].get(f"{self.proximadb_url}/health") as resp:
                data = await resp.json()
                return web.json_response(data)
        except Exception as e:
            return web.json_response({
                "status": "error",
                "error": str(e)
            }, status=500)
    
    async def list_collections(self, request):
        """List collections from ProximaDB"""
        try:
            async with self.app['session'].get(f"{self.proximadb_url}/api/v1/collections") as resp:
                data = await resp.json()
                return web.json_response(data)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def create_collection(self, request):
        """Create a collection in ProximaDB"""
        collection_id = request.match_info['collection_id']
        body = await request.json()
        
        try:
            payload = {
                "operation": "create",
                "collection_id": collection_id,
                "config": body
            }
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/collection",
                json=payload
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def delete_collection(self, request):
        """Delete a collection from ProximaDB"""
        collection_id = request.match_info['collection_id']
        
        try:
            payload = {
                "operation": "delete",
                "collection_id": collection_id
            }
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/collection",
                json=payload
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def search_vectors(self, request):
        """Search vectors in ProximaDB"""
        collection_id = request.match_info['collection_id']
        body = await request.json()
        
        try:
            payload = {
                "collection_id": collection_id,
                **body
            }
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/vector/search",
                json=payload
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def insert_vectors(self, request):
        """Insert vectors into ProximaDB"""
        collection_id = request.match_info['collection_id']
        body = await request.json()
        
        try:
            payload = {
                "operation": "insert",
                "collection_id": collection_id,
                "vectors": body.get("vectors", [])
            }
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/vector/batch",
                json=payload
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def execute_sql(self, request):
        """Execute SQL query in ProximaDB"""
        body = await request.json()
        
        try:
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/sql/execute",
                json=body
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def websocket_handler(self, request):
        """Handle WebSocket connections for live updates"""
        ws = web.WebSocketResponse()
        await ws.prepare(request)
        
        self.websockets.add(ws)
        
        try:
            # Send initial connection message
            await ws.send_json({
                "type": "connected",
                "message": "Connected to ProximaDB demo server"
            })
            
            async for msg in ws:
                if msg.type == web.WSMsgType.TEXT:
                    data = json.loads(msg.data)
                    # Handle client messages if needed
                elif msg.type == web.WSMsgType.ERROR:
                    print(f'WebSocket error: {ws.exception()}')
        finally:
            self.websockets.discard(ws)
        
        return ws
    
    async def broadcast(self, message):
        """Broadcast message to all connected WebSocket clients"""
        if self.websockets:
            await asyncio.gather(
                *[ws.send_json(message) for ws in self.websockets],
                return_exceptions=True
            )
    
    async def startup(self, app):
        """Initialize HTTP session on startup"""
        import aiohttp
        app['session'] = aiohttp.ClientSession()
    
    async def cleanup(self, app):
        """Cleanup on shutdown"""
        await app['session'].close()
    
    def run(self):
        """Start the server"""
        print(f"🚀 ProximaDB Unified Demo Server")
        print(f"📡 Serving on http://localhost:{self.port}")
        print(f"🔗 WebSocket: ws://localhost:{self.port}/ws")
        print(f"🔗 ProximaDB URL: {self.proximadb_url}")
        print(f"📁 Static files: {self.static_dir}")
        print(f"\n✨ Available demos: {', '.join(self.demos.keys())}")
        print("\nPress Ctrl+C to stop")
        
        self.app.on_startup.append(self.startup)
        self.app.on_cleanup.append(self.cleanup)
        
        web.run_app(self.app, host='0.0.0.0', port=self.port)


class DemoRunner:
    """Runs a demo and streams output to WebSocket clients"""
    
    def __init__(self, demo_id, run_id, demo_config, websockets):
        self.demo_id = demo_id
        self.run_id = run_id
        self.demo_config = demo_config
        self.websockets = websockets
        self.status = "pending"
        self.progress = 0
        self.current_section = ""
        self.start_time = None
        self.process = None
        
    async def run(self):
        """Run the demo script and stream output"""
        self.status = "running"
        self.start_time = datetime.now()
        
        script_path = Path(__file__).parent / self.demo_config['script']
        
        # Broadcast start message
        await self.broadcast({
            "type": "demo_started",
            "demo_id": self.demo_id,
            "run_id": self.run_id,
            "demo_name": self.demo_config['name'],
            "timestamp": self.start_time.isoformat()
        })
        
        try:
            # Run the demo script
            self.process = await asyncio.create_subprocess_exec(
                sys.executable,
                str(script_path),
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            
            # Stream output
            while True:
                line = await self.process.stdout.readline()
                if not line:
                    break
                
                text = line.decode('utf-8').strip()
                if text:
                    await self.handle_output_line(text)
            
            # Wait for completion
            await self.process.wait()
            
            if self.process.returncode == 0:
                self.status = "completed"
                await self.broadcast({
                    "type": "demo_completed",
                    "demo_id": self.demo_id,
                    "run_id": self.run_id,
                    "duration": (datetime.now() - self.start_time).total_seconds()
                })
            else:
                self.status = "failed"
                stderr = await self.process.stderr.read()
                await self.broadcast({
                    "type": "demo_failed",
                    "demo_id": self.demo_id,
                    "run_id": self.run_id,
                    "error": stderr.decode('utf-8')
                })
                
        except Exception as e:
            self.status = "error"
            await self.broadcast({
                "type": "demo_error",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "error": str(e)
            })
    
    async def handle_output_line(self, line):
        """Parse and handle demo output line"""
        # Detect section headers
        if line.startswith("=") and len(line) > 20:
            return
        elif "📌" in line or line.startswith("###"):
            self.current_section = line.replace("📌", "").replace("#", "").strip()
            self.progress += 10
            await self.broadcast({
                "type": "section",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "section": self.current_section,
                "progress": min(self.progress, 90)
            })
        
        # Detect metrics
        elif "📊" in line:
            parts = line.split(":")
            if len(parts) >= 2:
                metric_name = parts[0].replace("📊", "").strip()
                metric_value = parts[1].strip()
                await self.broadcast({
                    "type": "metric",
                    "demo_id": self.demo_id,
                    "run_id": self.run_id,
                    "metric": metric_name,
                    "value": metric_value
                })
        
        # Detect success/error
        elif "✅" in line:
            await self.broadcast({
                "type": "success",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "message": line.replace("✅", "").strip()
            })
        elif "❌" in line:
            await self.broadcast({
                "type": "error",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "message": line.replace("❌", "").strip()
            })
        
        # Regular log line
        else:
            await self.broadcast({
                "type": "log",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "message": line,
                "section": self.current_section
            })
    
    async def broadcast(self, message):
        """Broadcast to all WebSocket clients"""
        if self.websockets:
            await asyncio.gather(
                *[ws.send_json(message) for ws in self.websockets],
                return_exceptions=True
            )


def main():
    """Start the unified ProximaDB demo server"""
    parser = argparse.ArgumentParser(description="ProximaDB Unified Demo Server")
    parser.add_argument("--port", type=int, default=8080, help="Server port (default: 8080)")
    parser.add_argument("--proximadb-url", default="http://localhost:5678", 
                       help="ProximaDB REST URL (default: http://localhost:5678)")
    parser.add_argument("--grpc-url", default="localhost:5679",
                       help="ProximaDB gRPC URL (default: localhost:5679)")
    args = parser.parse_args()
    
    server = ProximaDBDemoServer(
        port=args.port,
        proximadb_url=args.proximadb_url,
        grpc_url=args.grpc_url
    )
    server.run()


if __name__ == "__main__":
    main()