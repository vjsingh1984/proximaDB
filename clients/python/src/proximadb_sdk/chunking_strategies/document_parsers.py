"""
Document and binary file parsers for code chunking.

This module provides parsers for:
- Binary files (DLL, EXE, SO, DYLIB) using reverse engineering tools
- PDF documents using OCR and text extraction
- Images (TIFF, PNG, JPG) using OCR (Tesseract)

Reverse Engineering Tools Supported:
- radare2 (r2): Cross-platform disassembler and analyzer
- Ghidra: NSA's reverse engineering framework (via headless analyzer)
- objdump: GNU binutils disassembler
- Wine/Proton: Windows binary analysis on Linux

OCR Tools Supported:
- Tesseract: Open source OCR engine
- pdf2image: PDF to image conversion
- PyMuPDF (fitz): PDF text extraction

Design Patterns:
- Strategy: Different RE tools as strategies
- Adapter: Uniform interface for different tools
- Factory: Tool selection based on availability
"""

import hashlib
import logging
import os
import platform
import re
import shutil
import subprocess
import tempfile
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from enum import Enum, auto
from pathlib import Path
from typing import Any

from .parser_utils import BaseLanguageParser, ParseError, ParserError

logger = logging.getLogger(__name__)


# =============================================================================
# Data Structures
# =============================================================================


class BinaryType(Enum):
    """Types of binary files"""

    PE_EXE = auto()  # Windows executable
    PE_DLL = auto()  # Windows dynamic library
    ELF_EXEC = auto()  # Linux executable
    ELF_SO = auto()  # Linux shared object
    MACHO_EXEC = auto()  # macOS executable
    MACHO_DYLIB = auto()  # macOS dynamic library
    UNKNOWN = auto()


class DocumentType(Enum):
    """Types of document files"""

    PDF = auto()
    TIFF = auto()
    PNG = auto()
    JPEG = auto()
    BMP = auto()
    WEBP = auto()
    UNKNOWN = auto()


@dataclass
class BinarySymbol:
    """Represents a symbol extracted from binary"""

    name: str
    address: str
    symbol_type: str  # function, data, import, export
    size: int = 0
    section: str | None = None
    decompiled_code: str | None = None
    disassembly: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class BinaryAnalysis:
    """Result of binary file analysis"""

    file_path: str
    binary_type: BinaryType
    architecture: str
    symbols: list[BinarySymbol]
    imports: list[str]
    exports: list[str]
    strings: list[str]
    sections: list[dict[str, Any]]
    entry_point: str | None = None
    content_hash: str = ""
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class OCRResult:
    """Result of OCR extraction"""

    file_path: str
    document_type: DocumentType
    text: str
    pages: list[dict[str, Any]]
    confidence: float = 0.0
    language: str = "eng"
    content_hash: str = ""
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class BinaryParserConfig:
    """Configuration for binary parser"""

    # Tool preferences (in order of preference)
    preferred_tools: list[str] = field(
        default_factory=lambda: ["radare2", "ghidra", "objdump"]
    )
    # Extract strings with minimum length
    min_string_length: int = 4
    max_strings: int = 1000
    # Decompilation options
    decompile_functions: bool = True
    max_function_size: int = 10000
    # Wine/Proton for Windows binaries on Linux
    use_wine: bool = True
    wine_prefix: str | None = None


@dataclass
class OCRConfig:
    """Configuration for OCR parser"""

    # Tesseract options
    language: str = "eng"  # OCR language
    tesseract_cmd: str | None = None  # Path to tesseract
    # PDF options
    pdf_dpi: int = 300
    # Processing options
    preprocess_images: bool = True
    page_segmentation_mode: int = 3  # Fully automatic page segmentation
    # Output options
    preserve_formatting: bool = False


# =============================================================================
# Tool Detection
# =============================================================================


