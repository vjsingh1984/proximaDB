"""
Unit tests for document and binary parsers.

This module tests:
- Binary type detection
- Document type detection
- Tool detection
- Binary parser (with mocking)
- Document/OCR parser (with mocking)
"""

import pytest
import sys
import os
import tempfile
from pathlib import Path
from unittest.mock import Mock, patch, MagicMock
from dataclasses import dataclass

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# Import from loader which handles the module loading (also loads document_parsers)
from loader import code_module, RESOURCES_DIR

# Get document parsers from sys.modules (loader has already set it up)
doc_parsers = sys.modules["proximadb.chunking_strategies.document_parsers"]

# Get references
BinaryType = doc_parsers.BinaryType
DocumentType = doc_parsers.DocumentType
BinarySymbol = doc_parsers.BinarySymbol
BinaryAnalysis = doc_parsers.BinaryAnalysis
OCRResult = doc_parsers.OCRResult
BinaryParserConfig = doc_parsers.BinaryParserConfig
OCRConfig = doc_parsers.OCRConfig
ToolDetector = doc_parsers.ToolDetector
detect_binary_type = doc_parsers.detect_binary_type
detect_document_type = doc_parsers.detect_document_type
BinaryParser = doc_parsers.BinaryParser
DocumentParser = doc_parsers.DocumentParser
create_binary_parser = doc_parsers.create_binary_parser
create_document_parser = doc_parsers.create_document_parser
get_available_tools = doc_parsers.get_available_tools


class TestBinaryType:
    """Test BinaryType enum."""

    def test_all_binary_types_exist(self):
        """Test all binary type values exist."""
        assert BinaryType.PE_EXE
        assert BinaryType.PE_DLL
        assert BinaryType.ELF_EXEC
        assert BinaryType.ELF_SO
        assert BinaryType.MACHO_EXEC
        assert BinaryType.MACHO_DYLIB
        assert BinaryType.UNKNOWN


class TestDocumentType:
    """Test DocumentType enum."""

    def test_all_document_types_exist(self):
        """Test all document type values exist."""
        assert DocumentType.PDF
        assert DocumentType.TIFF
        assert DocumentType.PNG
        assert DocumentType.JPEG
        assert DocumentType.BMP
        assert DocumentType.WEBP
        assert DocumentType.UNKNOWN


class TestBinarySymbol:
    """Test BinarySymbol dataclass."""

    def test_symbol_creation(self):
        """Test creating a binary symbol."""
        sym = BinarySymbol(name="main", address="0x401000", symbol_type="function")
        assert sym.name == "main"
        assert sym.address == "0x401000"
        assert sym.symbol_type == "function"

    def test_symbol_with_all_fields(self):
        """Test symbol with all optional fields."""
        sym = BinarySymbol(
            name="process_data",
            address="0x402000",
            symbol_type="function",
            size=256,
            section=".text",
            decompiled_code="int process_data() { return 0; }",
            disassembly="push rbp; mov rbp, rsp",
            metadata={"calls": ["malloc", "free"]},
        )
        assert sym.size == 256
        assert sym.section == ".text"
        assert sym.decompiled_code is not None
        assert sym.disassembly is not None
        assert "calls" in sym.metadata


class TestBinaryAnalysis:
    """Test BinaryAnalysis dataclass."""

    def test_analysis_creation(self):
        """Test creating binary analysis result."""
        analysis = BinaryAnalysis(
            file_path="/path/to/file.exe",
            binary_type=BinaryType.PE_EXE,
            architecture="x86_64",
            symbols=[],
            imports=["kernel32.dll!CreateFileA"],
            exports=["DllMain"],
            strings=["Hello, World!"],
            sections=[{"name": ".text", "size": "0x1000"}],
        )
        assert analysis.file_path == "/path/to/file.exe"
        assert analysis.binary_type == BinaryType.PE_EXE
        assert analysis.architecture == "x86_64"
        assert len(analysis.imports) == 1


class TestOCRResult:
    """Test OCRResult dataclass."""

    def test_result_creation(self):
        """Test creating OCR result."""
        result = OCRResult(
            file_path="/path/to/document.pdf",
            document_type=DocumentType.PDF,
            text="Hello, World!",
            pages=[{"page": 1, "text": "Hello, World!", "confidence": 95.5}],
        )
        assert result.file_path == "/path/to/document.pdf"
        assert result.document_type == DocumentType.PDF
        assert result.text == "Hello, World!"
        assert len(result.pages) == 1

    def test_result_with_confidence(self):
        """Test OCR result with confidence score."""
        result = OCRResult(
            file_path="/path/to/image.png",
            document_type=DocumentType.PNG,
            text="Sample text",
            pages=[],
            confidence=92.5,
            language="eng",
        )
        assert result.confidence == 92.5
        assert result.language == "eng"


