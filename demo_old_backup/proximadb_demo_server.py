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
import time
from pathlib import Path
from datetime import datetime
from typing import Dict, List, Any, Optional
import uuid
import argparse
import logging

from aiohttp import web
import aiohttp
import aiohttp_cors
from aiohttp import web_ws

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Add parent directory for imports
sys.path.append(str(Path(__file__).parent))
from utils.demo_logger import DemoLogger
from utils.path_utils import setup_demo_environment

# Setup environment
env_info = setup_demo_environment()

# Import embedding service
try:
    from embedding_service import get_embedding_service, ProximaDBEmbeddingService
    EMBEDDING_SERVICE_AVAILABLE = True
    logger.info("✅ Embedding service module imported successfully")
except ImportError as e:
    EMBEDDING_SERVICE_AVAILABLE = False
    logger.warning(f"⚠️ Embedding service not available: {e}")
    logger.warning("Install sentence-transformers for real BERT embeddings: pip install sentence-transformers torch")


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
        
        # Initialize embedding service
        self.embedding_service = None
        if EMBEDDING_SERVICE_AVAILABLE:
            try:
                logger.info("Initializing BERT embedding service...")
                self.embedding_service = get_embedding_service(model_name="all-mpnet-base-v2", seed=42)
                model_info = self.embedding_service.get_model_info()
                logger.info(f"✅ BERT Embedding service initialized successfully")
                logger.info(f"   Model: {model_info['model_name']} ({model_info['dimension']}D)")
                logger.info(f"   Device: {model_info['device']}")
                logger.info(f"   Seed: {model_info['seed']}")
            except Exception as e:
                logger.error(f"❌ Failed to initialize embedding service: {e}")
                logger.error(f"   Working directory: {os.getcwd()}")
                logger.error(f"   Cache directory attempt: {os.getenv('EMBEDDING_CACHE_DIR', '/app/demo/embedding_cache')}")
                logger.error("   This will disable BERT embedding features in the UI")
                self.embedding_service = None
        else:
            logger.warning("Embedding service module not available - BERT features disabled")
            self.embedding_service = None
        
        # Available demos with categories (updated for consolidated structure)
        self.demos = {
            "feature_showcase": {
                "name": "Feature Showcase Consolidated",
                "script": "feature_showcase_consolidated.py",
                "description": "Comprehensive demonstration of all ProximaDB features including storage engines, SQL, and workflows",
                "category": "core",
                "icon": "fa-star",
                "color": "blue"
            },
            "integration_matrix": {
                "name": "Integration Test Matrix",
                "script": "integration_test_matrix.py",
                "description": "Complete integration testing across all protocols and storage engines",
                "category": "core",
                "icon": "fa-check-circle",
                "color": "green"
            },
            "performance_suite": {
                "name": "Performance Benchmark Suite",
                "script": "benchmarks/performance_suite.py",
                "description": "Comprehensive performance benchmarking with detailed metrics",
                "category": "benchmark",
                "icon": "fa-tachometer-alt",
                "color": "purple"
            },
            "ecommerce": {
                "name": "E-commerce Real World",
                "script": "specialized/ecommerce_demo.py",
                "description": "Real product search with BERT embeddings and natural language",
                "category": "specialized",
                "icon": "fa-shopping-cart",
                "color": "orange"
            },
            "metadata_filtering": {
                "name": "Advanced Metadata Filtering",
                "script": "specialized/server_side_metadata_filtering_demo.py",
                "description": "VIPER optimization with complex filtering scenarios",
                "category": "specialized",
                "icon": "fa-filter",
                "color": "indigo"
            },
            "compression_benchmark": {
                "name": "Compression Benchmark",
                "script": "benchmarks/compression_benchmark.py",
                "description": "REST/gRPC compression performance testing with bandwidth analysis",
                "category": "benchmark",
                "icon": "fa-compress",
                "color": "teal"
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
        self.app.router.add_get('/api/v1/collections', self.list_collections)  # UI compatibility
        self.app.router.add_get('/api/v1/collection/{collection_id}', self.get_collection_details)  # UI compatibility
        self.app.router.add_post('/api/v1/collection', self.create_collection_v1)  # UI compatibility
        self.app.router.add_delete('/api/v1/collection/{collection_id}', self.delete_collection)  # UI compatibility
        self.app.router.add_post('/api/v1/vector/batch', self.insert_vectors_batch)  # UI compatibility
        self.app.router.add_post('/api/v1/vector/search', self.search_vectors_v1)  # UI compatibility
        self.app.router.add_post('/api/v1/sql/execute', self.execute_sql)  # UI compatibility
        self.app.router.add_post('/api/collections/{collection_id}', self.create_collection)
        self.app.router.add_delete('/api/collections/{collection_id}', self.delete_collection)
        self.app.router.add_post('/api/collections/{collection_id}/search', self.search_vectors)
        self.app.router.add_post('/api/collections/{collection_id}/insert', self.insert_vectors)
        self.app.router.add_post('/api/sql/execute', self.execute_sql)
        
        # Embedding API endpoints
        self.app.router.add_post('/api/embeddings/embed', self.embed_text)
        self.app.router.add_post('/api/embeddings/chunk', self.chunk_and_embed)
        self.app.router.add_post('/api/embeddings/search', self.search_similar)
        self.app.router.add_get('/api/embeddings/models', self.get_embedding_models)
        self.app.router.add_get('/api/embeddings/info', self.get_embedding_info)
        # New unified endpoints for seamless generation + insert
        self.app.router.add_post('/api/embeddings/ingest', self.embed_and_ingest)
        self.app.router.add_post('/api/embeddings/ingest-document', self.chunk_embed_and_ingest)
        # Unified search endpoint for text-to-results flow
        self.app.router.add_post('/api/embeddings/search-text', self.search_by_text)
        # Direct vector search endpoint (no embedding, just numerical vectors)
        self.app.router.add_post('/api/vector/search', self.search_vectors_direct)
        
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
            # Forward headers from original request
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            async with self.app['session'].get(f"{self.proximadb_url}/health", headers=headers) as resp:
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
            # Forward headers from original request
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            async with self.app['session'].get(f"{self.proximadb_url}/api/v1/collections", headers=headers) as resp:
                data = await resp.json()
                return web.json_response(data)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def get_collection_details(self, request):
        """Get details of a specific collection from ProximaDB"""
        collection_id = request.match_info['collection_id']
        
        try:
            # Forward headers from original request
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            async with self.app['session'].get(
                f"{self.proximadb_url}/api/v1/collection/{collection_id}", 
                headers=headers
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def create_collection_v1(self, request):
        """Create a collection in ProximaDB (v1 API)"""
        body = await request.json()
        
        try:
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/collection",
                json=body,
                headers=headers
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def insert_vectors_batch(self, request):
        """Insert vectors in batch (v1 API)"""
        body = await request.json()
        
        try:
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/vector/batch",
                json=body,
                headers=headers
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def search_vectors_v1(self, request):
        """Search vectors (v1 API)"""
        body = await request.json()
        
        try:
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/vector/search",
                json=body,
                headers=headers
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
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
            
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/collection",
                json=payload,
                headers=headers
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
            
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/collection",
                json=payload,
                headers=headers
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
            
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/vector/search",
                json=payload,
                headers=headers
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
            
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/vector/batch",
                json=payload,
                headers=headers
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
            # Forward headers from original request and ensure Content-Type
            headers = dict(request.headers)
            headers.pop('Host', None)  # Remove host header
            headers['Content-Type'] = 'application/json'  # Ensure JSON content type
            
            async with self.app['session'].post(
                f"{self.proximadb_url}/api/v1/sql/execute",
                json=body,
                headers=headers
            ) as resp:
                data = await resp.json()
                return web.json_response(data, status=resp.status)
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def embed_text(self, request):
        """Generate embedding for text using BERT"""
        if not self.embedding_service:
            logger.warning("Embedding service not available for embed request")
            return web.json_response({
                "error": "Embedding service not available",
                "suggestion": "Install sentence-transformers: pip install sentence-transformers torch"
            }, status=503)
        
        try:
            body = await request.json()
            text = body.get('text', '')
            model = body.get('model', 'all-MiniLM-L6-v2')
            
            if not text:
                logger.warning("Empty text provided for embedding")
                return web.json_response({"error": "Text is required"}, status=400)
            
            logger.debug(f"Generating embedding for text of length {len(text)}")
            
            # Generate embedding
            embedding = self.embedding_service.embed_text(text)
            
            logger.info(f"Successfully generated {len(embedding)}D embedding for text")
            
            return web.json_response({
                "success": True,
                "embedding": embedding.tolist(),
                "dimension": self.embedding_service.dimension,
                "model": self.embedding_service.model_name,
                "text_length": len(text),
                "seed": self.embedding_service.seed
            })
            
        except Exception as e:
            logger.error(f"Error generating embedding: {e}")
            return web.json_response({"error": str(e)}, status=500)
    
    async def chunk_and_embed(self, request):
        """Chunk text and generate embeddings for each chunk"""
        if not self.embedding_service:
            return web.json_response({
                "error": "Embedding service not available",
                "suggestion": "Install sentence-transformers: pip install sentence-transformers torch"
            }, status=503)
        
        try:
            body = await request.json()
            text = body.get('text', '')
            strategy = body.get('strategy', 'sliding_window')
            chunk_size = body.get('chunk_size', 512)
            overlap = body.get('overlap', 128)
            model = body.get('model', 'all-MiniLM-L6-v2')
            document_id = body.get('document_id', 'doc')
            
            if not text:
                return web.json_response({"error": "Text is required"}, status=400)
            
            # Chunk and embed
            chunks = self.embedding_service.chunk_and_embed(
                text=text,
                strategy=strategy,
                chunk_size=chunk_size,
                overlap=overlap,
                document_id=document_id
            )
            
            return web.json_response({
                "success": True,
                "chunks": chunks,
                "total_chunks": len(chunks),
                "model": self.embedding_service.model_name,
                "dimension": self.embedding_service.dimension,
                "chunking_strategy": strategy,
                "chunk_size": chunk_size,
                "overlap": overlap,
                "seed": self.embedding_service.seed
            })
            
        except Exception as e:
            return web.json_response({"error": str(e)}, status=500)
    
    async def search_similar(self, request):
        """Search for similar chunks using cosine similarity"""
        if not self.embedding_service:
            return web.json_response({
                "error": "Embedding service not available",
                "suggestion": "Install sentence-transformers: pip install sentence-transformers torch"
            }, status=503)
        
        try:
            body = await request.json()
            query = body.get('query', '')
            chunks = body.get('chunks', [])
            top_k = body.get('top_k', 5)
            
            if not query:
                return web.json_response({"error": "Query is required"}, status=400)
            
            if not chunks:
                return web.json_response({"error": "Chunks are required"}, status=400)
            
            # Search similar chunks
            results = self.embedding_service.search_similar_chunks(query, chunks, top_k)
            
            return web.json_response({
                "success": True,
                "query": query,
                "results": results,
                "total_results": len(results),
                "model": self.embedding_service.model_name,
                "top_k": top_k
            })
            
        except Exception as e:
            return web.json_response({"error": str(e)}, status=500)
    
    async def get_embedding_models(self, request):
        """Get list of available embedding models"""
        models = {
            "all-MiniLM-L6-v2": {
                "dimension": 384,
                "description": "Fast, lightweight, good quality",
                "use_case": "general_purpose",
                "speed": "fast"
            },
            "all-mpnet-base-v2": {
                "dimension": 768,
                "description": "Best quality, slower",
                "use_case": "high_accuracy",
                "speed": "slow"
            },
            "all-MiniLM-L12-v2": {
                "dimension": 384,
                "description": "Balanced speed/quality",
                "use_case": "balanced",
                "speed": "medium"
            }
        }
        
        return web.json_response({
            "success": True,
            "models": models,
            "current_model": self.embedding_service.model_name if self.embedding_service else None,
            "service_available": self.embedding_service is not None
        })
    
    async def get_embedding_info(self, request):
        """Get current embedding service information"""
        logger.debug("Handling /api/embeddings/info request")
        
        if not self.embedding_service:
            logger.warning("Embedding service not available for info request")
            return web.json_response({
                "success": False,
                "error": "Embedding service not available",
                "suggestion": "Install sentence-transformers: pip install sentence-transformers torch",
                "info": {
                    "available": False,
                    "model_name": None,
                    "dimension": None,
                    "seed": None,
                    "device": None,
                    "description": "Service not initialized"
                },
                "chunking_strategies": [],
                "proximadb_sdk_available": False
            }, status=503)
        
        try:
            info = self.embedding_service.get_model_info()
            logger.debug(f"Embedding service info retrieved: {info}")
            
            return web.json_response({
                "success": True,
                "info": info,
                "chunking_strategies": [
                    "sentence", "paragraph", "sliding_window", 
                    "semantic", "fixed_size", "recursive"
                ],
                "proximadb_sdk_available": True  # We know it's available since we're using it
            })
        except Exception as e:
            logger.error(f"Error getting embedding service info: {e}")
            return web.json_response({
                "success": False,
                "error": str(e),
                "info": {
                    "available": False,
                    "model_name": None,
                    "dimension": None,
                    "seed": None,
                    "device": None,
                    "description": "Error retrieving info"
                }
            }, status=500)
    
    async def embed_and_ingest(self, request):
        """Unified endpoint: Generate embedding and insert into ProximaDB in one step"""
        if not self.embedding_service:
            return web.json_response({
                "success": False,
                "error": "Embedding service not available"
            })
        
        try:
            data = await request.json()
            text = data.get('text', '').strip()
            collection_id = data.get('collection_id')
            vector_id = data.get('vector_id')
            additional_metadata = data.get('metadata', {})
            
            if not text:
                return web.json_response({
                    "success": False,
                    "error": "Text is required"
                })
            
            if not collection_id:
                return web.json_response({
                    "success": False,
                    "error": "collection_id is required"
                })
            
            # Generate embedding
            embedding = self.embedding_service.embed_text(text)
            
            # Prepare metadata with text and embedding info
            metadata = {
                "text": text,
                "embedding_model": self.embedding_service.model_name,
                "embedding_dimension": self.embedding_service.dimension,
                "created_at": datetime.utcnow().isoformat() + "Z",
                "content_type": "single_text",
                **additional_metadata
            }
            
            # Generate vector ID if not provided
            if not vector_id:
                import hashlib
                text_hash = hashlib.md5(text.encode('utf-8')).hexdigest()[:8]
                vector_id = f"embed_{text_hash}_{int(time.time())}"
            
            # Insert into ProximaDB using SDK with compression disabled for colocated services
            try:
                from proximadb import ProximaDBClient, ClientConfig, CompressionConfig, Protocol
                
                # Create client with compression disabled for both REST and gRPC
                config = ClientConfig(
                    url=self.proximadb_url,
                    compression=CompressionConfig(
                        rest_enabled=False,  # Disabled for colocated services
                        grpc_enabled=False,  # Disabled for colocated services
                        rest_algorithm="gzip",
                        grpc_algorithm="gzip"
                    )
                )
                
                # Use gRPC for better performance (port 5679)
                grpc_url = self.proximadb_url.replace(':5678', ':5679')
                client = ProximaDBClient(url=grpc_url, protocol=Protocol.GRPC, config=config)
                
                # Insert vector
                result = client.insert_vector(
                    collection_id=collection_id,
                    vector_id=vector_id,
                    vector=embedding.tolist(),
                    metadata=metadata
                )
                
                return web.json_response({
                    "success": True,
                    "vector_id": vector_id,
                    "collection_id": collection_id,
                    "embedding_dimension": len(embedding),
                    "metadata_keys": list(metadata.keys()),
                    "text_length": len(text),
                    "model_used": self.embedding_service.model_name,
                    "protocol_used": "gRPC"
                })
                
            except Exception as e:
                logger.error(f"ProximaDB insertion failed: {e}")
                return web.json_response({
                    "success": False,
                    "error": f"ProximaDB insertion failed: {str(e)}"
                }, status=500)
                        
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def chunk_embed_and_ingest(self, request):
        """Unified endpoint: Chunk document, generate embeddings, and batch insert into ProximaDB"""
        if not self.embedding_service:
            return web.json_response({
                "success": False,
                "error": "Embedding service not available"
            })
        
        try:
            data = await request.json()
            text = data.get('text', '').strip()
            collection_id = data.get('collection_id')
            document_id = data.get('document_id')
            strategy = data.get('strategy', 'sliding_window')
            chunk_size = data.get('chunk_size', 512)
            overlap = data.get('overlap', 128)
            additional_metadata = data.get('metadata', {})
            
            if not text:
                return web.json_response({
                    "success": False,
                    "error": "Text is required"
                })
            
            if not collection_id:
                return web.json_response({
                    "success": False,
                    "error": "collection_id is required"
                })
            
            # Generate document ID if not provided
            if not document_id:
                import hashlib
                text_hash = hashlib.md5(text.encode('utf-8')).hexdigest()[:8]
                document_id = f"doc_{text_hash}_{int(time.time())}"
            
            # Generate chunks with embeddings
            base_metadata = {
                "document_id": document_id,
                "content_type": "document_chunk",
                "chunking_strategy": strategy,
                "chunk_size": chunk_size,
                "overlap": overlap,
                "created_at": datetime.utcnow().isoformat() + "Z",
                **additional_metadata
            }
            
            # Log the request details
            logger.info(f"📥 chunk_embed_and_ingest request:")
            logger.info(f"   Text length: {len(text)}")
            logger.info(f"   Collection: {collection_id}")
            logger.info(f"   Document ID: {document_id}")
            logger.info(f"   Strategy: {strategy}, chunk_size: {chunk_size}, overlap: {overlap}")
            
            chunks = self.embedding_service.chunk_and_embed(
                text=text,
                strategy=strategy,
                chunk_size=chunk_size,
                overlap=overlap,
                document_id=document_id,
                metadata=base_metadata
            )
            
            # Log the chunking results
            logger.info(f"📦 Chunking results:")
            logger.info(f"   Number of chunks generated: {len(chunks) if chunks else 0}")
            if chunks:
                for i, chunk in enumerate(chunks[:3]):  # Log first 3 chunks
                    logger.info(f"   Chunk {i}: id={chunk.get('id', 'N/A')}, text_len={len(chunk.get('text', ''))}")
            
            if not chunks:
                logger.error(f"❌ No chunks generated from text of length {len(text)}")
                return web.json_response({
                    "success": False,
                    "error": "No chunks were generated from the text"
                })
            
            # Prepare vector records for ProximaDB batch insert
            vector_records = []
            for chunk in chunks:
                vector_records.append({
                    "id": chunk["id"],
                    "vector": chunk["embedding"],
                    "metadata": chunk["metadata"]
                })
            
            # Batch insert using ProximaDB SDK with compression disabled for colocated services
            try:
                from proximadb import ProximaDBClient, ClientConfig, CompressionConfig, Protocol
                
                # Create client with compression disabled for both REST and gRPC
                config = ClientConfig(
                    url=self.proximadb_url,
                    compression=CompressionConfig(
                        rest_enabled=False,  # Disabled for colocated services
                        grpc_enabled=False,  # Disabled for colocated services
                        rest_algorithm="gzip",
                        grpc_algorithm="gzip"
                    )
                )
                
                # Use gRPC for better performance (port 5679)
                grpc_url = self.proximadb_url.replace(':5678', ':5679')
                client = ProximaDBClient(url=grpc_url, protocol=Protocol.GRPC, config=config)
                
                # Prepare vectors for batch insert
                vectors = []
                ids = []
                metadatas = []
                
                for record in vector_records:
                    vectors.append(record["vector"])
                    ids.append(record["id"])
                    metadatas.append(record["metadata"])
                
                # Batch insert
                logger.info(f"📨 Inserting {len(vector_records)} vectors into collection {collection_id}")
                result = client.insert_vectors(
                    collection_id=collection_id,
                    vectors=vectors,
                    ids=ids,
                    metadata=metadatas
                )
                
                # Log successful insertion
                logger.info(f"✅ Successfully inserted {len(vector_records)} chunks")
                logger.info(f"   Vector IDs: {ids[:3]}...")  # Log first 3 IDs
                
                response_data = {
                    "success": True,
                    "collection_id": collection_id,
                    "document_id": document_id,
                    "chunks_inserted": len(vector_records),
                    "vector_ids": [r["id"] for r in vector_records],
                    "chunking_strategy": strategy,
                    "chunk_size": chunk_size,
                    "overlap": overlap,
                    "total_text_length": len(text),
                    "model_used": self.embedding_service.model_name,
                    "embedding_dimension": self.embedding_service.dimension,
                    "protocol_used": "gRPC"
                }
                
                logger.info(f"📤 Returning response with {response_data['chunks_inserted']} chunks")
                return web.json_response(response_data)
                
            except Exception as e:
                logger.error(f"ProximaDB batch insertion failed: {e}")
                return web.json_response({
                    "success": False,
                    "error": f"ProximaDB batch insertion failed: {str(e)}"
                }, status=500)
                        
        except Exception as e:
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def search_by_text(self, request):
        """Unified endpoint: Generate embedding from text and search in one step with full optimization support"""
        if not self.embedding_service:
            return web.json_response({
                "success": False,
                "error": "Embedding service not available"
            }, status=503)
        
        try:
            data = await request.json()
            query_text = data.get('query', '').strip()
            collection_id = data.get('collection_id')
            top_k = data.get('top_k', 10)
            metadata_filter = data.get('metadata_filter', {})
            include_metadata = data.get('include_metadata', True)
            include_vectors = data.get('include_vectors', False)
            include_text = data.get('include_text', True)  # Include original text in results
            
            # Advanced search parameters
            optimization_level = data.get('optimization_level', 'high')  # 'high', 'medium', 'low'
            use_storage_aware = data.get('use_storage_aware', True)
            quantization_level = data.get('quantization_level', 'FP32')  # 'FP32', 'PQ8', 'PQ4', 'Binary'
            enable_simd = data.get('enable_simd', True)
            search_hints = data.get('search_hints', {})
            distance_metric_override = data.get('distance_metric_override')  # Override collection default
            
            # AXIS indexing hints
            index_algorithm = search_hints.get('index_algorithm')  # 'hnsw', 'ivf', 'lsh', 'annoy'
            ef_search = search_hints.get('ef_search', 50)  # HNSW search parameter
            nprobe = search_hints.get('nprobe', 10)  # IVF search parameter
            
            # Performance hints
            enable_gpu = search_hints.get('enable_gpu', True)
            parallel_search = search_hints.get('parallel_search', True)
            cache_results = search_hints.get('cache_results', True)
            
            if not query_text:
                return web.json_response({
                    "success": False,
                    "error": "Query text is required"
                }, status=400)
            
            if not collection_id:
                return web.json_response({
                    "success": False,
                    "error": "collection_id is required"
                }, status=400)
            
            # Step 1: Generate embedding for query text
            logger.info(f"Generating embedding for query: {query_text[:50]}...")
            query_embedding = self.embedding_service.embed_text(query_text)
            
            # Step 2: Search using ProximaDB SDK with compression disabled
            try:
                from proximadb import ProximaDBClient, ClientConfig, CompressionConfig, Protocol
                
                # Create client with compression disabled for colocated services
                config = ClientConfig(
                    url=self.proximadb_url,
                    compression=CompressionConfig(
                        rest_enabled=False,  # Disabled for colocated services
                        grpc_enabled=False,  # Disabled for colocated services
                        rest_algorithm="gzip",
                        grpc_algorithm="gzip"
                    )
                )
                
                # Use gRPC for better performance (port 5679)
                grpc_url = self.proximadb_url.replace(':5678', ':5679')
                client = ProximaDBClient(url=grpc_url, protocol=Protocol.GRPC, config=config)
                
                # Build optimization hints for ProximaDB
                optimization_hints = {
                    "optimization_level": optimization_level,
                    "use_storage_aware": use_storage_aware,
                    "quantization_level": quantization_level,
                    "enable_simd": enable_simd,
                    "enable_gpu": enable_gpu,
                    "parallel_search": parallel_search,
                    "cache_results": cache_results
                }
                
                # Build search parameters
                search_params = {}
                if index_algorithm:
                    search_params["index_algorithm"] = index_algorithm
                if index_algorithm == "hnsw":
                    search_params["ef_search"] = ef_search
                elif index_algorithm == "ivf":
                    search_params["nprobe"] = nprobe
                
                # Perform search with all optimization parameters
                results = client.search(
                    collection_id=collection_id,
                    vector=query_embedding.tolist(),
                    top_k=top_k,
                    metadata_filter=metadata_filter,
                    include_metadata=include_metadata,
                    include_vectors=include_vectors,
                    optimization_level=optimization_level,
                    use_storage_aware=use_storage_aware,
                    quantization_level=quantization_level,
                    enable_simd=enable_simd
                )
                
                # Format results with additional context
                formatted_results = []
                for result in results:
                    formatted_result = {
                        "id": result.id,
                        "score": result.score,
                        "rank": result.rank if result.rank is not None else None
                    }
                    
                    if include_metadata and result.metadata:
                        formatted_result["metadata"] = result.metadata
                        # Extract text if available and requested
                        if include_text and "text" in result.metadata:
                            formatted_result["text"] = result.metadata["text"]
                    
                    if include_vectors:
                        formatted_result["vector"] = result.vector
                    
                    formatted_results.append(formatted_result)
                
                return web.json_response({
                    "success": True,
                    "query": query_text,
                    "collection_id": collection_id,
                    "results": formatted_results,
                    "total_results": len(formatted_results),
                    "embedding_model": self.embedding_service.model_name,
                    "embedding_dimension": len(query_embedding),
                    "protocol_used": "gRPC",
                    "optimization_settings": {
                        "optimization_level": optimization_level,
                        "use_storage_aware": use_storage_aware,
                        "quantization_level": quantization_level,
                        "enable_simd": enable_simd,
                        "enable_gpu": enable_gpu,
                        "parallel_search": parallel_search,
                        "cache_results": cache_results,
                        "index_algorithm": index_algorithm,
                        "search_parameters": search_params
                    },
                    "distance_metric": distance_metric_override or "collection_default"
                })
                
            except Exception as e:
                logger.error(f"ProximaDB search failed: {e}")
                return web.json_response({
                    "success": False,
                    "error": f"Search operation failed: {str(e)}"
                }, status=500)
                
        except Exception as e:
            logger.error(f"Search by text failed: {e}")
            return web.json_response({
                "success": False,
                "error": str(e)
            }, status=500)
    
    async def search_vectors_direct(self, request):
        """Direct vector search endpoint with full optimization parameters (no embedding generation)"""
        try:
            data = await request.json()
            collection_id = data.get('collection_id')
            query_vector = data.get('vector', [])
            top_k = data.get('top_k', 10)
            metadata_filter = data.get('metadata_filter', {})
            include_metadata = data.get('include_metadata', True)
            include_vectors = data.get('include_vectors', False)
            
            # Advanced search parameters
            optimization_level = data.get('optimization_level', 'high')
            use_storage_aware = data.get('use_storage_aware', True)
            quantization_level = data.get('quantization_level', 'FP32')
            enable_simd = data.get('enable_simd', True)
            search_hints = data.get('search_hints', {})
            distance_metric_override = data.get('distance_metric_override')
            
            # Scoring options
            normalize_scores = data.get('normalize_scores', True)
            score_threshold = data.get('score_threshold')  # Only return results above threshold
            
            # AXIS indexing hints
            index_algorithm = search_hints.get('index_algorithm')
            ef_search = search_hints.get('ef_search', 50)
            nprobe = search_hints.get('nprobe', 10)
            
            # Performance hints
            enable_gpu = search_hints.get('enable_gpu', True)
            parallel_search = search_hints.get('parallel_search', True)
            cache_results = search_hints.get('cache_results', True)
            
            if not collection_id:
                return web.json_response({
                    "success": False,
                    "error": "collection_id is required"
                }, status=400)
            
            if not query_vector:
                return web.json_response({
                    "success": False,
                    "error": "vector is required"
                }, status=400)
            
            # Create ProximaDB client with compression disabled
            try:
                from proximadb import ProximaDBClient, ClientConfig, CompressionConfig, Protocol
                
                config = ClientConfig(
                    url=self.proximadb_url,
                    compression=CompressionConfig(
                        rest_enabled=False,
                        grpc_enabled=False,
                        rest_algorithm="gzip",
                        grpc_algorithm="gzip"
                    )
                )
                
                # Use gRPC for better performance
                grpc_url = self.proximadb_url.replace(':5678', ':5679')
                client = ProximaDBClient(url=grpc_url, protocol=Protocol.GRPC, config=config)
                
                # Perform direct vector search
                results = client.search(
                    collection_id=collection_id,
                    vector=query_vector,
                    top_k=top_k,
                    metadata_filter=metadata_filter,
                    include_metadata=include_metadata,
                    include_vectors=include_vectors,
                    optimization_level=optimization_level,
                    use_storage_aware=use_storage_aware,
                    quantization_level=quantization_level,
                    enable_simd=enable_simd
                )
                
                # Format results with scoring details
                formatted_results = []
                max_score = 0.0
                min_score = float('inf')
                
                for result in results:
                    if result.score > max_score:
                        max_score = result.score
                    if result.score < min_score:
                        min_score = result.score
                    
                    # Apply score threshold if specified
                    if score_threshold and result.score < score_threshold:
                        continue
                    
                    formatted_result = {
                        "id": result.id,
                        "score": result.score,
                        "distance": result.distance,
                        "rank": result.rank,
                        "score_details": {
                            "raw_score": result.score,
                            "distance_value": result.distance,
                            "metric_used": distance_metric_override or "collection_default"
                        }
                    }
                    
                    if include_metadata and result.metadata:
                        formatted_result["metadata"] = result.metadata
                    
                    if include_vectors:
                        formatted_result["vector"] = result.vector
                    
                    formatted_results.append(formatted_result)
                
                # Normalize scores if requested
                if normalize_scores and max_score > min_score:
                    score_range = max_score - min_score
                    for result in formatted_results:
                        result["normalized_score"] = (result["score"] - min_score) / score_range
                
                # Build search parameters info
                search_params = {}
                if index_algorithm:
                    search_params["index_algorithm"] = index_algorithm
                    if index_algorithm == "hnsw":
                        search_params["ef_search"] = ef_search
                    elif index_algorithm == "ivf":
                        search_params["nprobe"] = nprobe
                
                return web.json_response({
                    "success": True,
                    "collection_id": collection_id,
                    "results": formatted_results,
                    "total_results": len(formatted_results),
                    "vector_dimension": len(query_vector),
                    "protocol_used": "gRPC",
                    "scoring_info": {
                        "max_score": max_score,
                        "min_score": min_score if min_score != float('inf') else 0.0,
                        "score_threshold": score_threshold,
                        "normalized": normalize_scores
                    },
                    "optimization_settings": {
                        "optimization_level": optimization_level,
                        "use_storage_aware": use_storage_aware,
                        "quantization_level": quantization_level,
                        "enable_simd": enable_simd,
                        "enable_gpu": enable_gpu,
                        "parallel_search": parallel_search,
                        "cache_results": cache_results,
                        "index_algorithm": index_algorithm,
                        "search_parameters": search_params
                    },
                    "distance_metric": distance_metric_override or "collection_default"
                })
                
            except Exception as e:
                logger.error(f"ProximaDB search failed: {e}")
                return web.json_response({
                    "success": False,
                    "error": f"Search operation failed: {str(e)}"
                }, status=500)
                
        except Exception as e:
            logger.error(f"Direct vector search failed: {e}")
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
        
        # Check if running in Docker
        is_docker = os.path.exists('/.dockerenv') or os.environ.get('DOCKER_CONTAINER', False)
        
        if is_docker:
            # Initialize demo data in background
            logger.info("🚀 Running in Docker - initializing demo data...")
            asyncio.create_task(self.initialize_demo_data())
        else:
            logger.info("Running locally - skipping automatic data initialization")
    
    async def initialize_demo_data(self):
        """Initialize demo data by running the SEC EDGAR data generator"""
        try:
            # Wait a bit for services to stabilize
            await asyncio.sleep(10)
            
            # Check if data already exists
            async with aiohttp.ClientSession() as session:
                try:
                    async with session.post(
                        f"{self.proximadb_url}/api/embeddings/search-text",
                        json={
                            "query": "test",
                            "collection_id": "sec_edgar_large_filings",
                            "top_k": 1
                        }
                    ) as resp:
                        if resp.status == 200:
                            logger.info("✅ SEC EDGAR demo data already exists")
                            return
                except:
                    pass
            
            logger.info("📊 Generating SEC EDGAR demo data...")
            
            # Run the data generator
            script_path = Path(__file__).parent / "specialized" / "sec_edgar_data_generator.py"
            if script_path.exists():
                process = await asyncio.create_subprocess_exec(
                    sys.executable, str(script_path),
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE
                )
                
                stdout, stderr = await process.communicate()
                
                if process.returncode == 0:
                    logger.info("✅ SEC EDGAR demo data generated successfully!")
                else:
                    logger.error(f"❌ Failed to generate demo data: {stderr.decode()}")
            else:
                logger.warning(f"Data generator script not found: {script_path}")
                
        except Exception as e:
            logger.error(f"Error initializing demo data: {e}")
    
    async def cleanup(self, app):
        """Cleanup on shutdown"""
        await app['session'].close()
    
    def run(self):
        """Start the server"""
        print(f"ProximaDB Unified Demo Server")
        print(f"📡 Serving on http://localhost:{self.port}")
        print(f"🔗 WebSocket: ws://localhost:{self.port}/ws")
        print(f"🔗 ProximaDB URL: {self.proximadb_url}")
        print(f"Static files: {self.static_dir}")
        print(f"\nAvailable demos: {', '.join(self.demos.keys())}")
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
        elif "[METRIC]" in line:
            parts = line.split(":")
            if len(parts) >= 2:
                metric_name = parts[0].replace("[METRIC]", "").strip()
                metric_value = parts[1].strip()
                await self.broadcast({
                    "type": "metric",
                    "demo_id": self.demo_id,
                    "run_id": self.run_id,
                    "metric": metric_name,
                    "value": metric_value
                })
        
        # Detect success/error
        elif "[SUCCESS]" in line:
            await self.broadcast({
                "type": "success",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "message": line.replace("[SUCCESS]", "").strip()
            })
        elif "[ERROR]" in line:
            await self.broadcast({
                "type": "error",
                "demo_id": self.demo_id,
                "run_id": self.run_id,
                "message": line.replace("[ERROR]", "").strip()
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