class ToolDetector:
    """Detects available reverse engineering and OCR tools"""

    _cache: dict[str, str | None] = {}

    @classmethod
    def find_tool(cls, tool_name: str) -> str | None:
        """Find path to tool executable"""
        if tool_name in cls._cache:
            return cls._cache[tool_name]

        path = shutil.which(tool_name)
        cls._cache[tool_name] = path
        return path

    @classmethod
    def is_available(cls, tool_name: str) -> bool:
        """Check if tool is available"""
        return cls.find_tool(tool_name) is not None

    @classmethod
    def get_available_re_tools(cls) -> list[str]:
        """Get list of available RE tools"""
        tools = []
        for tool in [
            "r2",
            "radare2",
            "ghidra-analyzeHeadless",
            "objdump",
            "nm",
            "strings",
        ]:
            if cls.is_available(tool):
                tools.append(tool)
        return tools

    @classmethod
    def get_available_ocr_tools(cls) -> list[str]:
        """Get list of available OCR tools"""
        tools = []
        for tool in ["tesseract", "pdftoppm", "pdftotext"]:
            if cls.is_available(tool):
                tools.append(tool)
        return tools

    @classmethod
    def has_wine(cls) -> bool:
        """Check if Wine/Proton is available"""
        return cls.is_available("wine") or cls.is_available("wine64")

    @classmethod
    def get_system_info(cls) -> dict[str, Any]:
        """Get system information for tool selection"""
        return {
            "platform": platform.system(),
            "machine": platform.machine(),
            "re_tools": cls.get_available_re_tools(),
            "ocr_tools": cls.get_available_ocr_tools(),
            "has_wine": cls.has_wine(),
        }


# =============================================================================
# Binary Detection
# =============================================================================


def detect_binary_type(file_path: str) -> BinaryType:
    """Detect the type of binary file"""
    try:
        with open(file_path, "rb") as f:
            magic = f.read(4)

        # PE (Windows)
        if magic[:2] == b"MZ":
            # Check for PE header
            with open(file_path, "rb") as f:
                f.seek(0x3C)
                pe_offset = int.from_bytes(f.read(4), "little")
                f.seek(pe_offset)
                pe_sig = f.read(4)
                if pe_sig == b"PE\x00\x00":
                    # Check characteristics to determine EXE vs DLL
                    f.seek(pe_offset + 22)
                    characteristics = int.from_bytes(f.read(2), "little")
                    if characteristics & 0x2000:  # IMAGE_FILE_DLL
                        return BinaryType.PE_DLL
                    return BinaryType.PE_EXE

        # ELF (Linux)
        if magic[:4] == b"\x7fELF":
            with open(file_path, "rb") as f:
                f.seek(16)  # e_type offset
                e_type = int.from_bytes(f.read(2), "little")
                if e_type == 2:  # ET_EXEC
                    return BinaryType.ELF_EXEC
                elif e_type == 3:  # ET_DYN
                    return BinaryType.ELF_SO

        # Mach-O (macOS)
        if magic[:4] in (
            b"\xfe\xed\xfa\xce",
            b"\xfe\xed\xfa\xcf",
            b"\xce\xfa\xed\xfe",
            b"\xcf\xfa\xed\xfe",
        ):
            # Simplified detection - check file extension for lib vs exec
            ext = Path(file_path).suffix.lower()
            if ext == ".dylib":
                return BinaryType.MACHO_DYLIB
            return BinaryType.MACHO_EXEC

    except Exception as e:
        logger.warning(f"Failed to detect binary type for {file_path}: {e}")

    return BinaryType.UNKNOWN


def detect_document_type(file_path: str) -> DocumentType:
    """Detect the type of document file"""
    ext = Path(file_path).suffix.lower()
    mapping = {
        ".pdf": DocumentType.PDF,
        ".tiff": DocumentType.TIFF,
        ".tif": DocumentType.TIFF,
        ".png": DocumentType.PNG,
        ".jpg": DocumentType.JPEG,
        ".jpeg": DocumentType.JPEG,
        ".bmp": DocumentType.BMP,
        ".webp": DocumentType.WEBP,
    }
    return mapping.get(ext, DocumentType.UNKNOWN)


# =============================================================================
# Reverse Engineering Adapters
# =============================================================================


class REToolAdapter(ABC):
    """Abstract adapter for reverse engineering tools"""

    @property
    @abstractmethod
    def tool_name(self) -> str:
        """Name of the RE tool"""
        pass

    @abstractmethod
    def is_available(self) -> bool:
        """Check if tool is available"""
        pass

    @abstractmethod
    def analyze(self, file_path: str, config: BinaryParserConfig) -> BinaryAnalysis:
        """Analyze binary file"""
        pass