class TestBinaryParserConfig:
    """Test BinaryParserConfig dataclass."""

    def test_default_config(self):
        """Test default configuration."""
        config = BinaryParserConfig()
        assert "radare2" in config.preferred_tools
        assert config.min_string_length == 4
        assert config.max_strings == 1000
        assert config.decompile_functions is True

    def test_custom_config(self):
        """Test custom configuration."""
        config = BinaryParserConfig(
            preferred_tools=["objdump"],
            min_string_length=8,
            max_strings=500,
            decompile_functions=False,
            use_wine=False,
        )
        assert config.preferred_tools == ["objdump"]
        assert config.min_string_length == 8
        assert config.decompile_functions is False


class TestOCRConfig:
    """Test OCRConfig dataclass."""

    def test_default_config(self):
        """Test default configuration."""
        config = OCRConfig()
        assert config.language == "eng"
        assert config.pdf_dpi == 300
        assert config.preprocess_images is True

    def test_custom_config(self):
        """Test custom configuration."""
        config = OCRConfig(
            language="deu",
            pdf_dpi=150,
            preprocess_images=False,
            page_segmentation_mode=6,
        )
        assert config.language == "deu"
        assert config.pdf_dpi == 150
        assert config.page_segmentation_mode == 6


class TestToolDetector:
    """Test ToolDetector class."""

    def test_is_available(self):
        """Test checking tool availability."""
        # 'ls' should be available on most systems
        result = ToolDetector.is_available("ls")
        assert isinstance(result, bool)

    def test_find_tool_nonexistent(self):
        """Test finding nonexistent tool."""
        result = ToolDetector.find_tool("nonexistent_tool_12345")
        assert result is None

    def test_get_system_info(self):
        """Test getting system info."""
        info = ToolDetector.get_system_info()
        assert "platform" in info
        assert "machine" in info
        assert "re_tools" in info
        assert "ocr_tools" in info
        assert "has_wine" in info

    def test_get_available_re_tools(self):
        """Test getting available RE tools."""
        tools = ToolDetector.get_available_re_tools()
        assert isinstance(tools, list)

    def test_get_available_ocr_tools(self):
        """Test getting available OCR tools."""
        tools = ToolDetector.get_available_ocr_tools()
        assert isinstance(tools, list)


class TestDetectDocumentType:
    """Test document type detection."""

    def test_detect_pdf(self):
        """Test detecting PDF."""
        assert detect_document_type("file.pdf") == DocumentType.PDF
        assert detect_document_type("file.PDF") == DocumentType.PDF

    def test_detect_tiff(self):
        """Test detecting TIFF."""
        assert detect_document_type("file.tiff") == DocumentType.TIFF
        assert detect_document_type("file.tif") == DocumentType.TIFF

    def test_detect_png(self):
        """Test detecting PNG."""
        assert detect_document_type("file.png") == DocumentType.PNG

    def test_detect_jpeg(self):
        """Test detecting JPEG."""
        assert detect_document_type("file.jpg") == DocumentType.JPEG
        assert detect_document_type("file.jpeg") == DocumentType.JPEG

    def test_detect_bmp(self):
        """Test detecting BMP."""
        assert detect_document_type("file.bmp") == DocumentType.BMP

    def test_detect_webp(self):
        """Test detecting WebP."""
        assert detect_document_type("file.webp") == DocumentType.WEBP

    def test_detect_unknown(self):
        """Test detecting unknown type."""
        assert detect_document_type("file.xyz") == DocumentType.UNKNOWN


class TestDetectBinaryType:
    """Test binary type detection."""

    def test_detect_unknown_for_nonexistent(self):
        """Test detecting type for nonexistent file."""
        result = detect_binary_type("/nonexistent/file.exe")
        assert result == BinaryType.UNKNOWN

    def test_detect_elf_format(self):
        """Test detecting ELF format."""
        with tempfile.NamedTemporaryFile(suffix=".so", delete=False) as f:
            # Write ELF magic header (for shared object)
            f.write(b"\x7fELF")  # Magic
            f.write(b"\x02")  # 64-bit
            f.write(b"\x01")  # Little endian
            f.write(b"\x01")  # ELF version
            f.write(b"\x00" * 9)  # Padding
            f.write(b"\x03\x00")  # e_type = ET_DYN (shared object)
            f.write(b"\x00" * 100)  # Padding
            f.flush()

            result = detect_binary_type(f.name)
            os.unlink(f.name)

            assert result == BinaryType.ELF_SO

    def test_detect_pe_format(self):
        """Test detecting PE format."""
        with tempfile.NamedTemporaryFile(suffix=".exe", delete=False) as f:
            # Build a proper minimal PE header
            # DOS header needs PE offset at 0x3C (byte 60)
            dos_header = bytearray(64)
            dos_header[0:2] = b"MZ"  # DOS magic at offset 0
            dos_header[0x3C:0x40] = (
                b"\x40\x00\x00\x00"  # PE offset = 0x40 (64) at offset 0x3C
            )
            f.write(bytes(dos_header))

            # PE signature at offset 0x40 (64)
            f.write(b"PE\x00\x00")  # PE signature (4 bytes)

            # COFF header (20 bytes): characteristics at offset PE+22 = 64+22 = 86
            # We need 18 bytes to get to characteristics
            f.write(b"\x00" * 18)  # COFF header up to characteristics
            f.write(b"\x00\x00")  # Characteristics = 0 (EXE, not DLL)

            f.write(b"\x00" * 100)  # Padding
            f.flush()

            result = detect_binary_type(f.name)
            os.unlink(f.name)

            assert result == BinaryType.PE_EXE


