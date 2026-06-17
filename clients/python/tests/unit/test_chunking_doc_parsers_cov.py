"""Offline coverage tests for chunking_strategies.document_parsers.

Fully offline: no real subprocess, no real RE/OCR tools. All tool detection and
subprocess.run calls are monkeypatched. Binary headers are synthesized in-memory
and written to tmp files so detect_binary_type runs against real bytes.
"""

import struct
import subprocess

import pytest

from proximadb_sdk.chunking_strategies import document_parsers as dp
from proximadb_sdk.chunking_strategies.document_parsers import (
    BinaryAnalysis,
    BinaryParser,
    BinaryParserConfig,
    BinarySymbol,
    BinaryType,
    DocumentParser,
    DocumentType,
    ObjdumpAdapter,
    OCRConfig,
    OCRResult,
    Radare2Adapter,
    TesseractAdapter,
    ToolDetector,
    create_binary_parser,
    create_document_parser,
    detect_binary_type,
    detect_document_type,
    get_available_tools,
)
from proximadb_sdk.chunking_strategies.parser_utils import ParseError, ParserError

# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeCompleted:
    def __init__(self, stdout="", returncode=0):
        self.stdout = stdout
        self.returncode = returncode
        self.stderr = ""


@pytest.fixture(autouse=True)
def _clear_tool_cache():
    """ToolDetector caches which() results process-wide; reset around each test."""
    ToolDetector._cache.clear()
    yield
    ToolDetector._cache.clear()


# ---------------------------------------------------------------------------
# Binary header builders
# ---------------------------------------------------------------------------


def _write(tmp_path, name, data):
    p = tmp_path / name
    p.write_bytes(data)
    return str(p)


def _pe_bytes(is_dll: bool) -> bytes:
    pe_offset = 0x40
    buf = bytearray(b"\x00" * (pe_offset + 24))
    buf[0:2] = b"MZ"
    struct.pack_into("<I", buf, 0x3C, pe_offset)
    buf[pe_offset : pe_offset + 4] = b"PE\x00\x00"
    characteristics = 0x2000 if is_dll else 0x0002
    struct.pack_into("<H", buf, pe_offset + 22, characteristics)
    return bytes(buf)


def _elf_bytes(e_type: int) -> bytes:
    buf = bytearray(b"\x00" * 32)
    buf[0:4] = b"\x7fELF"
    struct.pack_into("<H", buf, 16, e_type)
    return bytes(buf)


def _macho_bytes(magic: bytes) -> bytes:
    return magic + b"\x00" * 28


# ---------------------------------------------------------------------------
# detect_binary_type
# ---------------------------------------------------------------------------


def test_detect_pe_exe(tmp_path):
    path = _write(tmp_path, "a.exe", _pe_bytes(is_dll=False))
    assert detect_binary_type(path) == BinaryType.PE_EXE


def test_detect_pe_dll(tmp_path):
    path = _write(tmp_path, "a.dll", _pe_bytes(is_dll=True))
    assert detect_binary_type(path) == BinaryType.PE_DLL


def test_detect_elf_exec(tmp_path):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    assert detect_binary_type(path) == BinaryType.ELF_EXEC


def test_detect_elf_so(tmp_path):
    path = _write(tmp_path, "a.so", _elf_bytes(3))
    assert detect_binary_type(path) == BinaryType.ELF_SO


def test_detect_elf_unknown_etype(tmp_path):
    path = _write(tmp_path, "a.rel", _elf_bytes(1))
    assert detect_binary_type(path) == BinaryType.UNKNOWN


@pytest.mark.parametrize(
    "magic",
    [
        b"\xfe\xed\xfa\xce",
        b"\xfe\xed\xfa\xcf",
        b"\xce\xfa\xed\xfe",
        b"\xcf\xfa\xed\xfe",
    ],
)
def test_detect_macho_exec(tmp_path, magic):
    path = _write(tmp_path, "bin", _macho_bytes(magic))
    assert detect_binary_type(path) == BinaryType.MACHO_EXEC


def test_detect_macho_dylib(tmp_path):
    path = _write(tmp_path, "lib.dylib", _macho_bytes(b"\xfe\xed\xfa\xce"))
    assert detect_binary_type(path) == BinaryType.MACHO_DYLIB


