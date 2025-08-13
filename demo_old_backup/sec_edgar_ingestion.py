"""
SEC Edgar Filing Ingestion System for ProximaDB

This module provides a complete implementation for ingesting SEC Edgar filings
into ProximaDB with advanced parsing, chunking, and embedding strategies.
"""

import asyncio
import json
import re
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Any, Optional, Tuple
import hashlib

import pandas as pd
import numpy as np
from lxml import etree
from bs4 import BeautifulSoup
import aiohttp
from dataclasses import dataclass
from enum import Enum

# ProximaDB imports
from proximadb import ProximaDBClient, Protocol
from proximadb.models import (
    CollectionConfig, VectorRecord, StorageEngine,
    DistanceMetric, QuantizationType, CompressionType,
    FilterableColumn, FilterableDataType
)
from proximadb.chunking import ChunkingConfig, ChunkingStrategy, TextChunker


class FilingType(Enum):
    """SEC filing types"""
    FORM_10K = "10-K"
    FORM_10Q = "10-Q"
    FORM_8K = "8-K"
    DEF_14A = "DEF 14A"
    FORM_S1 = "S-1"


class SectionType(Enum):
    """Document section types for targeted processing"""
    BUSINESS_OVERVIEW = "business_overview"
    RISK_FACTORS = "risk_factors"
    MDA = "management_discussion_analysis"
    FINANCIAL_STATEMENTS = "financial_statements"
    NOTES_TO_FINANCIALS = "notes_to_financials"
    LEGAL_PROCEEDINGS = "legal_proceedings"
    EXECUTIVE_COMPENSATION = "executive_compensation"
    AUDITOR_REPORT = "auditor_report"


@dataclass
class FilingMetadata:
    """Metadata for SEC filing"""
    ticker: str
    company_name: str
    cik: str
    filing_type: FilingType
    filing_date: datetime
    period_end_date: datetime
    fiscal_year: int
    fiscal_quarter: Optional[int]
    url: str
    format: str  # xbrl, html, txt