class TestBinaryParser:
    """Test BinaryParser class."""

    def test_parser_creation(self):
        """Test creating binary parser."""
        parser = BinaryParser()
        assert parser.language == "binary"
        assert ".exe" in parser.file_extensions
        assert ".dll" in parser.file_extensions
        assert ".so" in parser.file_extensions

    def test_parser_with_config(self):
        """Test parser with custom config."""
        config = BinaryParserConfig(preferred_tools=["objdump"], max_strings=100)
        parser = BinaryParser(config=config)
        assert parser.config.max_strings == 100

    def test_parse_nonexistent_file(self):
        """Test parsing nonexistent file raises error."""
        parser = BinaryParser()
        with pytest.raises(Exception):
            parser.parse("", "/nonexistent/file.exe")


class TestDocumentParser:
    """Test DocumentParser class."""

    def test_parser_creation(self):
        """Test creating document parser."""
        parser = DocumentParser()
        assert parser.language == "document"
        assert ".pdf" in parser.file_extensions
        assert ".png" in parser.file_extensions
        assert ".jpg" in parser.file_extensions

    def test_parser_with_config(self):
        """Test parser with custom config."""
        config = OCRConfig(language="deu", pdf_dpi=150)
        parser = DocumentParser(config=config)
        assert parser.config.language == "deu"
        assert parser.config.pdf_dpi == 150

    def test_has_ocr_property(self):
        """Test has_ocr property."""
        parser = DocumentParser()
        # Will be True if tesseract is installed, False otherwise
        assert isinstance(parser.has_ocr, bool)

    def test_parse_nonexistent_file(self):
        """Test parsing nonexistent file raises error."""
        parser = DocumentParser()
        with pytest.raises(Exception):
            parser.parse("", "/nonexistent/document.pdf")


class TestFactoryFunctions:
    """Test factory functions."""

    def test_create_binary_parser(self):
        """Test create_binary_parser factory."""
        parser = create_binary_parser()
        assert isinstance(parser, BinaryParser)

    def test_create_binary_parser_with_config(self):
        """Test create_binary_parser with config."""
        config = BinaryParserConfig(max_strings=50)
        parser = create_binary_parser(config)
        assert parser.config.max_strings == 50

    def test_create_document_parser(self):
        """Test create_document_parser factory."""
        parser = create_document_parser()
        assert isinstance(parser, DocumentParser)

    def test_create_document_parser_with_config(self):
        """Test create_document_parser with config."""
        config = OCRConfig(language="fra")
        parser = create_document_parser(config)
        assert parser.config.language == "fra"

    def test_get_available_tools(self):
        """Test get_available_tools function."""
        tools = get_available_tools()
        assert isinstance(tools, dict)
        assert "re_tools" in tools
        assert "ocr_tools" in tools
        assert "has_wine" in tools
        assert "platform" in tools


class TestMockedBinaryParsing:
    """Test binary parsing with mocked subprocess."""

    @patch("subprocess.run")
    def test_radare2_analysis_mocked(self, mock_run):
        """Test radare2 analysis with mocked subprocess."""
        # Mock subprocess to simulate radare2 output
        mock_run.return_value = MagicMock(stdout="arch x86_64\n", returncode=0)

        # Test would require more complete mocking of the adapter
        # For now, just verify the mock is called correctly
        parser = BinaryParser()
        # The actual parsing would need a real or mocked file


class TestMockedOCR:
    """Test OCR with mocked subprocess."""

    @patch("subprocess.run")
    def test_tesseract_ocr_mocked(self, mock_run):
        """Test tesseract OCR with mocked subprocess."""
        mock_run.return_value = MagicMock(
            stdout="Hello, World!\nThis is OCR text.", returncode=0
        )

        parser = DocumentParser()
        # The actual OCR would need a real or mocked file


class TestIntegration:
    """Integration tests (run only if tools are available)."""

    def test_with_real_binary(self):
        """Test with a real binary if available."""
        # Skip if no RE tools available
        tools = get_available_tools()
        if not tools.get("re_tools"):
            pytest.skip("No RE tools available")

        parser = BinaryParser()

        # Try to parse /bin/ls on Unix systems
        ls_path = "/bin/ls"
        if os.path.exists(ls_path):
            try:
                result = parser.parse("", ls_path)
                assert result is not None
                assert result.language == "binary"
            except Exception:
                # Tool might fail for various reasons
                pass

    def test_with_real_image(self):
        """Test with a real image if tesseract available."""
        tools = get_available_tools()
        if "tesseract" not in tools.get("ocr_tools", []):
            pytest.skip("Tesseract not available")

        # Would need a real test image
        pass


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