def test_detect_binary_unknown_magic(tmp_path):
    path = _write(tmp_path, "x.bin", b"ABCD" + b"\x00" * 60)
    assert detect_binary_type(path) == BinaryType.UNKNOWN


def test_detect_binary_missing_file_returns_unknown():
    assert detect_binary_type("/nonexistent/path/xyz.bin") == BinaryType.UNKNOWN


# ---------------------------------------------------------------------------
# detect_document_type
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "name,expected",
    [
        ("doc.pdf", DocumentType.PDF),
        ("a.tiff", DocumentType.TIFF),
        ("a.tif", DocumentType.TIFF),
        ("a.png", DocumentType.PNG),
        ("a.jpg", DocumentType.JPEG),
        ("a.jpeg", DocumentType.JPEG),
        ("a.bmp", DocumentType.BMP),
        ("a.webp", DocumentType.WEBP),
        ("a.txt", DocumentType.UNKNOWN),
        ("noext", DocumentType.UNKNOWN),
    ],
)
def test_detect_document_type(name, expected):
    assert detect_document_type(name) == expected


# ---------------------------------------------------------------------------
# ToolDetector
# ---------------------------------------------------------------------------


def test_tool_detector_find_and_cache(monkeypatch):
    calls = []

    def fake_which(name):
        calls.append(name)
        return "/usr/bin/" + name if name == "objdump" else None

    monkeypatch.setattr(dp.shutil, "which", fake_which)
    assert ToolDetector.find_tool("objdump") == "/usr/bin/objdump"
    assert ToolDetector.is_available("objdump") is True
    assert ToolDetector.find_tool("objdump") == "/usr/bin/objdump"
    assert calls.count("objdump") == 1

    assert ToolDetector.find_tool("missing") is None
    assert ToolDetector.is_available("missing") is False


def test_tool_detector_available_lists(monkeypatch):
    present = {"r2", "objdump", "tesseract", "pdftotext", "wine"}
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: ("/bin/" + n) if n in present else None
    )
    re_tools = ToolDetector.get_available_re_tools()
    assert "r2" in re_tools and "objdump" in re_tools
    ocr_tools = ToolDetector.get_available_ocr_tools()
    assert "tesseract" in ocr_tools and "pdftotext" in ocr_tools
    assert ToolDetector.has_wine() is True


def test_tool_detector_has_wine64(monkeypatch):
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/wine64" if n == "wine64" else None
    )
    assert ToolDetector.has_wine() is True