class SECEdgarParser:
    """
    Advanced parser for SEC Edgar filings that handles XBRL, HTML, and TXT formats
    with intelligent fallback strategies and structure preservation.
    """
    
    def __init__(self):
        self.xbrl_namespaces = {
            'xbrli': 'http://www.xbrl.org/2003/instance',
            'us-gaap': 'http://fasb.org/us-gaap/2024',
            'dei': 'http://xbrl.sec.gov/dei/2024'
        }
        
        # Common section patterns in SEC filings
        self.section_patterns = {
            SectionType.BUSINESS_OVERVIEW: [
                r'ITEM\s+1\.?\s*BUSINESS',
                r'BUSINESS\s+OVERVIEW',
                r'DESCRIPTION\s+OF\s+BUSINESS'
            ],
            SectionType.RISK_FACTORS: [
                r'ITEM\s+1A\.?\s*RISK\s+FACTORS',
                r'RISK\s+FACTORS',
                r'RISKS\s+RELATED\s+TO'
            ],
            SectionType.MDA: [
                r'ITEM\s+7\.?\s*MANAGEMENT.*DISCUSSION',
                r'MD&A',
                r'MANAGEMENT.*ANALYSIS'
            ],
            SectionType.FINANCIAL_STATEMENTS: [
                r'ITEM\s+8\.?\s*FINANCIAL\s+STATEMENTS',
                r'CONSOLIDATED.*STATEMENTS',
                r'FINANCIAL\s+STATEMENTS'
            ]
        }
    
    async def parse_filing(
        self,
        content: str,
        metadata: FilingMetadata
    ) -> Dict[str, Any]:
        """
        Parse SEC filing with format detection and fallback strategies
        """
        if metadata.format == 'xbrl':
            return await self._parse_xbrl(content, metadata)
        elif metadata.format == 'html':
            return await self._parse_html(content, metadata)
        else:  # txt
            return await self._parse_txt(content, metadata)
    
    async def _parse_xbrl(
        self,
        content: str,
        metadata: FilingMetadata
    ) -> Dict[str, Any]:
        """
        Parse XBRL format with structured data extraction
        """
        sections = {}
        financial_data = {}
        
        try:
            # Parse XML
            root = etree.fromstring(content.encode('utf-8'))
            
            # Extract contexts (time periods)
            contexts = self._extract_xbrl_contexts(root)
            
            # Extract financial facts
            for element in root.xpath('.//*[@contextRef]'):
                tag = element.tag.split('}')[-1] if '}' in element.tag else element.tag
                context_ref = element.get('contextRef')
                value = element.text
                
                if context_ref in contexts:
                    period = contexts[context_ref]
                    if tag not in financial_data:
                        financial_data[tag] = {}
                    financial_data[tag][period] = value
            
            # Extract text blocks
            text_blocks = root.xpath('.//us-gaap:*[contains(local-name(), "TextBlock")]',
                                    namespaces=self.xbrl_namespaces)
            
            for block in text_blocks:
                section_name = block.tag.split('}')[-1].replace('TextBlock', '')
                sections[section_name] = self._clean_html(block.text or '')
            
        except Exception as e:
            print(f"XBRL parsing error: {e}")
            # Fallback to text extraction
            sections = self._extract_text_sections(content)
        
        return {
            'sections': sections,
            'financial_data': financial_data,
            'metadata': metadata.__dict__
        }
    
    async def _parse_html(
        self,
        content: str,
        metadata: FilingMetadata
    ) -> Dict[str, Any]:
        """
        Parse HTML format with table extraction and structure recovery
        """
        soup = BeautifulSoup(content, 'html.parser')
        sections = {}
        tables = []
        
        # Remove script and style elements
        for script in soup(["script", "style"]):
            script.decompose()
        
        # Extract sections based on headers
        current_section = None
        current_content = []
        
        for element in soup.find_all(['h1', 'h2', 'h3', 'h4', 'p', 'table', 'div']):
            if element.name in ['h1', 'h2', 'h3', 'h4']:
                # Save previous section
                if current_section and current_content:
                    sections[current_section] = '\n'.join(current_content)
                
                # Start new section
                current_section = element.get_text(strip=True)
                current_content = []
                
            elif element.name == 'table':
                # Extract table
                table_data = self._extract_html_table(element)
                if not table_data.empty:
                    tables.append({
                        'section': current_section,
                        'data': table_data
                    })
                    current_content.append(f"[TABLE: {len(table_data)} rows]")
                    
            elif element.name in ['p', 'div']:
                text = element.get_text(strip=True)
                if text and len(text) > 20:  # Skip very short text
                    current_content.append(text)
        
        # Save last section
        if current_section and current_content:
            sections[current_section] = '\n'.join(current_content)
        
        # If no sections found, try pattern-based extraction
        if not sections:
            sections = self._extract_text_sections(soup.get_text())
        
        return {
            'sections': sections,
            'tables': tables,
            'metadata': metadata.__dict__
        }
    
    async def _parse_txt(
        self,
        content: str,
        metadata: FilingMetadata
    ) -> Dict[str, Any]:
        """
        Parse TXT format with pattern recognition and ASCII table detection
        """
        sections = self._extract_text_sections(content)
        tables = self._extract_ascii_tables(content)
        
        return {
            'sections': sections,
            'tables': tables,
            'metadata': metadata.__dict__
        }
    
    def _extract_xbrl_contexts(self, root) -> Dict[str, str]:
        """Extract time period contexts from XBRL"""
        contexts = {}
        
        for context in root.xpath('.//xbrli:context', namespaces=self.xbrl_namespaces):
            context_id = context.get('id')
            
            # Get period
            period = context.find('.//xbrli:period', namespaces=self.xbrl_namespaces)
            if period is not None:
                instant = period.find('xbrli:instant', namespaces=self.xbrl_namespaces)
                if instant is not None:
                    contexts[context_id] = instant.text
                else:
                    start = period.find('xbrli:startDate', namespaces=self.xbrl_namespaces)
                    end = period.find('xbrli:endDate', namespaces=self.xbrl_namespaces)
                    if start is not None and end is not None:
                        contexts[context_id] = f"{start.text} to {end.text}"
        
        return contexts
    
    def _extract_text_sections(self, text: str) -> Dict[str, str]:
        """Extract sections using pattern matching"""
        sections = {}
        
        for section_type, patterns in self.section_patterns.items():
            for pattern in patterns:
                regex = re.compile(pattern, re.IGNORECASE | re.MULTILINE)
                matches = list(regex.finditer(text))
                
                if matches:
                    for i, match in enumerate(matches):
                        start = match.end()
                        
                        # Find end of section (next section or pattern)
                        if i < len(matches) - 1:
                            end = matches[i + 1].start()
                        else:
                            # Look for next major section
                            next_section = re.search(
                                r'ITEM\s+\d+[A-Z]?\.?\s+[A-Z]',
                                text[start:],
                                re.IGNORECASE
                            )
                            end = start + next_section.start() if next_section else len(text)
                        
                        section_text = text[start:end].strip()
                        if len(section_text) > 100:  # Minimum content threshold
                            sections[section_type.value] = section_text
                    
                    break  # Use first matching pattern
        
        return sections
    
    def _extract_html_table(self, table_element) -> pd.DataFrame:
        """Extract and structure HTML table"""
        rows = []
        headers = []
        
        # Extract headers
        header_row = table_element.find('thead')
        if header_row:
            headers = [th.get_text(strip=True) for th in header_row.find_all(['th', 'td'])]
        else:
            # Try first row as headers
            first_row = table_element.find('tr')
            if first_row:
                potential_headers = [cell.get_text(strip=True) 
                                   for cell in first_row.find_all(['th', 'td'])]
                if all(len(h) < 50 for h in potential_headers):  # Likely headers
                    headers = potential_headers
        
        # Extract data rows
        for tr in table_element.find_all('tr')[1 if headers else 0:]:
            row = [td.get_text(strip=True) for td in tr.find_all(['td', 'th'])]
            if row and any(cell for cell in row):  # Skip empty rows
                rows.append(row)
        
        # Create DataFrame
        if headers and rows:
            # Ensure consistent column count
            max_cols = max(len(headers), max(len(row) for row in rows))
            headers = headers[:max_cols] + [''] * (max_cols - len(headers))
            rows = [row[:max_cols] + [''] * (max_cols - len(row)) for row in rows]
            return pd.DataFrame(rows, columns=headers)
        elif rows:
            return pd.DataFrame(rows)
        else:
            return pd.DataFrame()
    
    def _extract_ascii_tables(self, text: str) -> List[Dict[str, Any]]:
        """Detect and extract ASCII art tables from text"""
        tables = []
        lines = text.split('\n')
        
        in_table = False
        table_lines = []
        table_start_line = 0
        
        for i, line in enumerate(lines):
            # Detect table boundaries (lines with multiple dashes or equals)
            is_separator = (
                line.count('-') >= 20 or
                line.count('=') >= 20 or
                line.count('_') >= 20
            )
            
            # Detect table content (lines with multiple pipes or consistent spacing)
            has_structure = (
                line.count('|') >= 2 or
                (len(line) > 0 and line.count('  ') >= 3)  # Multiple spaces suggest columns
            )
            
            if (is_separator or has_structure) and not in_table:
                in_table = True
                table_start_line = i
                table_lines = [line]
            elif in_table:
                if line.strip() == '' and i > table_start_line + 2:
                    # End of table
                    if len(table_lines) > 2:
                        tables.append({
                            'line_start': table_start_line,
                            'line_end': i,
                            'content': '\n'.join(table_lines)
                        })
                    in_table = False
                    table_lines = []
                else:
                    table_lines.append(line)
        
        return tables
    
    def _clean_html(self, html_text: str) -> str:
        """Clean HTML text while preserving structure"""
        if not html_text:
            return ''
        
        soup = BeautifulSoup(html_text, 'html.parser')
        
        # Remove scripts and styles
        for element in soup(['script', 'style']):
            element.decompose()
        
        # Convert breaks to newlines
        for br in soup.find_all('br'):
            br.replace_with('\n')
        
        # Convert paragraphs to double newlines
        for p in soup.find_all('p'):
            p.insert_before('\n\n')
            p.insert_after('\n\n')
        
        # Get text
        text = soup.get_text()
        
        # Clean up whitespace
        lines = [line.strip() for line in text.split('\n')]
        lines = [line for line in lines if line]
        
        return '\n'.join(lines)


