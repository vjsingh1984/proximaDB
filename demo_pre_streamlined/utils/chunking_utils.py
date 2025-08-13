#!/usr/bin/env python3
"""
Unified chunking utilities for ProximaDB demos

Provides consistent chunking and metadata handling across all demos
"""

import time
import requests
from typing import List, Dict, Any, Optional, Tuple
from proximadb import VectorRecord
import logging

logger = logging.getLogger(__name__)


class ChunkingService:
    """Service for chunking text and preparing vectors with metadata"""
    
    def __init__(self, embedding_service_url: str = "http://localhost:8080"):
        self.embedding_service_url = embedding_service_url
        
    def chunk_and_prepare_vectors(
        self,
        text: str,
        source_id: str,
        collection_type: str,
        strategy: str = "semantic",
        chunk_size: int = 400,
        overlap: int = 100,
        model: str = "all-mpnet-base-v2",
        additional_metadata: Optional[Dict[str, Any]] = None
    ) -> Tuple[List[VectorRecord], List[Dict[str, Any]]]:
        """
        Chunk text and prepare VectorRecord objects with proper metadata
        
        Args:
            text: Text to chunk
            source_id: ID of the source document/item
            collection_type: Type of collection (e.g., 'knowledge_base', 'product', 'document')
            strategy: Chunking strategy
            chunk_size: Size of chunks
            overlap: Overlap between chunks
            model: Embedding model to use
            additional_metadata: Additional metadata to include with each chunk
            
        Returns:
            Tuple of (vector_records, chunk_info)
        """
        # Call chunking API
        chunk_response = requests.post(
            f"{self.embedding_service_url}/api/embeddings/chunk",
            json={
                "text": text,
                "strategy": strategy,
                "chunk_size": chunk_size,
                "overlap": overlap,
                "model": model
            }
        )
        
        if chunk_response.status_code != 200:
            raise Exception(f"Failed to chunk text: {chunk_response.text}")
            
        chunks_data = chunk_response.json()
        chunks = chunks_data.get("chunks", [])
        
        # Prepare vector records with comprehensive metadata
        vector_records = []
        chunk_info = []
        
        for idx, chunk in enumerate(chunks):
            # Base metadata that should always be included
            metadata = {
                # Core chunk information
                "text": chunk["text"],  # Store the actual chunk text
                "chunk_index": idx,
                "chunk_id": f"{source_id}_chunk_{idx}",
                "source_id": source_id,
                "collection_type": collection_type,
                
                # Chunking details
                "chunk_strategy": strategy,
                "chunk_size": chunk_size,
                "chunk_overlap": overlap,
                "total_chunks": len(chunks),
                
                # Positional information
                "start_char": chunk.get("start", 0),
                "end_char": chunk.get("end", len(chunk["text"])),
                
                # Timestamps
                "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "indexed_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                
                # Model info
                "embedding_model": model,
                "embedding_dimension": len(chunk["embedding"])
            }
            
            # Add any additional metadata provided
            if additional_metadata:
                # Prefix additional metadata to avoid conflicts
                for key, value in additional_metadata.items():
                    if key not in metadata:  # Don't override core metadata
                        metadata[f"meta_{key}"] = str(value) if value is not None else ""
            
            # Create vector record
            vector_record = VectorRecord(
                id=f"{source_id}_chunk_{idx}",
                vector=chunk["embedding"],
                metadata=metadata
            )
            
            vector_records.append(vector_record)
            
            # Store chunk info for reporting
            chunk_info.append({
                "index": idx,
                "text_preview": chunk["text"][:100] + "..." if len(chunk["text"]) > 100 else chunk["text"],
                "length": len(chunk["text"]),
                "metadata_keys": list(metadata.keys())
            })
            
        return vector_records, chunk_info


def chunk_document(
    document_text: str,
    document_id: str,
    document_type: str = "document",
    chunking_service: Optional[ChunkingService] = None,
    **kwargs
) -> Tuple[List[VectorRecord], List[Dict[str, Any]]]:
    """
    Convenience function to chunk a document with standard metadata
    
    Args:
        document_text: Full document text
        document_id: Unique document identifier
        document_type: Type of document
        chunking_service: Optional ChunkingService instance (creates one if not provided)
        **kwargs: Additional arguments passed to chunk_and_prepare_vectors
        
    Returns:
        Tuple of (vector_records, chunk_info)
    """
    if chunking_service is None:
        chunking_service = ChunkingService()
        
    return chunking_service.chunk_and_prepare_vectors(
        text=document_text,
        source_id=document_id,
        collection_type=document_type,
        **kwargs
    )


def chunk_knowledge_base_entry(
    title: str,
    content: str,
    category: str,
    chunking_service: Optional[ChunkingService] = None,
    **kwargs
) -> Tuple[List[VectorRecord], List[Dict[str, Any]]]:
    """
    Chunk a knowledge base entry with appropriate metadata
    """
    if chunking_service is None:
        chunking_service = ChunkingService()
        
    additional_metadata = {
        "title": title,
        "category": category,
        "content_type": "knowledge_base"
    }
    
    # Merge with any provided additional metadata
    if "additional_metadata" in kwargs:
        additional_metadata.update(kwargs.pop("additional_metadata"))
        
    return chunking_service.chunk_and_prepare_vectors(
        text=content,
        source_id=title.lower().replace(" ", "_"),
        collection_type="knowledge_base",
        additional_metadata=additional_metadata,
        **kwargs
    )


def chunk_product_description(
    product: Dict[str, Any],
    chunking_service: Optional[ChunkingService] = None,
    **kwargs
) -> Tuple[List[VectorRecord], List[Dict[str, Any]]]:
    """
    Chunk a product description with e-commerce metadata
    """
    if chunking_service is None:
        chunking_service = ChunkingService()
        
    # Combine product fields for embedding
    text = f"{product['name']} {product.get('description', '')} {product.get('brand', '')} {product.get('category', '')}"
    
    additional_metadata = {
        "product_name": product.get("name", ""),
        "product_id": product.get("id", ""),
        "category": product.get("category", ""),
        "subcategory": product.get("subcategory", ""),
        "brand": product.get("brand", ""),
        "price": str(product.get("price", 0)),
        "rating": str(product.get("rating", 0)),
        "in_stock": str(product.get("in_stock", True)),
        "tags": ",".join(product.get("tags", []))
    }
    
    # Since products are usually short, we might want smaller chunks
    if "chunk_size" not in kwargs:
        kwargs["chunk_size"] = 200
    if "overlap" not in kwargs:
        kwargs["overlap"] = 50
        
    return chunking_service.chunk_and_prepare_vectors(
        text=text,
        source_id=product.get("id", "unknown"),
        collection_type="product",
        additional_metadata=additional_metadata,
        **kwargs
    )


def print_chunk_summary(chunk_info: List[Dict[str, Any]], source_name: str):
    """Print a summary of chunks created"""
    print(f"\n📊 Chunking Summary for '{source_name}':")
    print(f"   Total chunks: {len(chunk_info)}")
    
    if chunk_info:
        avg_length = sum(c["length"] for c in chunk_info) / len(chunk_info)
        print(f"   Average chunk length: {avg_length:.0f} characters")
        print(f"   Metadata fields: {len(chunk_info[0]['metadata_keys'])}")
        
        # Show first few chunks
        print("\n   Sample chunks:")
        for i, chunk in enumerate(chunk_info[:3]):
            print(f"   [{i+1}] {chunk['text_preview']}")
            
        if len(chunk_info) > 3:
            print(f"   ... and {len(chunk_info) - 3} more chunks")