def test_tool_detector_no_wine(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    assert ToolDetector.has_wine() is False


def test_tool_detector_system_info(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    info = ToolDetector.get_system_info()
    assert set(info) == {"platform", "machine", "re_tools", "ocr_tools", "has_wine"}
    assert info["re_tools"] == []
    assert info["has_wine"] is False


# ---------------------------------------------------------------------------
# Radare2Adapter
# ---------------------------------------------------------------------------


def test_radare2_not_available(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    a = Radare2Adapter()
    assert a.tool_name == "radare2"
    assert a.is_available() is False


def test_radare2_analyze(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.so", _elf_bytes(3))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/usr/bin/r2" if n == "r2" else None
    )

    outputs = {
        "iI": "arch x86\nbits 64",
        "ie": "vaddr=0x1234 entry0",
        "is": "0x1000 1 10 FUNC .text main\n0x2000 1 8 OBJ .data gvar",
        "ii": "0x0 import printf\n0x0 import malloc",
        "iE": "0x3000 export foo\n0x3000 export bar",
        "iz~[2:]": "hello world\nanother string\nx",
        "iS": "0x0 4096 x .text\n0x1000 2048 x .data",
    }

    def fake_run(args, **kw):
        cmd = args[3]  # ["r2", "-q", "-c", <cmd>, file]
        return FakeCompleted(stdout=outputs.get(cmd, ""))

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = Radare2Adapter()
    assert a.is_available() is True
    result = a.analyze(path, BinaryParserConfig())
    assert isinstance(result, BinaryAnalysis)
    assert result.binary_type == BinaryType.ELF_SO
    assert result.architecture == "x86"
    assert result.entry_point == "0x1234"
    assert any(s.name == "main" for s in result.symbols)
    assert "printf" in result.imports
    assert "foo" in result.exports
    assert "hello world" in result.strings
    assert any(sec["name"] == ".text" for sec in result.sections)
    assert result.metadata["tool"] == "radare2"
    assert len(result.content_hash) == 64


def test_radare2_analyze_missing_tool_raises(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.so", _elf_bytes(3))
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    a = Radare2Adapter()
    with pytest.raises(ParserError):
        a.analyze(path, BinaryParserConfig())


def test_radare2_analyze_timeout(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.so", _elf_bytes(3))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/usr/bin/r2" if n == "r2" else None
    )

    def fake_run(args, **kw):
        raise subprocess.TimeoutExpired(cmd="r2", timeout=60)

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    with pytest.raises(ParseError):
        Radare2Adapter().analyze(path, BinaryParserConfig())


def test_radare2_analyze_generic_error(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.so", _elf_bytes(3))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/usr/bin/r2" if n == "r2" else None
    )

    def fake_run(args, **kw):
        raise RuntimeError("boom")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    with pytest.raises(ParseError):
        Radare2Adapter().analyze(path, BinaryParserConfig())


# ---------------------------------------------------------------------------
# ObjdumpAdapter
# ---------------------------------------------------------------------------


def test_objdump_analyze(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    present = {"objdump", "strings"}
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: ("/bin/" + n) if n in present else None
    )

    def fake_run(args, **kw):
        tool = args[0]
        if tool == "objdump":
            flag = args[1]
            if flag == "-f":
                return FakeCompleted("architecture: i386:x86-64, flags 0x00")
            if flag == "-t":
                return FakeCompleted(
                    "0000000000001000 g F .text 0000 main\n"
                    "0000000000002000 g O .data 0000 gvar"
                )
            if flag == "-T":
                return FakeCompleted(
                    "0000 DF *UND* 0000 printf\n0000 g DF .text 0000 exported_fn"
                )
            if flag == "-h":
                return FakeCompleted("Idx Name Size VMA\n 0 .text 00001000 00000000\n")
        if tool == "strings":
            return FakeCompleted("alpha\nbeta\n\n")
        return FakeCompleted("")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    a = ObjdumpAdapter()
    assert a.tool_name == "objdump"
    assert a.is_available() is True
    result = a.analyze(path, BinaryParserConfig())
    assert result.binary_type == BinaryType.ELF_EXEC
    assert "x86-64" in result.architecture
    assert any(s.name == "main" for s in result.symbols)
    assert "printf" in result.imports
    assert "exported_fn" in result.exports
    assert any(sec["name"] == ".text" for sec in result.sections)
    assert "alpha" in result.strings
    assert result.metadata["tool"] == "objdump"


def test_objdump_analyze_no_strings_tool(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )
    monkeypatch.setattr(dp.subprocess, "run", lambda args, **kw: FakeCompleted(""))
    result = ObjdumpAdapter().analyze(path, BinaryParserConfig())
    assert result.strings == []


def test_objdump_analyze_timeout(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )

    def fake_run(args, **kw):
        raise subprocess.TimeoutExpired(cmd="objdump", timeout=30)

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    with pytest.raises(ParseError):
        ObjdumpAdapter().analyze(path, BinaryParserConfig())


def test_objdump_analyze_generic_error(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )

    def fake_run(args, **kw):
        raise RuntimeError("kaboom")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    with pytest.raises(ParseError):
        ObjdumpAdapter().analyze(path, BinaryParserConfig())


# ---------------------------------------------------------------------------
# TesseractAdapter
# ---------------------------------------------------------------------------


def test_tesseract_not_available(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    t = TesseractAdapter()
    assert t.tool_name == "tesseract"
    assert t.is_available() is False


def test_tesseract_ocr_image(tmp_path, monkeypatch):
    path = _write(tmp_path, "scan.png", b"\x89PNG\r\n")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )

    tsv = (
        "level\tpage\tblock\tpar\tline\tword\tleft\ttop\twidth\theight\tconf\ttext\n"
        "5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90.5\tHello\n"
        "5\t1\t1\t1\t1\t2\t0\t0\t10\t10\t80.5\tWorld\n"
    )

    def fake_run(args, **kw):
        if "tsv" in args:
            return FakeCompleted(tsv, returncode=0)
        return FakeCompleted("Hello World", returncode=0)

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    result = TesseractAdapter().extract_text(path, OCRConfig())
    assert isinstance(result, OCRResult)
    assert result.text == "Hello World"
    assert result.document_type == DocumentType.PNG
    assert result.confidence > 0
    assert result.pages[0]["page"] == 1
    assert result.metadata["tool"] == "tesseract"


def test_tesseract_ocr_image_conf_failure(tmp_path, monkeypatch):
    path = _write(tmp_path, "scan.jpg", b"\xff\xd8\xff")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )

    def fake_run(args, **kw):
        if "tsv" in args:
            return FakeCompleted("", returncode=1)  # confidence pass fails
        return FakeCompleted("text only")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    result = TesseractAdapter().extract_text(path, OCRConfig())
    assert result.text == "text only"
    assert result.confidence == 0.0


def test_tesseract_ocr_image_bad_conf_value(tmp_path, monkeypatch):
    # A non-numeric conf field exercises the ValueError swallow branch.
    path = _write(tmp_path, "scan.png", b"\x89PNG")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )

    tsv = (
        "h\n"
        "5\t1\t1\t1\t1\t1\t0\t0\t1\t1\tNOTANUM\tword\n"  # conf parse fails
        "5\t1\t1\t1\t1\t2\t0\t0\t1\t1\t-1\tword2\n"  # conf <= 0 ignored
        "5\t1\t1\t1\t1\t3\t0\t0\t1\t1\t77.0\tword3\n"  # valid
    )

    def fake_run(args, **kw):
        if "tsv" in args:
            return FakeCompleted(tsv, returncode=0)
        return FakeCompleted("body")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    result = TesseractAdapter().extract_text(path, OCRConfig())
    assert result.confidence == 77.0


def test_tesseract_unsupported_type(tmp_path, monkeypatch):
    path = _write(tmp_path, "doc.txt", b"hello")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )
    with pytest.raises(ParseError):
        TesseractAdapter().extract_text(path, OCRConfig())


def test_tesseract_ocr_pdf_via_pdftoppm(tmp_path, monkeypatch):
    path = _write(tmp_path, "doc.pdf", b"%PDF-1.4")
    present = {"tesseract", "pdftoppm"}
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: ("/bin/" + n) if n in present else None
    )

    def fake_run(args, **kw):
        if args[0] == "pdftoppm":
            out_prefix = args[-1]  # os.path.join(tmpdir, "page")
            png = out_prefix + "-1.png"
            with open(png, "wb") as f:
                f.write(b"\x89PNG")
            return FakeCompleted("")
        if "tsv" in args:
            return FakeCompleted(
                "h\n5\t1\t1\t1\t1\t1\t0\t0\t1\t1\t95.0\tpdfpage\n", returncode=0
            )
        return FakeCompleted("pdfpage text")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    result = TesseractAdapter().extract_text(path, OCRConfig())
    assert result.document_type == DocumentType.PDF
    assert "pdfpage text" in result.text
    assert result.metadata["page_count"] == 1
    assert result.pages[0]["page"] == 1