class SECFilingChunker:
    """
    Advanced chunking system for SEC filings that handles different
    content types with appropriate strategies
    """
    
    def __init__(self, client: ProximaDBClient):
        self.client = client
        
        # Define chunking strategies for different content types
        self.chunking_configs = {
            SectionType.RISK_FACTORS: ChunkingConfig(
                strategy=ChunkingStrategy.SEMANTIC,
                chunk_size=768,
                chunk_overlap=100,
                min_chunk_size=256,
                preserve_sentences=True
            ),
            
            SectionType.MDA: ChunkingConfig(
                strategy=ChunkingStrategy.RECURSIVE,
                chunk_size=1024,
                chunk_overlap=128,
                preserve_paragraphs=True,
                add_context=True,
                context_size=100
            ),
            
            SectionType.FINANCIAL_STATEMENTS: ChunkingConfig(
                strategy=ChunkingStrategy.FIXED_SIZE,
                chunk_size=2048,
                chunk_overlap=0,
                preserve_tables=True
            ),
            
            SectionType.BUSINESS_OVERVIEW: ChunkingConfig(
                strategy=ChunkingStrategy.PARAGRAPH,
                min_chunk_size=300,
                max_chunk_size=1500,
                preserve_paragraphs=True
            ),
            
            # Default for unclassified sections
            'default': ChunkingConfig(
                strategy=ChunkingStrategy.SLIDING_WINDOW,
                chunk_size=512,
                chunk_overlap=64
            )
        }
    
    def classify_section(self, section_name: str) -> SectionType:
        """Classify section type based on name"""
        section_lower = section_name.lower()
        
        if 'risk' in section_lower:
            return SectionType.RISK_FACTORS
        elif 'management' in section_lower or 'md&a' in section_lower:
            return SectionType.MDA
        elif 'financial' in section_lower and 'statement' in section_lower:
            return SectionType.FINANCIAL_STATEMENTS
        elif 'business' in section_lower or 'overview' in section_lower:
            return SectionType.BUSINESS_OVERVIEW
        elif 'legal' in section_lower:
            return SectionType.LEGAL_PROCEEDINGS
        elif 'note' in section_lower:
            return SectionType.NOTES_TO_FINANCIALS
        else:
            return None
    
    async def chunk_filing(
        self,
        parsed_filing: Dict[str, Any]
    ) -> List[Dict[str, Any]]:
        """
        Chunk parsed filing into vector-ready documents
        """
        chunks = []
        metadata = parsed_filing['metadata']
        
        # Process text sections
        for section_name, section_content in parsed_filing.get('sections', {}).items():
            section_type = self.classify_section(section_name)
            config = self.chunking_configs.get(
                section_type,
                self.chunking_configs['default']
            )
            
            chunker = TextChunker(config)
            
            # Generate source ID
            source_id = f"{metadata['ticker']}_{metadata['filing_type']}_{metadata['filing_date']}"
            
            # Chunk the text
            text_chunks = chunker.chunk_text(
                section_content,
                source_id=source_id,
                metadata={
                    'section': section_name,
                    'section_type': section_type.value if section_type else 'unknown',
                    'ticker': metadata['ticker'],
                    'filing_type': metadata['filing_type'],
                    'filing_date': metadata['filing_date'],
                    'fiscal_year': metadata['fiscal_year']
                }
            )
            
            for chunk in text_chunks:
                chunks.append({
                    'id': chunk.chunk_id,
                    'text': chunk.text,
                    'metadata': chunk.metadata
                })
        
        # Process tables
        for table_info in parsed_filing.get('tables', []):
            table_text = self._table_to_text(table_info['data'])
            
            # Create single chunk for small tables, split large ones
            if len(table_text) < 2000:
                chunk_id = f"table_{hashlib.md5(table_text.encode()).hexdigest()[:8]}"
                chunks.append({
                    'id': chunk_id,
                    'text': table_text,
                    'metadata': {
                        'type': 'table',
                        'section': table_info.get('section', 'unknown'),
                        'ticker': metadata['ticker'],
                        'filing_type': metadata['filing_type'],
                        'filing_date': metadata['filing_date'],
                        'rows': len(table_info['data']) if isinstance(table_info['data'], pd.DataFrame) else 0
                    }
                })
            else:
                # Split large tables
                table_chunks = self._split_large_table(table_info['data'])
                for i, chunk_text in enumerate(table_chunks):
                    chunk_id = f"table_{hashlib.md5(chunk_text.encode()).hexdigest()[:8]}_{i}"
                    chunks.append({
                        'id': chunk_id,
                        'text': chunk_text,
                        'metadata': {
                            'type': 'table_segment',
                            'segment': i,
                            'section': table_info.get('section', 'unknown'),
                            'ticker': metadata['ticker'],
                            'filing_type': metadata['filing_type'],
                            'filing_date': metadata['filing_date']
                        }
                    })
        
        # Process financial data (from XBRL)
        if 'financial_data' in parsed_filing:
            for metric, values in parsed_filing['financial_data'].items():
                # Create searchable text representation
                text = f"{metric}: " + ", ".join([f"{period}: {value}" for period, value in values.items()])
                
                chunk_id = f"financial_{hashlib.md5(text.encode()).hexdigest()[:8]}"
                chunks.append({
                    'id': chunk_id,
                    'text': text,
                    'metadata': {
                        'type': 'financial_metric',
                        'metric': metric,
                        'ticker': metadata['ticker'],
                        'filing_type': metadata['filing_type'],
                        'filing_date': metadata['filing_date'],
                        'fiscal_year': metadata['fiscal_year']
                    }
                })
        
        return chunks
    
    def _table_to_text(self, table_data) -> str:
        """Convert table to searchable text"""
        if isinstance(table_data, pd.DataFrame):
            return table_data.to_string()
        elif isinstance(table_data, str):
            return table_data
        else:
            return str(table_data)
    
    def _split_large_table(self, table_data: pd.DataFrame, max_rows: int = 20) -> List[str]:
        """Split large table into chunks"""
        if not isinstance(table_data, pd.DataFrame):
            return [str(table_data)]
        
        chunks = []
        for i in range(0, len(table_data), max_rows):
            chunk = table_data.iloc[i:i+max_rows]
            chunks.append(chunk.to_string())
        
        return chunks


