"""
Test prepare_vector_records metadata handling with filterable/extra separation
"""
import pytest
from proximadb.chunking import prepare_vector_records
from proximadb import VectorRecord


class TestPrepareVectorRecords:
    """Test the flexible metadata handling in prepare_vector_records"""
    
    def test_basic_conversion(self):
        """Test basic chunk to VectorRecord conversion"""
        response = {
            "chunks": [
                {
                    "id": "chunk_0",
                    "text": "Test chunk text",
                    "embedding": [0.1, 0.2, 0.3],
                    "start_pos": 0,
                    "end_pos": 15
                }
            ],
            "model": "all-mpnet-base-v2",
            "chunking_strategy": "semantic",
            "chunk_size": 400,
            "overlap": 100
        }
        
        records = prepare_vector_records(
            response,
            source_id="test_doc",
            source_type="test"
        )
        
        assert len(records) == 1
        record = records[0]
        assert isinstance(record, VectorRecord)
        assert record.id == "chunk_0"
        assert record.vector == [0.1, 0.2, 0.3]
        assert record.metadata["text"] == "Test chunk text"
        assert record.metadata["source_id"] == "test_doc"
        assert record.metadata["source_type"] == "test"
        assert record.metadata["chunk_strategy"] == "semantic"
    
    def test_filter_fields_handling(self):
        """Test that special filter fields stay at top level"""
        response = {
            "chunks": [{"id": "c1", "text": "Text", "embedding": [0.1]}]
        }
        
        records = prepare_vector_records(
            response,
            source_id="doc",
            source_metadata={
                "category": "AI",
                "author": "John Doe",
                "tags": ["ml", "nlp"],
                "custom_field": "value"
            }
        )
        
        metadata = records[0].metadata
        # Filter fields at top level
        assert metadata["category"] == "AI"
        assert metadata["author"] == "John Doe"
        assert metadata["tags"] == ["ml", "nlp"]
        # Custom field is namespaced
        assert metadata["source_custom_field"] == "value"
        assert "custom_field" not in metadata
    
    def test_custom_metadata_function(self):
        """Test custom chunk metadata function"""
        def custom_fn(chunk, index):
            return {
                "section": f"part_{index // 2}",
                "importance": "high" if index == 0 else "normal",
                "char_count": len(chunk["text"])
            }
        
        response = {
            "chunks": [
                {"id": f"c{i}", "text": f"Chunk {i}", "embedding": [0.1]}
                for i in range(4)
            ]
        }
        
        records = prepare_vector_records(
            response,
            source_id="doc",
            chunk_metadata_fn=custom_fn
        )
        
        # Check first chunk
        meta0 = records[0].metadata
        assert meta0["custom_section"] == "part_0"
        assert meta0["custom_importance"] == "high"
        assert meta0["custom_char_count"] == 7
        
        # Check third chunk
        meta2 = records[2].metadata
        assert meta2["custom_section"] == "part_1"
        assert meta2["custom_importance"] == "normal"
    
    def test_preserve_embedding_metadata(self):
        """Test preserving/ignoring embedding service metadata"""
        response = {
            "chunks": [{
                "id": "c1",
                "text": "Text",
                "embedding": [0.1],
                "confidence": 0.95,
                "language": "en"
            }]
        }
        
        # With preservation
        records = prepare_vector_records(
            response,
            source_id="doc",
            preserve_embedding_metadata=True
        )
        assert records[0].metadata["chunk_confidence"] == 0.95
        assert records[0].metadata["chunk_language"] == "en"
        
        # Without preservation
        records = prepare_vector_records(
            response,
            source_id="doc",
            preserve_embedding_metadata=False
        )
        assert "chunk_confidence" not in records[0].metadata
        assert "chunk_language" not in records[0].metadata
    
    def test_error_handling(self):
        """Test error handling for invalid inputs"""
        # No chunks
        with pytest.raises(ValueError, match="No chunks found"):
            prepare_vector_records({"chunks": []}, "doc")
        
        # Missing embedding
        with pytest.raises(ValueError, match="missing embedding"):
            prepare_vector_records({
                "chunks": [{"id": "c1", "text": "No embedding"}]
            }, "doc")
    
    def test_custom_function_error_handling(self):
        """Test that custom function errors don't break processing"""
        def bad_fn(chunk, index):
            if index == 1:
                raise ValueError("Test error")
            return {"ok": True}
        
        response = {
            "chunks": [
                {"id": f"c{i}", "text": f"Text {i}", "embedding": [0.1]}
                for i in range(3)
            ]
        }
        
        # Should not raise, just log warning
        records = prepare_vector_records(
            response,
            source_id="doc",
            chunk_metadata_fn=bad_fn
        )
        
        assert len(records) == 3
        assert records[0].metadata["custom_ok"] is True
        assert "custom_ok" not in records[1].metadata  # Failed
        assert records[2].metadata["custom_ok"] is True
    
    def test_filterable_metadata_separation(self):
        """Test proper separation of filterable vs extra metadata"""
        response = {
            "chunks": [{
                "id": "c1",
                "text": "Test content",
                "embedding": [0.1, 0.2, 0.3]
            }],
            "model": "all-mpnet-base-v2",
            "chunk_size": 400
        }
        
        records = prepare_vector_records(
            response,
            source_id="DOC-001",
            source_type="product_manual",
            source_metadata={
                # High cardinality - should be filterable
                "product_id": "SKU-12345",
                "brand": "TechCorp",
                "price": 299.99,
                # Low cardinality - should be extra
                "currency": "USD",
                "version": "1.0",
                "language": "en"
            },
            filterable_fields=["product_id", "brand", "price"]
        )
        
        meta = records[0].metadata
        
        # Check filterable fields are present
        assert meta["text"] == "Test content"
        assert meta["chunk_index"] == 0
        assert meta["source_type"] == "product_manual"
        assert meta["product_id"] == "SKU-12345"
        assert meta["brand"] == "TechCorp"
        assert meta["price"] == 299.99
        
        # Check non-filterable fields are namespaced
        assert meta["source_currency"] == "USD"
        assert meta["source_version"] == "1.0"
        assert meta["source_language"] == "en"
        
        # Check low-cardinality fields in extra
        assert meta["source_id"] == "DOC-001"
        assert meta["embedding_model"] == "all-mpnet-base-v2"
        assert meta["chunk_size"] == 400
    
    def test_ecommerce_use_case(self):
        """Test realistic e-commerce product chunking"""
        def enrich_product_chunk(chunk, index):
            # Simulate product-specific enrichment
            text = chunk["text"].lower()
            return {
                "has_specs": "specifications" in text or "specs" in text,
                "has_warranty": "warranty" in text,
                "section": "overview" if index == 0 else "details"
            }
        
        response = {
            "chunks": [
                {
                    "id": "prod_chunk_0",
                    "text": "TechCorp UltraBook Pro - Premium laptop with specifications",
                    "embedding": [0.1] * 768
                },
                {
                    "id": "prod_chunk_1", 
                    "text": "Extended warranty available. Intel processor, 16GB RAM",
                    "embedding": [0.2] * 768
                }
            ],
            "model": "all-mpnet-base-v2",
            "dimension": 768
        }
        
        records = prepare_vector_records(
            response,
            source_id="PROD-789",
            source_type="product",
            source_metadata={
                # E-commerce specific metadata
                "sku": "TECH-UB-PRO-2024",
                "brand": "TechCorp",
                "category": "Laptops",
                "subcategory": "Professional",
                "price": 1499.99,
                "currency": "USD",  # Low cardinality
                "in_stock": True,
                "rating": 4.5,
                "review_count": 128,
                "color": "Space Gray",
                "storage": "512GB",
                "ram": "16GB"
            },
            chunk_metadata_fn=enrich_product_chunk,
            filterable_fields=[
                "sku", "brand", "category", "subcategory",
                "price", "in_stock", "rating", "color",
                "storage", "ram", "has_specs", "has_warranty", "section"
            ]
        )
        
        # Check first chunk
        meta0 = records[0].metadata
        assert meta0["sku"] == "TECH-UB-PRO-2024"
        assert meta0["price"] == 1499.99
        assert meta0["in_stock"] is True
        assert meta0["has_specs"] is True
        assert meta0["section"] == "overview"
        assert meta0["source_currency"] == "USD"  # Low cardinality in extra
        
        # Check second chunk
        meta1 = records[1].metadata
        assert meta1["has_warranty"] is True
        assert meta1["section"] == "details"
    
    def test_comprehensive_metadata(self):
        """Test complete metadata handling with all features"""
        def enrich_chunk(chunk, index):
            text = chunk["text"]
            return {
                "category": "Technical",  # Override source category
                "has_numbers": any(c.isdigit() for c in text),
                "word_count": len(text.split()),
                "position_tag": "intro" if index < 2 else "body"
            }
        
        response = {
            "chunks": [
                {
                    "id": f"chunk_{i}",
                    "text": f"Chunk {i}: ProximaDB is fast",
                    "embedding": [0.1 + i * 0.1, 0.2, 0.3],
                    "start_pos": i * 100,
                    "end_pos": (i + 1) * 100,
                    "token_count": 5 + i
                }
                for i in range(3)
            ],
            "model": "all-mpnet-base-v2",
            "chunking_strategy": "sliding_window",
            "chunk_size": 100,
            "overlap": 20,
            "dimension": 768
        }
        
        records = prepare_vector_records(
            response,
            source_id="technical_doc_v2",
            source_type="documentation",
            source_metadata={
                "category": "General",  # Will be overridden
                "author": "ProximaDB Team",
                "version": "2.0",
                "tags": ["database", "vector", "search"],
                "published_date": "2024-01-15",
                "internal_id": "DOC-12345"
            },
            chunk_metadata_fn=enrich_chunk,
            preserve_embedding_metadata=True,
            filterable_fields=["has_numbers", "word_count", "position_tag"]
        )
        
        # Validate all records created
        assert len(records) == 3
        
        # Check first record in detail
        r0 = records[0]
        meta = r0.metadata
        
        # Core filterable fields
        assert meta["text"] == "Chunk 0: ProximaDB is fast"
        assert meta["chunk_index"] == 0
        assert meta["source_type"] == "documentation"
        
        # Filterable fields from different sources
        assert meta["category"] == "Technical"  # From custom function
        assert meta["author"] == "ProximaDB Team"
        assert meta["tags"] == ["database", "vector", "search"]
        assert meta["has_numbers"] is True
        assert meta["word_count"] == 5
        assert meta["position_tag"] == "intro"
        
        # Non-filterable fields in extra
        assert meta["source_id"] == "technical_doc_v2"
        assert meta["source_version"] == "2.0"
        assert meta["source_published_date"] == "2024-01-15"
        assert meta["source_internal_id"] == "DOC-12345"
        
        # Preserved embedding metadata in extra
        assert meta["chunk_token_count"] == 5
        
        # Auto-generated fields in extra
        assert meta["chunk_strategy"] == "sliding_window"
        assert meta["embedding_model"] == "all-mpnet-base-v2"
        assert meta["chunk_size"] == 100
        assert meta["chunk_overlap"] == 20
        assert "created_at" in meta
        assert "indexed_at" in meta