def test_tesseract_ocr_pdf_via_pdftotext(tmp_path, monkeypatch):
    path = _write(tmp_path, "doc.pdf", b"%PDF-1.4")
    present = {"tesseract", "pdftotext"}  # no pdftoppm
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: ("/bin/" + n) if n in present else None
    )

    def fake_run(args, **kw):
        if args[0] == "pdftotext":
            return FakeCompleted("extracted layout text")
        return FakeCompleted("")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    result = TesseractAdapter().extract_text(path, OCRConfig())
    assert result.document_type == DocumentType.PDF
    assert "extracted layout text" in result.text
    assert result.confidence == 100.0


def test_tesseract_ocr_pdf_no_tools(tmp_path, monkeypatch):
    path = _write(tmp_path, "doc.pdf", b"%PDF-1.4")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )
    monkeypatch.setattr(dp.subprocess, "run", lambda args, **kw: FakeCompleted(""))
    with pytest.raises(ParseError):
        TesseractAdapter().extract_text(path, OCRConfig())


# ---------------------------------------------------------------------------
# BinaryParser
# ---------------------------------------------------------------------------


def test_binary_parser_init_no_tools(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = BinaryParser()
    assert p.language == "binary"
    assert p.tree_sitter_language_name == "binary"
    assert ".exe" in p.file_extensions
    assert p.has_tree_sitter is False
    assert p._adapters == []


def test_binary_parser_init_with_adapters(monkeypatch):
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )
    p = BinaryParser(BinaryParserConfig(preferred_tools=["objdump", "radare2"]))
    assert any(isinstance(a, ObjdumpAdapter) for a in p._adapters)