class SECEdgarIngestionPipeline:
    """
    Complete ingestion pipeline for SEC Edgar filings into ProximaDB
    """
    
    def __init__(
        self,
        proximadb_url: str = "http://localhost:5678",
        grpc_url: str = "http://localhost:5679"
    ):
        self.rest_client = ProximaDBClient(
            base_url=proximadb_url,
            protocol=Protocol.REST
        )
        self.grpc_client = ProximaDBClient(
            base_url=grpc_url,
            protocol=Protocol.GRPC
        )
        
        self.parser = SECEdgarParser()
        self.chunker = SECFilingChunker(self.rest_client)
        
        # SEC API configuration
        self.sec_base_url = "https://www.sec.gov/Archives/edgar/data"
        self.user_agent = "ProximaDB Research (research@example.com)"
    
    async def initialize_collections(self):
        """
        Create ProximaDB collections for SEC filings
        """
        # Main filing collection with FinBERT embeddings (768 dimensions)
        main_collection = CollectionConfig(
            name="sec_filings_main",
            dimension=768,  # FinBERT dimension
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            compression=CompressionType.ZSTD,
            quantization={
                'type': QuantizationType.INT8,
                'enable_two_stage_search': True,
                'rerank_top_k': 100
            },
            filterable_columns=[
                FilterableColumn("ticker", FilterableDataType.STRING),
                FilterableColumn("filing_type", FilterableDataType.STRING),
                FilterableColumn("section_type", FilterableDataType.STRING),
                FilterableColumn("filing_date", FilterableDataType.TIMESTAMP),
                FilterableColumn("fiscal_year", FilterableDataType.INTEGER),
            ]
        )
        
        # Financial metrics collection (smaller embeddings)
        metrics_collection = CollectionConfig(
            name="sec_financial_metrics",
            dimension=384,  # MiniLM dimension
            distance_metric=DistanceMetric.EUCLIDEAN,
            storage_engine=StorageEngine.SST,
            filterable_columns=[
                FilterableColumn("ticker", FilterableDataType.STRING),
                FilterableColumn("metric", FilterableDataType.STRING),
                FilterableColumn("fiscal_year", FilterableDataType.INTEGER),
            ]
        )
        
        # Create collections
        await self.grpc_client.create_collection_async(main_collection)
        await self.grpc_client.create_collection_async(metrics_collection)
        
        print("Collections initialized successfully")
    
    async def fetch_filing(
        self,
        cik: str,
        accession_number: str,
        filing_type: str
    ) -> str:
        """
        Fetch filing content from SEC Edgar
        """
        # Format CIK (10 digits, zero-padded)
        cik = cik.zfill(10)
        
        # Format accession number (remove dashes)
        accession_clean = accession_number.replace('-', '')
        
        # Construct URL
        url = f"{self.sec_base_url}/{cik}/{accession_clean}/{accession_number}.txt"
        
        headers = {
            'User-Agent': self.user_agent,
            'Accept-Encoding': 'gzip, deflate',
            'Host': 'www.sec.gov'
        }
        
        async with aiohttp.ClientSession() as session:
            async with session.get(url, headers=headers) as response:
                if response.status == 200:
                    return await response.text()
                else:
                    raise Exception(f"Failed to fetch filing: {response.status}")
    
    async def ingest_filing(
        self,
        ticker: str,
        cik: str,
        filing_type: FilingType,
        accession_number: str,
        filing_date: datetime,
        period_end_date: datetime,
        fiscal_year: int,
        fiscal_quarter: Optional[int] = None
    ):
        """
        Complete ingestion pipeline for a single filing
        """
        print(f"Ingesting {filing_type.value} for {ticker}...")
        
        # Fetch filing
        content = await self.fetch_filing(cik, accession_number, filing_type.value)
        
        # Detect format
        format_type = self._detect_format(content)
        
        # Create metadata
        metadata = FilingMetadata(
            ticker=ticker,
            company_name=ticker,  # Would fetch from company API
            cik=cik,
            filing_type=filing_type,
            filing_date=filing_date,
            period_end_date=period_end_date,
            fiscal_year=fiscal_year,
            fiscal_quarter=fiscal_quarter,
            url=f"{self.sec_base_url}/{cik}/{accession_number}",
            format=format_type
        )
        
        # Parse filing
        parsed = await self.parser.parse_filing(content, metadata)
        
        # Chunk filing
        chunks = await self.chunker.chunk_filing(parsed)
        
        # Generate embeddings (using mock embeddings for demo)
        vectors = []
        for chunk in chunks:
            # In production, use FinBERT or other embedding model
            embedding = np.random.randn(768).tolist()  # Mock embedding
            
            vectors.append(VectorRecord(
                id=chunk['id'],
                vector=embedding,
                metadata=chunk['metadata']
            ))
        
        # Insert into ProximaDB
        if vectors:
            # Use gRPC for bulk insertion (faster)
            result = await self.grpc_client.insert_vectors_async(
                collection_id="sec_filings_main",
                records=vectors
            )
            print(f"Inserted {len(vectors)} chunks for {ticker} {filing_type.value}")
        
        return len(chunks)
    
    def _detect_format(self, content: str) -> str:
        """Detect filing format"""
        if '<xbrl' in content.lower() or '<?xml' in content.lower():
            return 'xbrl'
        elif '<html' in content.lower():
            return 'html'
        else:
            return 'txt'
    
    async def search_filings(
        self,
        query: str,
        ticker: Optional[str] = None,
        filing_type: Optional[FilingType] = None,
        fiscal_year: Optional[int] = None,
        top_k: int = 10
    ) -> List[Dict[str, Any]]:
        """
        Search SEC filings with metadata filtering
        """
        # Generate query embedding (mock for demo)
        query_embedding = np.random.randn(768).tolist()
        
        # Build metadata filter
        metadata_filter = {}
        if ticker:
            metadata_filter['ticker'] = ticker
        if filing_type:
            metadata_filter['filing_type'] = filing_type.value
        if fiscal_year:
            metadata_filter['fiscal_year'] = fiscal_year
        
        # Search using REST client for metadata filtering
        results = await self.rest_client.search_vectors_async(
            collection_id="sec_filings_main",
            query_vector=query_embedding,
            metadata_filter=metadata_filter,
            top_k=top_k
        )
        
        return results


async def main():
    """
    Demo of SEC Edgar ingestion pipeline
    """
    pipeline = SECEdgarIngestionPipeline()
    
    # Initialize collections
    await pipeline.initialize_collections()
    
    # Example: Ingest Apple's 10-K
    await pipeline.ingest_filing(
        ticker="AAPL",
        cik="0000320193",
        filing_type=FilingType.FORM_10K,
        accession_number="000032019324000123",
        filing_date=datetime(2024, 11, 1),
        period_end_date=datetime(2024, 9, 28),
        fiscal_year=2024
    )
    
    # Search example
    results = await pipeline.search_filings(
        query="artificial intelligence risks regulatory",
        ticker="AAPL",
        filing_type=FilingType.FORM_10K,
        fiscal_year=2024,
        top_k=5
    )
    
    print(f"\nSearch Results:")
    for result in results:
        print(f"- {result['id']}: {result['metadata']}")


if __name__ == "__main__":
    asyncio.run(main())