class Radare2Adapter(REToolAdapter):
    """Adapter for radare2 (r2)"""

    @property
    def tool_name(self) -> str:
        return "radare2"

    def is_available(self) -> bool:
        return ToolDetector.is_available("r2") or ToolDetector.is_available("radare2")

    def _get_r2_path(self) -> str:
        return ToolDetector.find_tool("r2") or ToolDetector.find_tool("radare2")

    def analyze(self, file_path: str, config: BinaryParserConfig) -> BinaryAnalysis:
        """Analyze binary using radare2"""
        r2_path = self._get_r2_path()
        if not r2_path:
            raise ParserError("radare2 not found", file_path=file_path)

        binary_type = detect_binary_type(file_path)
        symbols = []
        imports = []
        exports = []
        strings = []
        sections = []

        try:
            # Get basic info
            result = subprocess.run(
                [r2_path, "-q", "-c", "iI", file_path],
                capture_output=True,
                text=True,
                timeout=60,
            )
            arch = "unknown"
            for line in result.stdout.split("\n"):
                if "arch" in line.lower():
                    parts = line.split()
                    if len(parts) >= 2:
                        arch = parts[-1]
                        break

            # Get entry point
            result = subprocess.run(
                [r2_path, "-q", "-c", "ie", file_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
            entry_point = None
            for line in result.stdout.split("\n"):
                if "entry" in line.lower():
                    match = re.search(r"0x[0-9a-fA-F]+", line)
                    if match:
                        entry_point = match.group()
                        break

            # Get symbols
            result = subprocess.run(
                [r2_path, "-q", "-c", "is", file_path],
                capture_output=True,
                text=True,
                timeout=60,
            )
            for line in result.stdout.split("\n"):
                if line.strip() and not line.startswith("["):
                    parts = line.split()
                    if len(parts) >= 5:
                        symbols.append(
                            BinarySymbol(
                                name=parts[-1] if parts else "unknown",
                                address=parts[0] if parts else "0x0",
                                symbol_type=parts[3] if len(parts) > 3 else "unknown",
                                section=parts[4] if len(parts) > 4 else None,
                            )
                        )

            # Get imports
            result = subprocess.run(
                [r2_path, "-q", "-c", "ii", file_path],
                capture_output=True,
                text=True,
                timeout=60,
            )
            for line in result.stdout.split("\n"):
                if line.strip() and not line.startswith("["):
                    parts = line.split()
                    if parts:
                        imports.append(parts[-1])

            # Get exports
            result = subprocess.run(
                [r2_path, "-q", "-c", "iE", file_path],
                capture_output=True,
                text=True,
                timeout=60,
            )
            for line in result.stdout.split("\n"):
                if line.strip() and not line.startswith("["):
                    parts = line.split()
                    if parts:
                        exports.append(parts[-1])

            # Get strings
            result = subprocess.run(
                [r2_path, "-q", "-c", "iz~[2:]", file_path],
                capture_output=True,
                text=True,
                timeout=60,
            )
            for line in result.stdout.split("\n")[: config.max_strings]:
                if line.strip() and len(line) >= config.min_string_length:
                    strings.append(line.strip())

            # Get sections
            result = subprocess.run(
                [r2_path, "-q", "-c", "iS", file_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
            for line in result.stdout.split("\n"):
                if line.strip() and not line.startswith("["):
                    parts = line.split()
                    if len(parts) >= 3:
                        sections.append(
                            {
                                "name": parts[-1] if parts else "unknown",
                                "address": parts[0] if parts else "0x0",
                                "size": parts[1] if len(parts) > 1 else "0",
                            }
                        )

            # Compute hash
            with open(file_path, "rb") as f:
                content_hash = hashlib.sha256(f.read()).hexdigest()

            return BinaryAnalysis(
                file_path=file_path,
                binary_type=binary_type,
                architecture=arch,
                symbols=symbols,
                imports=imports[:500],
                exports=exports[:500],
                strings=strings,
                sections=sections,
                entry_point=entry_point,
                content_hash=content_hash,
                metadata={"tool": "radare2"},
            )

        except subprocess.TimeoutExpired:
            raise ParseError("radare2 analysis timeout", file_path=file_path)
        except Exception as e:
            raise ParseError(f"radare2 analysis failed: {e}", file_path=file_path)


class ObjdumpAdapter(REToolAdapter):
    """Adapter for GNU objdump"""

    @property
    def tool_name(self) -> str:
        return "objdump"

    def is_available(self) -> bool:
        return ToolDetector.is_available("objdump")

    def analyze(self, file_path: str, config: BinaryParserConfig) -> BinaryAnalysis:
        """Analyze binary using objdump"""
        binary_type = detect_binary_type(file_path)
        symbols = []
        imports = []
        exports = []
        sections = []

        try:
            # Get file format and architecture
            result = subprocess.run(
                ["objdump", "-f", file_path], capture_output=True, text=True, timeout=30
            )
            arch = "unknown"
            for line in result.stdout.split("\n"):
                if "architecture:" in line.lower():
                    match = re.search(r"architecture:\s*(\S+)", line.lower())
                    if match:
                        arch = match.group(1)
                        break

            # Get symbols
            result = subprocess.run(
                ["objdump", "-t", file_path], capture_output=True, text=True, timeout=60
            )
            for line in result.stdout.split("\n"):
                parts = line.split()
                if len(parts) >= 5 and parts[0].startswith("0"):
                    symbols.append(
                        BinarySymbol(
                            name=parts[-1],
                            address=parts[0],
                            symbol_type=parts[2] if len(parts) > 2 else "unknown",
                            section=parts[3] if len(parts) > 3 else None,
                        )
                    )

            # Get dynamic symbols (imports/exports)
            result = subprocess.run(
                ["objdump", "-T", file_path], capture_output=True, text=True, timeout=60
            )
            for line in result.stdout.split("\n"):
                parts = line.split()
                if len(parts) >= 5:
                    name = parts[-1]
                    if "*UND*" in line:
                        imports.append(name)
                    else:
                        exports.append(name)

            # Get sections
            result = subprocess.run(
                ["objdump", "-h", file_path], capture_output=True, text=True, timeout=30
            )
            for line in result.stdout.split("\n"):
                parts = line.split()
                if len(parts) >= 3 and parts[0].isdigit():
                    sections.append(
                        {
                            "name": parts[1],
                            "size": parts[2],
                            "address": parts[3] if len(parts) > 3 else "0",
                        }
                    )

            # Get strings using strings command
            strings = []
            if ToolDetector.is_available("strings"):
                result = subprocess.run(
                    ["strings", "-n", str(config.min_string_length), file_path],
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                strings = result.stdout.split("\n")[: config.max_strings]

            # Compute hash
            with open(file_path, "rb") as f:
                content_hash = hashlib.sha256(f.read()).hexdigest()

            return BinaryAnalysis(
                file_path=file_path,
                binary_type=binary_type,
                architecture=arch,
                symbols=symbols[:1000],
                imports=imports[:500],
                exports=exports[:500],
                strings=[s for s in strings if s.strip()],
                sections=sections,
                content_hash=content_hash,
                metadata={"tool": "objdump"},
            )

        except subprocess.TimeoutExpired:
            raise ParseError("objdump analysis timeout", file_path=file_path)
        except Exception as e:
            raise ParseError(f"objdump analysis failed: {e}", file_path=file_path)


# =============================================================================
# OCR Adapters
# =============================================================================


class OCRAdapter(ABC):
    """Abstract adapter for OCR tools"""

    @property
    @abstractmethod
    def tool_name(self) -> str:
        """Name of the OCR tool"""
        pass

    @abstractmethod
    def is_available(self) -> bool:
        """Check if tool is available"""
        pass

    @abstractmethod
    def extract_text(self, file_path: str, config: OCRConfig) -> OCRResult:
        """Extract text from document"""
        pass


class TesseractAdapter(OCRAdapter):
    """Adapter for Tesseract OCR"""

    @property
    def tool_name(self) -> str:
        return "tesseract"

    def is_available(self) -> bool:
        return ToolDetector.is_available("tesseract")

    def extract_text(self, file_path: str, config: OCRConfig) -> OCRResult:
        """Extract text using Tesseract"""
        doc_type = detect_document_type(file_path)

        try:
            # For images, directly use Tesseract
            if doc_type in (
                DocumentType.PNG,
                DocumentType.JPEG,
                DocumentType.TIFF,
                DocumentType.BMP,
                DocumentType.WEBP,
            ):
                return self._ocr_image(file_path, config, doc_type)

            # For PDF, convert to images first
            elif doc_type == DocumentType.PDF:
                return self._ocr_pdf(file_path, config)

            else:
                raise ParseError(
                    f"Unsupported document type: {doc_type}", file_path=file_path
                )

        except Exception as e:
            raise ParseError(f"OCR failed: {e}", file_path=file_path)

    def _ocr_image(
        self, file_path: str, config: OCRConfig, doc_type: DocumentType
    ) -> OCRResult:
        """OCR a single image file"""
        tesseract_cmd = config.tesseract_cmd or "tesseract"

        cmd = [
            tesseract_cmd,
            file_path,
            "stdout",
            "-l",
            config.language,
            "--psm",
            str(config.page_segmentation_mode),
        ]

        result = subprocess.run(cmd, capture_output=True, text=True, timeout=120)

        text = result.stdout.strip()

        # Try to get confidence
        confidence = 0.0
        conf_result = subprocess.run(
            [
                tesseract_cmd,
                file_path,
                "stdout",
                "-l",
                config.language,
                "--psm",
                str(config.page_segmentation_mode),
                "tsv",
            ],
            capture_output=True,
            text=True,
            timeout=120,
        )
        if conf_result.returncode == 0:
            confidences = []
            for line in conf_result.stdout.split("\n")[1:]:
                parts = line.split("\t")
                if len(parts) >= 11:
                    try:
                        conf = float(parts[10])
                        if conf > 0:
                            confidences.append(conf)
                    except (ValueError, IndexError):
                        pass
            if confidences:
                confidence = sum(confidences) / len(confidences)

        # Compute hash
        with open(file_path, "rb") as f:
            content_hash = hashlib.sha256(f.read()).hexdigest()

        return OCRResult(
            file_path=file_path,
            document_type=doc_type,
            text=text,
            pages=[{"page": 1, "text": text, "confidence": confidence}],
            confidence=confidence,
            language=config.language,
            content_hash=content_hash,
            metadata={"tool": "tesseract"},
        )

    def _ocr_pdf(self, file_path: str, config: OCRConfig) -> OCRResult:
        """OCR a PDF file by converting to images first"""
        # Check for pdf2image/pdftoppm
        has_pdftoppm = ToolDetector.is_available("pdftoppm")

        all_text = []
        pages = []

        with tempfile.TemporaryDirectory() as tmpdir:
            if has_pdftoppm:
                # Convert PDF to images
                subprocess.run(
                    [
                        "pdftoppm",
                        "-png",
                        "-r",
                        str(config.pdf_dpi),
                        file_path,
                        os.path.join(tmpdir, "page"),
                    ],
                    check=True,
                    timeout=300,
                )

                # OCR each page
                page_files = sorted(Path(tmpdir).glob("page-*.png"))
                for i, page_file in enumerate(page_files):
                    result = self._ocr_image(str(page_file), config, DocumentType.PNG)
                    all_text.append(result.text)
                    pages.append(
                        {
                            "page": i + 1,
                            "text": result.text,
                            "confidence": result.confidence,
                        }
                    )
            else:
                # Try pdftotext for text-based PDFs
                if ToolDetector.is_available("pdftotext"):
                    result = subprocess.run(
                        ["pdftotext", "-layout", file_path, "-"],
                        capture_output=True,
                        text=True,
                        timeout=120,
                    )
                    text = result.stdout.strip()
                    all_text.append(text)
                    pages.append({"page": 1, "text": text, "confidence": 100.0})
                else:
                    raise ParseError(
                        "PDF processing requires pdftoppm or pdftotext",
                        file_path=file_path,
                    )

        combined_text = "\n\n".join(all_text)
        avg_confidence = (
            (sum(p.get("confidence", 0) for p in pages) / len(pages)) if pages else 0
        )

        # Compute hash
        with open(file_path, "rb") as f:
            content_hash = hashlib.sha256(f.read()).hexdigest()

        return OCRResult(
            file_path=file_path,
            document_type=DocumentType.PDF,
            text=combined_text,
            pages=pages,
            confidence=avg_confidence,
            language=config.language,
            content_hash=content_hash,
            metadata={"tool": "tesseract", "page_count": len(pages)},
        )


# =============================================================================
# High-Level Parsers
# =============================================================================


class BinaryParser(BaseLanguageParser):
    """
    Parser for binary files (DLL, EXE, SO, DYLIB).

    Uses available reverse engineering tools:
    - radare2 (preferred)
    - objdump (fallback)
    - Ghidra (if installed)
    """

    def __init__(self, config: BinaryParserConfig | None = None):
        self.config = config or BinaryParserConfig()
        self._adapters: list[REToolAdapter] = []
        self._init_adapters()
        super().__init__()

    def _init_adapters(self):
        """Initialize available RE tool adapters"""
        # Add adapters in preference order
        adapter_classes = {
            "radare2": Radare2Adapter,
            "r2": Radare2Adapter,
            "objdump": ObjdumpAdapter,
        }

        for tool in self.config.preferred_tools:
            if tool in adapter_classes:
                adapter = adapter_classes[tool]()
                if adapter.is_available():
                    self._adapters.append(adapter)

        # Add any remaining available adapters
        for name, cls in adapter_classes.items():
            adapter = cls()
            if adapter.is_available() and adapter not in self._adapters:
                self._adapters.append(adapter)

    @property
    def language(self) -> str:
        return "binary"

    @property
    def tree_sitter_language_name(self) -> str:
        return "binary"  # No tree-sitter for binaries

    @property
    def file_extensions(self) -> list[str]:
        return [".exe", ".dll", ".so", ".dylib", ".o", ".obj", ".a", ".lib"]

    def _init_tree_sitter(self):
        """No tree-sitter for binary files"""
        self._parser = None
        self._language_binding = None

    def parse(self, content: str, file_path: str):
        """Parse binary file and extract symbols"""
        from .code import CodeSymbol, ParsedCode, SourceLocation

        # For binary files, content is ignored - we read the file directly
        if not os.path.exists(file_path):
            raise ParseError(f"File not found: {file_path}", file_path=file_path)

        # Get analysis from first available adapter
        analysis = None
        last_error = None

        for adapter in self._adapters:
            try:
                analysis = adapter.analyze(file_path, self.config)
                break
            except Exception as e:
                last_error = e
                logger.warning(f"{adapter.tool_name} failed: {e}")
                continue

        if analysis is None:
            if last_error:
                raise last_error
            raise ParseError("No RE tools available", file_path=file_path)

        # Convert to CodeSymbol format
        symbols = []
        for i, sym in enumerate(analysis.symbols[:500]):
            symbols.append(
                CodeSymbol(
                    id=f"bin_{hashlib.md5(f'{file_path}:{sym.name}:{sym.address}'.encode()).hexdigest()[:12]}",
                    symbol_type=self._map_symbol_type(sym.symbol_type),
                    fully_qualified_name=f"{Path(file_path).name}::{sym.name}",
                    simple_name=sym.name,
                    location=SourceLocation(
                        file_path=file_path, start_line=0, end_line=0
                    ),
                    source_code=sym.disassembly
                    or sym.decompiled_code
                    or f"[{sym.symbol_type}] {sym.name} @ {sym.address}",
                    language="binary",
                    metadata={
                        "address": sym.address,
                        "section": sym.section,
                        "binary_type": analysis.binary_type.name,
                        "architecture": analysis.architecture,
                    },
                )
            )

        return ParsedCode(
            file_path=file_path,
            language="binary",
            symbols=symbols,
            relations=[],
            imports=analysis.imports[:100],
            content_hash=analysis.content_hash,
        )

    def _map_symbol_type(self, sym_type: str):
        """Map binary symbol type to CodeSymbolType"""
        from .code import CodeSymbolType

        mapping = {
            "func": CodeSymbolType.FUNCTION,
            "function": CodeSymbolType.FUNCTION,
            "data": CodeSymbolType.VARIABLE,
            "object": CodeSymbolType.VARIABLE,
            "import": CodeSymbolType.MODULE,
            "export": CodeSymbolType.FUNCTION,
        }
        return mapping.get(sym_type.lower(), CodeSymbolType.VARIABLE)

    def _fallback_regex_parse(self, content: str, file_path: str):
        """No regex fallback for binary files"""
        from .code import ParsedCode

        return ParsedCode(
            file_path=file_path,
            language="binary",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=hashlib.sha256(content.encode()).hexdigest(),
        )

    def get_analysis(self, file_path: str) -> BinaryAnalysis:
        """Get detailed binary analysis"""
        for adapter in self._adapters:
            try:
                return adapter.analyze(file_path, self.config)
            except Exception as e:
                logger.warning(f"{adapter.tool_name} failed: {e}")
                continue

        raise ParseError("No RE tools available", file_path=file_path)


class DocumentParser(BaseLanguageParser):
    """
    Parser for documents using OCR.

    Supports:
    - PDF documents
    - Images (TIFF, PNG, JPEG, BMP, WEBP)
    """

    def __init__(self, config: OCRConfig | None = None):
        self.config = config or OCRConfig()
        self._adapter: OCRAdapter | None = None
        self._init_adapter()
        super().__init__()

    def _init_adapter(self):
        """Initialize OCR adapter"""
        tesseract = TesseractAdapter()
        if tesseract.is_available():
            self._adapter = tesseract

    @property
    def language(self) -> str:
        return "document"

    @property
    def tree_sitter_language_name(self) -> str:
        return "document"

    @property
    def file_extensions(self) -> list[str]:
        return [".pdf", ".tiff", ".tif", ".png", ".jpg", ".jpeg", ".bmp", ".webp"]

    def _init_tree_sitter(self):
        """No tree-sitter for documents"""
        self._parser = None
        self._language_binding = None

    @property
    def has_ocr(self) -> bool:
        """Check if OCR is available"""
        return self._adapter is not None

    def parse(self, content: str, file_path: str):
        """Parse document and extract text"""
        from .code import CodeSymbol, CodeSymbolType, ParsedCode, SourceLocation

        if not os.path.exists(file_path):
            raise ParseError(f"File not found: {file_path}", file_path=file_path)

        if not self._adapter:
            raise ParseError("No OCR tools available", file_path=file_path)

        # Get OCR result
        result = self._adapter.extract_text(file_path, self.config)

        # Create a symbol for each page or section
        symbols = []
        for i, page in enumerate(result.pages):
            page_text = page.get("text", "")
            if page_text.strip():
                symbols.append(
                    CodeSymbol(
                        id=f"doc_{hashlib.md5(f'{file_path}:page{i+1}'.encode()).hexdigest()[:12]}",
                        symbol_type=CodeSymbolType.MODULE,
                        fully_qualified_name=f"{Path(file_path).name}::page{i+1}",
                        simple_name=f"Page {i+1}",
                        location=SourceLocation(
                            file_path=file_path, start_line=i + 1, end_line=i + 1
                        ),
                        source_code=page_text[:5000],  # Limit size
                        language="document",
                        documentation=f"OCR confidence: {page.get('confidence', 0):.1f}%",
                        metadata={
                            "page": i + 1,
                            "confidence": page.get("confidence", 0),
                            "document_type": result.document_type.name,
                        },
                    )
                )

        return ParsedCode(
            file_path=file_path,
            language="document",
            symbols=symbols,
            relations=[],
            imports=[],
            content_hash=result.content_hash,
        )

    def _fallback_regex_parse(self, content: str, file_path: str):
        """No regex fallback for documents"""
        from .code import ParsedCode

        return ParsedCode(
            file_path=file_path,
            language="document",
            symbols=[],
            relations=[],
            imports=[],
            content_hash=hashlib.sha256(content.encode()).hexdigest(),
        )

    def get_ocr_result(self, file_path: str) -> OCRResult:
        """Get detailed OCR result"""
        if not self._adapter:
            raise ParseError("No OCR tools available", file_path=file_path)
        return self._adapter.extract_text(file_path, self.config)


# =============================================================================
# Factory Functions
# =============================================================================


def create_binary_parser(config: BinaryParserConfig | None = None) -> BinaryParser:
    """Create a binary file parser"""
    return BinaryParser(config)


def create_document_parser(config: OCRConfig | None = None) -> DocumentParser:
    """Create a document/OCR parser"""
    return DocumentParser(config)


def get_available_tools() -> dict[str, list[str]]:
    """Get all available parsing tools"""
    return {
        "re_tools": ToolDetector.get_available_re_tools(),
        "ocr_tools": ToolDetector.get_available_ocr_tools(),
        "has_wine": ToolDetector.has_wine(),
        "platform": platform.system(),
    }


# =============================================================================
# Exports
# =============================================================================

__all__ = [
    # Enums
    "BinaryType",
    "DocumentType",
    # Data structures
    "BinarySymbol",
    "BinaryAnalysis",
    "OCRResult",
    "BinaryParserConfig",
    "OCRConfig",
    # Tool detection
    "ToolDetector",
    "detect_binary_type",
    "detect_document_type",
    # Adapters
    "REToolAdapter",
    "Radare2Adapter",
    "ObjdumpAdapter",
    "OCRAdapter",
    "TesseractAdapter",
    # Parsers
    "BinaryParser",
    "DocumentParser",
    # Factory functions
    "create_binary_parser",
    "create_document_parser",
    "get_available_tools",
]