def test_binary_parser_parse_file_not_found(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = BinaryParser()
    with pytest.raises(ParseError):
        p.parse("", "/no/such/file.exe")


def test_binary_parser_parse_no_tools(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.so", _elf_bytes(3))
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = BinaryParser()
    with pytest.raises(ParseError):
        p.parse("", path)


def test_binary_parser_parse_success(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )

    def fake_run(args, **kw):
        if args[0] == "objdump" and args[1] == "-t":
            return FakeCompleted(
                "0000000000001000 g F .text 0000 main\n"
                "0000000000002000 g O .data 0000 gvar"
            )
        if args[0] == "objdump" and args[1] == "-f":
            return FakeCompleted("architecture: i386:x86-64")
        if args[0] == "objdump" and args[1] == "-T":
            return FakeCompleted("0000 DF *UND* 0000 printf")
        return FakeCompleted("")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    p = BinaryParser()
    parsed = p.parse("", path)
    assert parsed.language == "binary"
    assert len(parsed.symbols) >= 1
    sym = parsed.symbols[0]
    assert sym.simple_name in ("main", "gvar")
    assert sym.metadata["binary_type"] == BinaryType.ELF_EXEC.name
    assert "printf" in parsed.imports


def test_binary_parser_parse_adapter_raises_propagates(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )

    def fake_run(args, **kw):
        raise RuntimeError("fail")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    p = BinaryParser()
    with pytest.raises(ParseError):
        p.parse("", path)


def test_binary_parser_map_symbol_type(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    from proximadb_sdk.chunking_strategies.code import CodeSymbolType

    p = BinaryParser()
    assert p._map_symbol_type("func") == CodeSymbolType.FUNCTION
    assert p._map_symbol_type("data") == CodeSymbolType.VARIABLE
    assert p._map_symbol_type("import") == CodeSymbolType.MODULE
    assert p._map_symbol_type("export") == CodeSymbolType.FUNCTION
    assert p._map_symbol_type("weird") == CodeSymbolType.VARIABLE


def test_binary_parser_fallback_regex(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = BinaryParser()
    parsed = p._fallback_regex_parse("some content", "x.exe")
    assert parsed.symbols == []
    assert parsed.language == "binary"
    assert len(parsed.content_hash) == 64


def test_binary_parser_get_analysis(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )
    monkeypatch.setattr(dp.subprocess, "run", lambda args, **kw: FakeCompleted(""))
    p = BinaryParser()
    analysis = p.get_analysis(path)
    assert isinstance(analysis, BinaryAnalysis)


def test_binary_parser_get_analysis_adapter_raises(tmp_path, monkeypatch):
    # Adapter present but analyze raises -> loop swallows and re-raises ParseError.
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/objdump" if n == "objdump" else None
    )

    def fake_run(args, **kw):
        raise RuntimeError("explode")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    p = BinaryParser()
    with pytest.raises(ParseError):
        p.get_analysis(path)


def test_binary_parser_get_analysis_no_tools(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.out", _elf_bytes(2))
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = BinaryParser()
    with pytest.raises(ParseError):
        p.get_analysis(path)


# ---------------------------------------------------------------------------
# DocumentParser
# ---------------------------------------------------------------------------


def test_document_parser_init_no_ocr(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = DocumentParser()
    assert p.language == "document"
    assert p.tree_sitter_language_name == "document"
    assert ".pdf" in p.file_extensions
    assert p.has_ocr is False
    assert p.has_tree_sitter is False


def test_document_parser_init_with_ocr(monkeypatch):
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )
    p = DocumentParser()
    assert p.has_ocr is True


def test_document_parser_parse_file_not_found(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = DocumentParser()
    with pytest.raises(ParseError):
        p.parse("", "/no/file.png")


def test_document_parser_parse_no_ocr(tmp_path, monkeypatch):
    path = _write(tmp_path, "a.png", b"\x89PNG")
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = DocumentParser()
    with pytest.raises(ParseError):
        p.parse("", path)


def test_document_parser_parse_success(tmp_path, monkeypatch):
    path = _write(tmp_path, "scan.png", b"\x89PNG")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )

    def fake_run(args, **kw):
        if "tsv" in args:
            return FakeCompleted(
                "h\n5\t1\t1\t1\t1\t1\t0\t0\t1\t1\t88.0\tword\n", returncode=0
            )
        return FakeCompleted("page body text")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    p = DocumentParser()
    parsed = p.parse("", path)
    assert parsed.language == "document"
    assert len(parsed.symbols) == 1
    sym = parsed.symbols[0]
    assert sym.simple_name == "Page 1"
    assert "page body text" in sym.source_code
    assert sym.metadata["document_type"] == DocumentType.PNG.name


def test_document_parser_parse_skips_empty_pages(tmp_path, monkeypatch):
    path = _write(tmp_path, "scan.png", b"\x89PNG")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )

    def fake_run(args, **kw):
        if "tsv" in args:
            return FakeCompleted("", returncode=1)
        return FakeCompleted("   ")  # whitespace-only -> stripped to empty

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    p = DocumentParser()
    parsed = p.parse("", path)
    assert parsed.symbols == []


def test_document_parser_fallback_regex(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = DocumentParser()
    parsed = p._fallback_regex_parse("content", "x.pdf")
    assert parsed.symbols == []
    assert parsed.language == "document"


def test_document_parser_get_ocr_result(tmp_path, monkeypatch):
    path = _write(tmp_path, "scan.png", b"\x89PNG")
    monkeypatch.setattr(
        dp.shutil, "which", lambda n: "/bin/tesseract" if n == "tesseract" else None
    )

    def fake_run(args, **kw):
        if "tsv" in args:
            return FakeCompleted("", returncode=1)
        return FakeCompleted("ocr text")

    monkeypatch.setattr(dp.subprocess, "run", fake_run)
    p = DocumentParser()
    result = p.get_ocr_result(path)
    assert isinstance(result, OCRResult)
    assert result.text == "ocr text"


def test_document_parser_get_ocr_result_no_ocr(tmp_path, monkeypatch):
    path = _write(tmp_path, "scan.png", b"\x89PNG")
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    p = DocumentParser()
    with pytest.raises(ParseError):
        p.get_ocr_result(path)


# ---------------------------------------------------------------------------
# Factory functions & dataclasses
# ---------------------------------------------------------------------------


def test_factory_functions(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    assert isinstance(create_binary_parser(), BinaryParser)
    assert isinstance(create_document_parser(), DocumentParser)
    assert isinstance(create_binary_parser(BinaryParserConfig()), BinaryParser)
    assert isinstance(create_document_parser(OCRConfig()), DocumentParser)


def test_get_available_tools(monkeypatch):
    monkeypatch.setattr(dp.shutil, "which", lambda n: None)
    tools = get_available_tools()
    assert set(tools) == {"re_tools", "ocr_tools", "has_wine", "platform"}
    assert tools["re_tools"] == []
    assert tools["has_wine"] is False


def test_dataclasses_defaults():
    sym = BinarySymbol(name="f", address="0x0", symbol_type="func")
    assert sym.size == 0 and sym.metadata == {}
    cfg = BinaryParserConfig()
    assert "radare2" in cfg.preferred_tools
    assert cfg.min_string_length == 4
    occfg = OCRConfig()
    assert occfg.language == "eng" and occfg.pdf_dpi == 300
    res = OCRResult(file_path="f", document_type=DocumentType.PDF, text="t", pages=[])
    assert res.confidence == 0.0 and res.language == "eng"
